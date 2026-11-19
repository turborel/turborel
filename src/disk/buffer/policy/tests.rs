//! Unit tests of the evictor a pushed policy programs: its choices
//! against CLOCK's of the same uses, each weight of the score, and its
//! reports (RULES.md R-TEST-1, R-TEST-2).

use std::sync::Arc;

use super::super::{Clock, Eviction, FileId, Key};
use super::{Policy, Range, Table, Weighted};

/// Page `id` of file 0.
fn page(id: u64) -> Key {
    Key { file: FileId(0), id }
}

/// An evictor of `frames` frames by `policy`, frame `f` holding page `f`.
fn filled(frames: usize, policy: &Policy) -> Arc<Weighted> {
    let table = Arc::new(Table::default());
    table.push(policy);
    let evictor = Arc::new(Weighted::new(frames, table));
    for f in 0..frames {
        evictor.placed(f, page(u64::try_from(f).unwrap()));
        evictor.accessed(f);
    }
    evictor
}

/// Verifies: [DESIGN] RULES.md D283: the default policy evicts the page
/// least recently used, by intervals, where CLOCK, after a page is used
/// again, sweeps to the first frame its hand finds clear; a range of
/// weight 0 keeps its page, chosen only when no other frame is evictable.
#[test]
fn the_policy_changes_which_frame_is_evicted() {
    let lru = filled(4, &Policy::default());
    let clock = Clock::new(4);
    for f in 0..4 {
        clock.accessed(f);
    }
    // Frame 0 used again after a miss: least recently used is now frame 1.
    lru.placed(3, page(9));
    lru.accessed(3);
    lru.accessed(0);
    clock.accessed(0);
    assert_eq!(lru.victim(&|_| true), Some(1));
    assert_eq!(clock.victim(&|_| true), Some(0), "CLOCK clears every bit, then takes the first frame");
    let keep = Policy { ranges: vec![Range { file: 0, first: 1, last: 1, weight: 0 }], ..Policy::default() };
    lru.table.push(&keep);
    lru.reweigh();
    assert_eq!(lru.victim(&|_| true), Some(2), "page 1 kept");
    assert_eq!(lru.victim(&|f| f == 1), Some(1), "a kept page is taken when it alone is evictable");
    assert_eq!(lru.victim(&|_| false), None);
}

/// Verifies: [DESIGN] RULES.md D284: HISTORY 2 counts a page's age from
/// its use before the last, so a page used once is evicted before one
/// used twice though used last; ONCE puts a page used in one interval
/// before one used again; FREQUENCY keeps pages used often; and a weight
/// over 100 evicts its page sooner.
#[test]
fn each_weight_of_the_score_counts() {
    let twice = |policy: Policy| {
        let e = filled(3, &policy);
        // Frame 0 used in a second interval, frame 2 last but once.
        e.placed(1, page(5));
        e.accessed(1);
        e.accessed(0);
        e.placed(2, page(6));
        e.accessed(2);
        e
    };
    assert_eq!(twice(Policy::default()).victim(&|_| true), Some(0), "LRU: ties go to the first frame looked at");
    assert_eq!(twice(Policy { history: 2, ..Policy::default() }).victim(&|_| true), Some(1));
    assert_eq!(twice(Policy { recency: 0, once: 5, ..Policy::default() }).victim(&|_| true), Some(1));
    assert_eq!(twice(Policy { recency: 10, frequency: 6, ..Policy::default() }).victim(&|_| true), Some(1));
    let sooner = Policy { once: 1, ranges: vec![Range { file: 0, first: 6, last: 6, weight: 1000 }], ..Policy::default() };
    assert_eq!(twice(sooner).victim(&|_| true), Some(2), "page 6 weighed ten times");
}

/// Verifies: [DESIGN] RULES.md D283, D284: a report samples the frames
/// holding pages, a window of SAMPLE moving on, and gives the pages
/// evicted since the last report once; the table keeps its last policy,
/// its ranges at most `RANGES`, and counts its pushes.
#[test]
fn reports_and_the_table() {
    let e = filled(4, &Policy::default());
    e.placed(0, page(7));
    let first = e.report(2);
    assert_eq!((first.frames, first.sampled.len(), first.evicted.clone()), (4, 2, vec![(0, 0)]));
    assert_eq!(first.sampled[0].page, 7);
    let second = e.report(2);
    assert!(second.evicted.is_empty());
    assert_eq!(second.sampled.iter().map(|s| s.page).collect::<Vec<_>>(), [2, 3]);
    let table = Table::default();
    let many = Policy { ranges: vec![Range { file: 0, first: 0, last: 0, weight: 0 }; super::RANGES + 1], ..Policy::default() };
    table.push(&many);
    assert_eq!((table.policy().ranges.len(), table.generation()), (super::RANGES, 2));
    assert_eq!(e.name(), "weighted");
}
