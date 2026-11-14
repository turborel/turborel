use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::value::{Decimal, SortOrder, Value};

/// The work area of a new database's operator.
fn work() -> Work {
    Work::new(&State::with_builtins(Box::new(MemoryCatalog::default())))
}

/// The next of a simple sequence of pseudo-random numbers below `n`.
fn below(state: &mut u64, n: u64) -> i128 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    i128::from((*state >> 33).checked_rem(n).unwrap_or_default())
}

/// `n` rows of a key of few values, some null, and their place.
fn made(state: &mut u64, n: i128) -> Vec<Row> {
    (0..n)
        .map(|place| {
            let key = below(state, 8);
            let key = if key == 0 { Value::Null } else { Value::Exact(Decimal::new(key, 0)) };
            vec![key, Value::Exact(Decimal::new(place, 0))]
        })
        .collect()
}

/// What the first `most` rows of a whole, stable sort of `rows` by `keys`
/// are, and of WITH TIES the peers of the last after them.
fn expected(keys: &[SortKey], rows: Vec<Row>, most: Most) -> Vec<Row> {
    let mut all = rows;
    all.sort_by(|a, b| compare_rows(keys, a, b));
    let mut first: Vec<Row> = all.iter().take(most.rows).cloned().collect();
    if let (true, Some(last)) = (most.ties, first.last().cloned()) {
        first.extend(all.into_iter().skip(most.rows).take_while(|r| compare_rows(keys, &last, r).is_eq()));
    }
    first
}

/// The rows `top` gives of `rows`, in `work`.
fn topped(keys: &[SortKey], rows: &[Row], most: Most, work: &mut Work) -> Vec<Row> {
    let owned: Vec<Row> = rows.to_owned();
    let input: RowIter = Box::new(owned.into_iter().map(Ok));
    top(keys, input, most, work).unwrap().map(Result::unwrap).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 GR5, GR7d] the first rows a sort
/// of only those holds are those a whole sort gives first, equal rows in the
/// order they came, ascending or descending, nulls first or last; of WITH
/// TIES with the peers of the last; of more asked for than there are, all.
#[test]
fn the_first_rows_held_are_a_whole_sorts_7_17() {
    let mut state = 7_u64;
    for order in [SortOrder::of(false, None), SortOrder::of(true, None), SortOrder::of(false, Some(true)), SortOrder::of(true, Some(false))]
    {
        let keys = [SortKey::new(0, order)];
        for n in [0, 1, 5, 40, 300] {
            let rows = made(&mut state, n);
            for most in [1, 2, 7, 64, 400] {
                for ties in [false, true] {
                    let most = Most { rows: most, ties };
                    let want = expected(&keys, rows.clone(), most);
                    let got = topped(&keys, &rows, most, &mut work());
                    assert_eq!(format!("{got:?}"), format!("{want:?}"), "{order:?} of {n} rows, {most:?}");
                }
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38, once the rows held outgrow the work area,
/// they and the rest are sorted whole, so the first rows given are still
/// a whole sort's, of WITH TIES too.
#[test]
fn rows_held_past_the_work_area_are_sorted_whole() {
    let mut state = 11_u64;
    let keys = [SortKey::ascending(0)];
    let rows = made(&mut state, 2000);
    for ties in [false, true] {
        let most = Most { rows: 1900, ties };
        let want = expected(&keys, rows.clone(), most);
        let got = topped(&keys, &rows, most, &mut work().part(4096));
        assert!(got.len() >= want.len(), "{} rows given", got.len());
        assert_eq!(format!("{:?}", got.get(..want.len())), format!("{:?}", Some(want.as_slice())), "ties {ties}");
    }
}
