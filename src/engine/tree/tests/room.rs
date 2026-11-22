//! Unit tests of room in a node for a change in place (RULES.md D500,
//! `room.rs`): garbage reclaimed only once it is worth the node built
//! again, a full node short of that split, and a shorter value written
//! over in place.

use super::*;

/// The root leaf of `tree`, holding keys 1 to `n` with `VALUE` and key 0
/// with a value long enough that the leaf has no room left; its page.
fn brimming(tree: &BLinkTree, n: u64) -> PageId {
    let root = PageId(tree.root.load(Ordering::Acquire));
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: tree.meta.0 };
    let keys: Vec<[u8; 8]> = (0..=n).map(key).collect();
    let built = |filler: usize| {
        let long = vec![5_u8; filler];
        let entries: Vec<(&[u8], &[u8])> =
            keys.iter().enumerate().map(|(i, k)| (k.as_slice(), if i == 0 { long.as_slice() } else { VALUE.as_slice() })).collect();
        node::build(&shape, &entries).unwrap()
    };
    let filler = tree.capacity.checked_sub(built(1).len()).and_then(|f| f.checked_add(1)).unwrap();
    let content = built(filler);
    assert_eq!(content.len(), tree.capacity, "no room left");
    *tree.pool.write(tree.file, root).unwrap() = content;
    root
}

/// The garbage of node `id` of `tree`.
fn garbage(tree: &BLinkTree, id: PageId) -> usize {
    Node::parse(&tree.pool.read(tree.file, id.0).unwrap()).unwrap().garbage()
}

/// Verifies: [DESIGN] RULES.md D500: in a leaf with no room, an entry
/// removed and added again, which fits once the leaf is built again,
/// splits it while its garbage is less than an eighth of the page; once
/// entries removed leave an eighth, the entry added builds it again, its
/// garbage gone, and nothing splits. Every key is found in order.
#[test]
fn garbage_is_reclaimed_only_once_it_is_an_eighth_of_the_page() {
    for few in [true, false] {
        let (tree, _scratch) = tree(if few { "room-few" } else { "room-many" });
        let n = u64::try_from(tree.capacity.checked_div(110).unwrap()).unwrap();
        let leaf = brimming(&tree, n);
        let removed = if few { 1 } else { u64::try_from(tree.capacity.checked_div(720).unwrap()).unwrap().checked_add(1).unwrap() };
        for k in 1..=removed {
            assert!(tree.delete(&key(k)).unwrap().is_some());
        }
        assert!(garbage(&tree, leaf) > 0);
        assert!(tree.insert(&key(1), &VALUE).unwrap());
        let counts = tree.counts();
        assert_eq!(counts.splits, u64::from(few), "few: {few}");
        // Split or built again, the leaf holds no garbage now.
        assert_eq!(garbage(&tree, leaf), 0, "few: {few}");
        let want: Vec<Vec<u8>> = (0..=n).filter(|&k| k == 0 || k == 1 || k > removed).map(|k| key(k).to_vec()).collect();
        assert_eq!(keys(&tree, Direction::Forward), want, "few: {few}");
    }
}

/// Verifies: [DESIGN] RULES.md D500, D31: a value replaced by a shorter
/// one in a leaf with no room is written over in place, no split and no
/// node built again, the bytes it leaves counted as garbage; a longer one
/// takes the room the shorter left.
#[test]
fn a_shorter_value_is_written_over_in_place() {
    let (tree, _scratch) = tree("room-shorter");
    let n = u64::try_from(tree.capacity.checked_div(110).unwrap()).unwrap();
    let leaf = brimming(&tree, n);
    let short = [3_u8; 60];
    assert!(tree.replace(&key(2), &short).unwrap().is_some());
    assert_eq!((tree.counts().splits, garbage(&tree, leaf)), (0, VALUE.len().checked_sub(short.len()).unwrap()));
    assert_eq!(tree.get(&key(2)).unwrap(), Some(short.to_vec()));
    assert_eq!(tree.get(&key(3)).unwrap(), Some(VALUE.to_vec()));
}
