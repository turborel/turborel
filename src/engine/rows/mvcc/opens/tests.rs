//! Unit tests of the shards of a table's open transactions (RULES.md
//! D784, R-TEST-1): which no API shows.

use super::*;

/// What `Opens::begin` makes: a reader's bookkeeping at snapshot 0.
fn open() -> Open {
    Open {
        snapshot: 0,
        changes: crate::spill::SpillList::new(crate::spill::ListSpace::default()),
        spilling: super::super::Spilling::Held,
        reads: super::super::Reads::default(),
        checked: true,
        reads_since: None,
        preserving: false,
        made: crate::hash::xxhash::Xxh3Set::default(),
        adopted: crate::hash::xxhash::Xxh3Set::default(),
    }
}

/// Verifies: [DESIGN] RULES.md D784: transactions next to each other fall
/// in shards of their own, each found, counted and ended in its own; the
/// newest but one and the oldest are found across them, and none open is
/// told by the counts. MC/DC of beginning: open already or not.
#[test]
fn open_transactions_are_kept_in_shards_of_their_own() {
    let opens = Opens::default();
    assert!(opens.is_empty());
    for tx in [7, 8, 40] {
        opens.begin(tx, open);
    }
    let first = opens.get(7).unwrap();
    opens.begin(7, || panic!("open already: nothing made"));
    assert!(Arc::ptr_eq(&first, &opens.get(7).unwrap()), "the one begun first");
    assert!(!std::ptr::eq(opens.shard(7).unwrap(), opens.shard(8).unwrap()), "next to each other, apart");
    assert!(std::ptr::eq(opens.shard(8).unwrap(), opens.shard(40).unwrap()), "one shard, by the id's rest");
    assert_eq!(opens.shard(8).unwrap().len.load(Ordering::SeqCst), 2);
    assert_eq!([opens.newest_but(40), opens.newest_but(7), opens.oldest()], [Some(8), Some(40), Some(7)]);
    for tx in [7, 8, 40] {
        assert!(opens.remove(tx).is_some(), "{tx}");
    }
    assert!(opens.remove(8).is_none(), "ended once");
    assert!(opens.is_empty() && opens.oldest().is_none());
}
