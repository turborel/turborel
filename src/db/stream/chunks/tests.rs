//! Output lookahead keeps row order, bounded memory and the first error,
//! and falls back without dropping a row when it cannot retain a batch.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::error::Code;
use crate::value::{Decimal, Value};

fn row(n: i32) -> Row {
    vec![Value::Exact(Decimal::new(i128::from(n), 0)), Value::Null]
}

fn shown(row: &Row) -> Vec<String> {
    row.iter().map(ToString::to_string).collect()
}

fn counted(rows: Vec<Result<Row>>) -> (RowIter, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let input = rows.into_iter().inspect(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
    });
    (Box::new(input), count)
}

/// Verifies: [DESIGN] D15, D38, D85: a spatial result reads no rows before
/// its first fetch, gives its first row directly, then reads bounded
/// groups. It preserves order and releases reservations on early close
/// or exhaustion.
#[test]
fn lookahead_is_lazy_bounded_and_releases_its_reservation() {
    let expected: Vec<_> = (0_i32..200_i32).map(row).collect();
    for finish in [false, true] {
        let budget = MemoryBudget::new(1 << 20);
        let (input, count) = counted(expected.iter().cloned().map(Ok).collect());
        let mut rows = read(input, &budget, 1 << 20, None);
        assert_eq!(count.load(Ordering::Relaxed), 0);
        assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&row(0))));
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&row(1))));
        assert_eq!(count.load(Ordering::Relaxed), ROWS.saturating_add(1));
        assert!(budget.reserved() >= headers(ROWS).saturating_add(ROWS.saturating_mul(row_bytes(&row(0)))));
        if finish {
            let actual = [row(0), row(1)].into_iter().chain(rows.by_ref().map(Result::unwrap)).collect::<Vec<_>>();
            assert_eq!(actual.iter().map(shown).collect::<Vec<_>>(), expected.iter().map(shown).collect::<Vec<_>>());
        }
        drop(rows);
        assert_eq!(budget.reserved(), 0);
    }
}

/// Verifies: [DESIGN] D38, D85: refused metadata, a row exceeding the
/// work area and refused payload growth all retain every row exactly
/// once. The first unbuffered row follows the reserved prefix, including
/// variable-width and NULL values. Fallback adds no resource error.
#[test]
fn lookahead_refusal_preserves_the_prefix_and_overflow_row() {
    let mut expected: Vec<_> = (0_i32..200_i32).map(row).collect();
    expected[3] = vec![Value::Char("x".repeat(16_384)), Value::Null];
    let normal = headers(ROWS).saturating_add(ROWS.saturating_mul(row_bytes(&row(0))));
    for (bytes, limit) in [(0, usize::MAX), (usize::MAX, 0), (usize::MAX, normal), (normal, usize::MAX)] {
        let budget = MemoryBudget::new(bytes);
        let (input, count) = counted(expected.iter().cloned().map(Ok).collect());
        let mut rows = read(input, &budget, limit, None);
        assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&row(0))));
        assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&row(1))));
        assert!(count.load(Ordering::Relaxed) <= 4, "only the prefix and its first overflow row");
        assert!(budget.reserved() <= bytes.min(limit));
        let actual = [row(0), row(1)].into_iter().chain(rows.map(Result::unwrap)).collect::<Vec<_>>();
        assert_eq!(actual.iter().map(shown).collect::<Vec<_>>(), expected.iter().map(shown).collect::<Vec<_>>());
        assert_eq!(budget.reserved(), 0);
    }
}

/// Verifies: [DESIGN] D15, D85: a storage error follows every preceding
/// row, is returned once, and ends the stream without reading later rows.
/// Errors at the first row, a group boundary and within a group agree.
#[test]
fn lookahead_keeps_the_first_error_after_its_preceding_rows() {
    for at in [0_i32, 64_i32, 67_i32] {
        let budget = MemoryBudget::new(1 << 20);
        let expected: Vec<_> = (0_i32..at).map(row).collect();
        let mut input: Vec<_> = expected.iter().cloned().map(Ok).collect();
        input.push(Err(SqlError::new(Code::Corrupt, "[DESIGN] injected spatial storage failure")));
        input.push(Ok(row(999)));
        let (input, count) = counted(input);
        let mut rows = read(input, &budget, 1 << 20, None);
        for preceding in expected {
            assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&preceding)));
        }
        assert_eq!(rows.next().unwrap().unwrap_err().code, Code::Corrupt);
        assert!(rows.next().is_none());
        assert_eq!(count.load(Ordering::Relaxed), usize::try_from(at).unwrap().saturating_add(1));
        drop(rows);
        assert_eq!(budget.reserved(), 0);
    }
}

/// Verifies: [DESIGN] D44, D85: cancellation during lookahead fails the
/// current fetch immediately, before it returns a buffered row. Later
/// fetches still fail, even in a new cancellation window, rather than
/// reporting successful end of data. No later input is consumed.
#[test]
fn cancellation_during_lookahead_fails_this_fetch_and_every_later_fetch() {
    let budget = MemoryBudget::new(1 << 20);
    let stop = Arc::new(Stop::default());
    let count = Arc::new(AtomicUsize::new(0));
    let (cancel, seen) = (Arc::clone(&stop), Arc::clone(&count));
    let input = (0_i32..100_i32).map(move |n| {
        seen.fetch_add(1, Ordering::Relaxed);
        if n == 3_i32 {
            cancel.cancel();
        }
        Ok(row(n))
    });
    let input = Box::new(checked(input, Some(Arc::clone(&stop))));
    let mut rows = read(input, &budget, 1 << 20, Some(Arc::clone(&stop)));
    {
        let _window = stop.window();
        assert_eq!(rows.next().transpose().unwrap().as_ref().map(shown), Some(shown(&row(0))));
    }
    for _ in 0_i32..2_i32 {
        let _window = stop.window();
        assert_eq!(rows.next().unwrap().unwrap_err().code, Code::Cancelled);
    }
    assert_eq!(count.load(Ordering::Relaxed), 4);
    drop(rows);
    assert_eq!(budget.reserved(), 0);
}
