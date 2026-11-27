//! Unit tests of a tree pinned by a thread (RULES.md D857, `pin.rs`): its
//! operations count nothing while the pin lives, the pin keeps the pages
//! taken out meanwhile, and a pin gone leaves them to count themselves.

use super::*;

/// Verifies: [DESIGN] RULES.md D857: a thread that pins a tree is guarded
/// by the pin, so its operations take no guard of their own, on that tree
/// alone; a leaf taken out while the pin lives is not freed until the pin
/// is dropped, whatever the thread does meanwhile; a second pin of the
/// tree dropped first leaves the first noted; and a thread with every
/// place taken pins unnoted, its operations guarding themselves.
#[test]
fn a_pin_guards_its_threads_operations_until_it_is_dropped() {
    let (tree, _scratch) = tree("pinned");
    let (other, _other) = super::tree("pinned-other");
    fill(&tree, 400);
    assert!(tree.epochs.guard().counted(), "not pinned");
    let pin = tree.pin();
    assert!(!tree.epochs.guard().counted(), "the pin guards it");
    assert!(other.epochs.guard().counted(), "another tree");
    let second = tree.pin();
    drop(second);
    assert!(!tree.epochs.guard().counted(), "the first pin still noted");
    for k in 100..300 {
        assert!(tree.delete(&key(k)).unwrap().is_some());
    }
    assert!(tree.counts().unlinked > 0);
    assert_eq!(keys(&tree, Direction::Forward).len(), 200);
    for _ in 0..4_u8 {
        assert!(tree.epochs.freeable().is_empty(), "kept while the thread is pinned");
    }
    drop(pin);
    assert!(tree.epochs.guard().counted(), "unnoted");
    let freed: usize = (0..4_u8).map(|_| tree.epochs.freeable().len()).sum();
    assert!(freed > 0, "freed once the pin is gone");
    let many: Vec<(BLinkTree, Scratch)> = (0..17_u8).map(|n| super::tree(&format!("pinned-{n}"))).collect();
    let pins: Vec<Pinned<'_>> = many.iter().map(|(one, _)| one.pin()).collect();
    let noted = many.iter().filter(|(one, _)| !one.epochs.guard().counted()).count();
    assert_eq!(noted, 16, "one more than the places");
    drop(pins);
    assert!(many.iter().all(|(one, _)| one.epochs.guard().counted()));
}
