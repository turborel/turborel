//! Unit tests of the inner node copies a thread keeps (RULES.md D807,
//! `inner.rs`): 256 of them, the one to give way chosen by a clock bit.

use super::*;

/// The leaves of `tree`, each once, in key order, its keys `1..=n`.
fn leaves(tree: &BLinkTree, n: u64) -> Vec<u64> {
    let mut found: Vec<u64> = Vec::new();
    for k in 1..=n {
        let leaf = tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0.0;
        if found.last() != Some(&leaf) {
            found.push(leaf);
        }
    }
    found
}

/// Verifies: [DESIGN] RULES.md D807, D502: a thread keeps 256 copies, not
/// fewer; when one must give way, those read since the hand last passed
/// are kept and the first unread one goes, where taking each in turn
/// would drop the ones read.
#[test]
fn copies_read_since_the_hand_passed_are_kept() {
    let (tree, _scratch) = tree("copies-clock");
    fill(&tree, 20_000);
    let pages = leaves(&tree, 20_000);
    assert!(pages.len() > inner::KEPT.checked_add(2).unwrap(), "{} pages", pages.len());
    std::thread::scope(|s| {
        s.spawn(|| {
            let page = |i: usize| (tree.file, pages.get(i).copied().unwrap());
            let read = |i: usize| inner::read(&tree.pool, page(i), None, true).unwrap();
            for i in 0..inner::KEPT {
                inner::keep(&read(i), page(i));
            }
            assert_eq!(inner::kept().len(), inner::KEPT, "all kept");
            // Every bit set: the hand clears them all and takes the first.
            inner::keep(&read(inner::KEPT), page(inner::KEPT));
            assert!(!inner::kept().contains(&page(0)), "the first gave way");
            for i in 1..10 {
                assert!(matches!(read(i), inner::Read::Copied(_)), "copy {i} read");
            }
            let next = inner::KEPT.checked_add(1).unwrap();
            inner::keep(&read(next), page(next));
            let kept = inner::kept();
            assert!((1..10).all(|i| kept.contains(&page(i))), "the copies read are kept");
            assert!(!kept.contains(&page(10)), "the first unread one gave way");
            assert!(kept.contains(&page(next)) && kept.len() == inner::KEPT);
        });
    });
}
