//! Unit tests of splits of nodes whose keys share a long prefix (RULES.md
//! D500, `fit.rs`): a key without it, below or above them, splits the
//! node into nodes that each fit their page, the rightmost leaf written
//! with no descent (D503) as any other.

use super::*;
use crate::disk::memory::MemoryFileSystem;
use crate::memory::MemoryBudget;

/// An empty tree of `page_size` pages, in memory.
fn sized(page_size: u32) -> BLinkTree {
    let hash = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Pager::create(&MemoryFileSystem::new(), std::path::Path::new("/s"), hash, Pipeline::none(), page_size).unwrap();
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    BLinkTree::create(pool, file).unwrap()
}

/// A key `len` bytes long: `shared` bytes `fill`, then `n`'s bytes, then
/// bytes 's' to its length.
fn long(len: usize, shared: usize, fill: u8, n: u64) -> Vec<u8> {
    let mut key = vec![fill; shared];
    key.extend_from_slice(&n.to_be_bytes());
    key.resize(len, b's');
    key
}

/// Every leaf of `tree`, left to right: its page and its length.
fn leaves(tree: &BLinkTree) -> Vec<(PageId, usize)> {
    let op = tree.epochs.guard();
    let mut id = Some(tree.descend((op.within(), Target::Key(&[])), 0, &mut Path::default(), |_| Ok(())).unwrap().0);
    let mut found = Vec::new();
    while let Some(leaf) = id {
        let page = tree.pool.read(tree.file, leaf.0).unwrap();
        found.push((leaf, page.len()));
        id = Node::parse(&page).unwrap().right;
    }
    found
}

/// A key `len` bytes long that shares all but the last 9 of `shared`
/// bytes 's', then `last`.
fn late(len: usize, shared: usize, last: u8) -> Vec<u8> {
    let mut key = vec![b's'; shared.saturating_sub(1)];
    key.push(last);
    key.resize(len, b'x');
    key
}

/// Verifies: [DESIGN] RULES.md D500, D31: of 4 KiB and 64 KiB pages, leaves
/// full of keys of the longest length that share all but their last 8
/// bytes, each split by a key of the longest length that shares none or
/// all but 9 of their bytes, below them and above them, become leaves that
/// each fit the page; every key is found, and the tree holds them in
/// order.
#[test]
fn keys_without_a_long_shared_prefix_split_leaves_into_nodes_that_fit() {
    for page_size in [4_096_u32, 65_536] {
        let tree = sized(page_size);
        let len = tree.max_key();
        let value = [7_u8; 8];
        let shared = len.saturating_sub(8);
        let mut want: Vec<Vec<u8>> = (1..=300_u64).map(|n| long(len, shared, b's', n)).collect();
        for key in &want {
            assert!(tree.insert(key, &value).unwrap());
        }
        for key in [long(len, 1, b'a', 1), late(len, shared, b'r'), late(len, shared, b't'), long(len, 1, b'z', 1)] {
            assert!(tree.insert(&key, &value).unwrap(), "{page_size}");
            want.push(key);
        }
        want.sort();
        for key in &want {
            assert_eq!(tree.get(key).unwrap(), Some(value.to_vec()), "{page_size}");
        }
        for (leaf, bytes) in leaves(&tree) {
            assert!(bytes <= tree.capacity, "leaf {} of {bytes} bytes, past the page's {}", leaf.0, tree.capacity);
        }
        assert_eq!(keys(&tree, Direction::Forward), want, "{page_size}");
    }
}

/// Verifies: [DESIGN] RULES.md D500: a leaf full of keys that share 800
/// bytes, split by a key that shares none of them, gives halves that each
/// fit the page, the keys that share the prefix together and the other
/// alone, with no third node.
#[test]
fn a_key_without_the_shared_prefix_splits_a_leaf_into_halves_that_fit() {
    let (tree, _scratch) = tree("shared-prefix");
    let mut want: Vec<Vec<u8>> = (1..=20_u64).map(|n| long(808, 800, b's', n)).collect();
    for key in &want {
        assert!(tree.insert(key, &VALUE).unwrap());
    }
    assert_eq!(tree.height().unwrap(), 1, "one leaf holds them, their prefix kept once");
    let key = long(808, 800, b'a', 1);
    assert!(tree.insert(&key, &VALUE).unwrap());
    want.insert(0, key);
    assert_eq!(tree.counts().splits, 1);
    assert_eq!(leaves(&tree).len(), 2);
    for (_, bytes) in leaves(&tree) {
        assert!(bytes <= tree.capacity);
    }
    assert_eq!(keys(&tree, Direction::Forward), want);
}

/// Verifies: [DESIGN] RULES.md D500, D31, `fit.rs`: a node of three pages'
/// worth of the longest keys, sharing little, is split again and again
/// until each part fits its page, each linked to the next, its keys in
/// order; and each split is returned to be posted.
#[test]
fn a_node_past_its_page_is_split_until_each_part_fits() {
    let tree = sized(4_096);
    let root = PageId(tree.root.load(Ordering::Acquire));
    let want: Vec<Vec<u8>> = (0..12_u64).map(|n| long(tree.max_key(), 1, b'k', n)).collect();
    let value = [7_u8; 8];
    let entries: Vec<(&[u8], &[u8])> = want.iter().map(|k| (k.as_slice(), value.as_slice())).collect();
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: tree.meta.0 };
    let content = node::build(&shape, &entries).unwrap();
    assert!(content.len() > 2 * tree.capacity);
    let mut splits = Vec::new();
    let fitted = tree.fitted(content, &mut splits).unwrap();
    *tree.pool.write(tree.file, root).unwrap() = fitted;
    let parts = leaves(&tree);
    assert_eq!(parts.len(), splits.len() + 1);
    assert!(parts.len() >= 3, "{parts:?}");
    assert!(parts.iter().all(|&(_, bytes)| bytes <= tree.capacity), "{parts:?}");
    assert_eq!(keys(&tree, Direction::Forward), want);
}

