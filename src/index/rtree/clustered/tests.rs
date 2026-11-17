//! Candidate grouping preserves entries, lazy errors and budget fallback.

use super::*;
use crate::error::Code;

fn entries(count: usize) -> Vec<Entry> {
    (0..count)
        .map(|at| {
            let coordinate = f64::from(u32::try_from(at).unwrap());
            (Bounds::point([coordinate, coordinate]).unwrap(), u64::try_from(count.saturating_sub(at)).unwrap() % 73)
        })
        .collect()
}

/// Verifies: [DESIGN] D38, D82: blocks preserve every envelope/row pair,
/// including duplicate row ids for different versions, and sort large
/// blocks for storage locality. The initial and grown buffers are charged,
/// bounded independently of result size, and released with the cursor.
#[test]
fn clustered_candidates_preserve_all_versions_and_bound_memory() {
    let budget = MemoryBudget::new(usize::MAX);
    let source = entries(BLOCK * 2 + SORT_FROM);
    let mut scan = Clustered::new(source.iter().copied().map(Ok), Some(&budget));
    assert_eq!(budget.reserved(), INITIAL * size_of::<Entry>());
    let mut actual = vec![scan.next().unwrap().unwrap()];
    assert_eq!(budget.reserved(), BLOCK * size_of::<Entry>());
    actual.extend(scan.by_ref().map(Result::unwrap));
    assert!(scan.next().is_none());
    drop(scan);
    assert_eq!(budget.reserved(), 0);
    for block in actual.chunks(BLOCK) {
        assert!(block.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    }
    let key = |entry: &Entry| (entry.1, entry.0.lower()[0].to_bits());
    let mut expected = source;
    expected.sort_unstable_by_key(key);
    actual.sort_unstable_by_key(key);
    assert_eq!(actual, expected);
}

/// Verifies: [DESIGN] D38, D82: refusing either the initial allocation or
/// its growth independently preserves every candidate in streaming order.
/// A small result keeps only the small buffer; an absent budget opts out.
#[test]
fn clustered_candidates_decline_unaffordable_buffers_without_losing_rows() {
    let source = entries(BLOCK + 1);
    for limit in [0_usize, INITIAL * size_of::<Entry>()] {
        let budget = MemoryBudget::new(limit);
        let scan = Clustered::new(source.iter().copied().map(Ok), Some(&budget));
        assert_eq!(scan.map(Result::unwrap).collect::<Vec<_>>(), source);
        assert_eq!(budget.reserved(), 0);
    }
    let scan = Clustered::new(source.iter().copied().map(Ok), None);
    assert_eq!(scan.map(Result::unwrap).collect::<Vec<_>>(), source);
    let budget = MemoryBudget::new(usize::MAX);
    for count in [0_usize, INITIAL - 1] {
        let small = entries(count);
        let mut small_scan = Clustered::new(small.iter().copied().map(Ok), Some(&budget));
        assert_eq!(small_scan.by_ref().map(Result::unwrap).collect::<Vec<_>>(), small);
        assert_eq!(budget.reserved(), INITIAL * size_of::<Entry>());
        drop(small_scan);
        assert_eq!(budget.reserved(), 0);
    }
}

/// Verifies: [DESIGN] D15, D82: prefetch does not expose an index failure
/// before its preceding candidates. No entry after that failure escapes,
/// and a cursor stopped after its first candidate never observes it.
#[test]
fn clustered_candidates_defer_prefetched_errors_until_prior_rows() {
    let budget = MemoryBudget::new(usize::MAX);
    let source = entries(4);
    let error = SqlError::new(Code::Corrupt, "[DESIGN] a later index page failed");
    let input = source.iter().copied().map(Ok).chain([Err(error)]).chain(source.iter().copied().map(Ok));
    let mut scan = Clustered::new(input, Some(&budget));
    for expected in &source {
        assert_eq!(&scan.next().unwrap().unwrap(), expected);
    }
    assert_eq!(scan.next().unwrap().unwrap_err().code, Code::Corrupt);
    assert!(scan.next().is_none());
    assert!(scan.next().is_none());
    drop(scan);
    assert_eq!(budget.reserved(), 0);
}
