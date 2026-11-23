//! Unit tests of the inner node copies a thread keeps (RULES.md D807,
//! `inner.rs`): 256 of them, the one to give way chosen by a clock bit,
//! each with its own header parsed (D839).

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
/// would drop the ones read. A copy found is put in its place whether
/// the copies are full or not; one not found is added while they are not
/// full (MC/DC of `Copies::put`, found or not, full or not). A copy put
/// in another's place keeps its own header as parsed (D839).
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
                {
                    let got = read(i);
                    let parsed = got.node().unwrap().parsed();
                    inner::keep(&got, page(i), parsed);
                }
            }
            assert_eq!(inner::kept().len(), inner::KEPT, "all kept");
            // Every bit is set, so the hand clears them all and takes the first.
            {
                let got = read(inner::KEPT);
                let parsed = got.node().unwrap().parsed();
                inner::keep(&got, page(inner::KEPT), parsed);
            }
            assert!(!inner::kept().contains(&page(0)), "the first gave way");
            // The copy that took its place keeps its own header parsed, not
            // the one it replaced (RULES.md D839).
            let inner::Read::Copied(bytes, header) = read(inner::KEPT) else { panic!("a copy read") };
            assert_eq!(Node::parsed_again(&bytes, &header).unwrap().parsed(), Node::parse(&bytes).unwrap().parsed());
            for i in 1..10 {
                assert!(matches!(read(i), inner::Read::Copied(..)), "copy {i} read");
            }
            let next = inner::KEPT.checked_add(1).unwrap();
            {
                let got = read(next);
                let parsed = got.node().unwrap().parsed();
                inner::keep(&got, page(next), parsed);
            }
            let kept = inner::kept();
            assert!((1..10).all(|i| kept.contains(&page(i))), "the copies read are kept");
            assert!(!kept.contains(&page(10)), "the first unread one gave way");
            assert!(kept.contains(&page(next)) && kept.len() == inner::KEPT);
            // A page kept already, read through its latch, takes its own
            // place, so nothing gives way.
            {
                let got = inner::read(&tree.pool, page(5), None, false).unwrap();
                let parsed = got.node().unwrap().parsed();
                inner::keep(&got, page(5), parsed);
            }
            assert_eq!(inner::kept(), kept, "kept in place");
        });
    });
}