/// Verifies: [DESIGN] RULES.md D503, D500, `fit.rs`: of 4 KiB and 64 KiB
/// pages, keys of the longest length that share all but their last 8
/// bytes, written in order, go to the rightmost leaf with no descent, as
/// the index of a long character column's values does; keys past them
/// that share all but 9 of their bytes or none go there with no descent
/// too, and split it into leaves that each fit the page; every key is
/// found, and the tree holds them in order.
#[test]
fn keys_past_the_end_without_the_shared_prefix_split_the_rightmost_leaf_into_nodes_that_fit() {
    for page_size in [4_096_u32, 65_536] {
        let tree = sized(page_size);
        let len = tree.max_key();
        let value = [7_u8; 8];
        let shared = len.saturating_sub(8);
        let mut want: Vec<Vec<u8>> = (1..=300_u64).map(|n| long(len, shared, b's', n)).collect();
        for key in &want {
            assert!(tree.insert(key, &value).unwrap());
        }
        let before = tree.counts();
        assert!(before.edge_writes >= 298, "{page_size}: {before:?}");
        for key in [late(len, shared, b't'), long(len, 1, b'z', 1), long(len, 1, b'z', 2)] {
            let edge = tree.counts().edge_writes;
            assert!(tree.insert(&key, &value).unwrap(), "{page_size}");
            assert_eq!(tree.counts().edge_writes, edge + 1, "{page_size}: written with no descent");
            want.push(key);
        }
        assert!(tree.counts().splits > before.splits, "{page_size}");
        for key in &want {
            assert_eq!(tree.get(key).unwrap(), Some(value.to_vec()), "{page_size}");
        }
        for (leaf, bytes) in leaves(&tree) {
            assert!(bytes <= tree.capacity, "leaf {} of {bytes} bytes, past the page's {}", leaf.0, tree.capacity);
        }
        assert_eq!(keys(&tree, Direction::Forward), want, "{page_size}");
    }
}

/// An empty tree of 4 KiB pages in memory, its file's pages reserved of
/// the budget given with it.
fn budgeted() -> (BLinkTree, Arc<MemoryBudget>) {
    let budget = MemoryBudget::new(1 << 40);
    let fs = MemoryFileSystem::within(Arc::clone(&budget), "tree test pages");
    let hash = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Pager::create(&fs, std::path::Path::new("/s"), hash, Pipeline::none(), 4_096).unwrap();
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    (BLinkTree::create(pool, file).unwrap(), budget)
}

/// Verifies: [DESIGN] RULES.md D1271, `fit.rs`: a split whose halves each
/// need a page more, of a node of twelve of the longest keys sharing
/// little, given room in its file for no page, one, two and so on until
/// it has enough, fails each time it is refused one with SQLCODE -918,
/// gives back every page it took, so its file has as many pages in use
/// as before, and leaves the node as it was; with room enough it makes
/// the nodes, each fitting its page, and returns each split. A regression
/// test: a split refused its second page or a later one left those before
/// it allocated and unlinked, the first holding a leaf's content.
#[test]
fn a_split_refused_a_page_gives_back_those_it_took() {
    let value = [7_u8; 8];
    let mut refused = Vec::new();
    for room in 0..8_usize {
        let (tree, budget) = budgeted();
        let root = PageId(tree.root.load(Ordering::Acquire));
        let keys: Vec<Vec<u8>> = (0..13_u64).map(|n| long(tree.max_key(), 1, b'k', n)).collect();
        let entries: Vec<(&[u8], &[u8])> = keys.iter().take(12).map(|k| (k.as_slice(), value.as_slice())).collect();
        let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: tree.meta.0 };
        let content = node::build(&shape, &entries).unwrap();
        let before = tree.pool.read(tree.file, root.0).unwrap().to_vec();
        let in_use = tree.pool.pages_in_use(tree.file).unwrap();
        budget.set_limit(budget.reserved() + room * 4_096);
        match tree.split(root, &content, (&keys[12], &value), Slot::Insert(12)) {
            Err(e) => {
                assert_eq!(e.sqlcode(), -918_i32, "room for {room}: {e}");
                assert_eq!(tree.pool.pages_in_use(tree.file).unwrap(), in_use, "room for {room}: every page taken is given back");
                assert_eq!(tree.pool.read(tree.file, root.0).unwrap().to_vec(), before, "room for {room}: the node as it was");
                refused.push(room);
            }
            Ok(splits) => {
                assert_eq!(leaves(&tree).len(), splits.len() + 1, "room for {room}");
                assert!(leaves(&tree).iter().all(|&(_, bytes)| bytes <= tree.capacity));
                assert_eq!(super::keys(&tree, Direction::Forward), keys, "room for {room}");
                assert_eq!(usize::try_from(tree.pool.pages_in_use(tree.file).unwrap() - in_use).unwrap(), splits.len());
                break;
            }
        }
    }
    assert!(refused.len() >= 3 && refused.first() == Some(&0), "refused its first page, its second and a later one: {refused:?}");
}
