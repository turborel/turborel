//! Unit tests of a probe from a key (RULES.md D859, `probe.rs`): the
//! entries at or after the key, in order, shown in place until the caller
//! stops, across leaves, with no buffer of its own once the thread has one.

use super::*;

/// The keys `tree` shows a probe from `k` that takes `most` entries.
fn probed(tree: &BLinkTree, k: u64, most: usize) -> Vec<Vec<u8>> {
    let mut seen = Vec::new();
    let show = |found: &[u8], value: &[u8]| {
        assert_eq!(value, &VALUE[..]);
        seen.push(found.to_vec());
        Ok(if seen.len() < most { Visit::Taken } else { Visit::Stop })
    };
    tree.first_from(&key(k), show).unwrap();
    seen
}

/// Verifies: [DESIGN] RULES.md D859: a probe shows the entries at or
/// after its key, in key order, the key's own first if it is there, the
/// next above it if not; it ends when its caller stops, or at the last
/// entry; it goes on across leaves, each key shown once; it counts as a
/// lookup, the tree's inserts counting none; and it fails as its caller
/// does.
#[test]
fn a_probe_shows_the_entries_from_its_key_until_stopped() {
    let (tree, _scratch) = tree("probed");
    for k in 1..=300_u64 {
        assert!(tree.insert(&key(k * 2), &VALUE).unwrap());
    }
    assert!(tree.counts().splits > 5);
    assert_eq!(probed(&tree, 10, 1), [key(10).to_vec()], "its own");
    assert_eq!(probed(&tree, 11, 2), [key(12).to_vec(), key(14).to_vec()], "the next above");
    let all: Vec<Vec<u8>> = (1..=300_u64).map(|k| key(k * 2).to_vec()).collect();
    assert_eq!(probed(&tree, 0, usize::MAX), all, "every leaf, each key once");
    assert_eq!(probed(&tree, 500, usize::MAX), all[249..], "to the last entry");
    assert!(probed(&tree, 601, 1).is_empty(), "past every key");
    assert_eq!(tree.missed_share(1_000_000), None, "too few lookups yet");
    assert!(tree.missed_share(5).is_some(), "each probe a lookup in the tally");
    let refused = tree.first_from(&key(10), |_, _| Err(SqlError::new(Code::Api, "[DESIGN] refused")));
    assert_eq!(refused.unwrap_err().code, Code::Api);
    assert_eq!(probed(&tree, 10, 1), [key(10).to_vec()], "the buffer given back after a failure");
}
