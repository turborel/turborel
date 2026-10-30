//! Unit tests of the B-link tree's concurrent paths, made deterministic: a
//! writer whose path was read before the tree changed, and a reader that
//! began at an old root, are set up on one thread, as the interleavings
//! that make them would leave them (RULES.md R-TEST-1, R-TEST-5); so are a
//! writer, a deleter and a scan that meet a leaf taken out. And where runs
//! of keys split, bulk loading, the longest entry, and malformed trees
//! refused (R-TEST-2 robustness). Split from tree.rs (RULES.md R-SIZE-2).

use std::ops::Bound;

use turborel_test_support::Scratch;

use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::io::OsFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};

/// An empty tree in a new pager file of 4 KiB pages, in a pool holding it.
fn tree(test: &str) -> (BLinkTree, Scratch) {
    let scratch = Scratch::new(&format!("unit-{test}"));
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("tree"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(2048, Box::new(Clock::new(2048))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    (BLinkTree::create(pool, file).unwrap(), scratch)
}

fn key(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}

const VALUE: [u8; 92] = [7; 92];

/// Fills `tree` with keys `1..=n` in order, so every leaf but the last is
/// full.
fn fill(tree: &BLinkTree, n: u64) {
    for i in 1..=n {
        assert!(tree.insert(&key(i), &VALUE).unwrap());
    }
}

fn keys(tree: &BLinkTree, direction: Direction) -> Vec<Vec<u8>> {
    tree.scan(Bound::Unbounded, Bound::Unbounded, direction).map(|e| e.unwrap().0).collect()
}

/// Verifies: [DESIGN] RULES.md D31, B-link insert (Lehman and Yao §5,
/// §6.3 Part 3): a writer that read its path while the tree was one leaf,
/// before it grew to three levels, splits that leaf, finds no remembered
/// parent and a root above the leaf's level, and finds the parent again
/// from the root, without growing the tree; a writer and a deleter given a
/// leaf that has split many times since move right to the key's leaf.
#[test]
fn stale_paths_move_right_and_find_their_parent_again() {
    let (tree, _scratch) = tree("stale-paths");
    let mut path = Path::default();
    let (first, ()) = tree.descend(Target::Key(&key(0)), 0, &mut path, |_| Ok(())).unwrap();
    assert_eq!(path.stack, [], "the tree was one leaf");
    let n = 20_000;
    fill(&tree, n);
    assert_eq!(tree.height().unwrap(), 3);
    let before = tree.counts();
    assert!(matches!(tree.write_from(path, first, (&key(0), &VALUE), Mode::Insert).unwrap(), Written::Done(None)));
    let after = tree.counts();
    assert!(after.splits > before.splits && after.root_splits == before.root_splits, "the full leaf split; no new root");
    assert_eq!(tree.get(&key(0)).unwrap(), Some(VALUE.to_vec()));
    let mut expected: Vec<Vec<u8>> = (0..=n).map(|i| key(i).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), expected);
    assert_eq!(tree.delete_from(&Path::default(), first, &key(19_000), None).unwrap(), Some(VALUE.to_vec()));
    assert!(tree.counts().right_moves > after.right_moves + 400, "moved right across the leaves");
    assert_eq!(tree.get(&key(19_000)).unwrap(), None);
    assert!(matches!(tree.write_from(Path::default(), first, (&key(19_000), b"back"), Mode::Insert).unwrap(), Written::Done(None)));
    assert_eq!(tree.get(&key(19_000)).unwrap(), Some(b"back".to_vec()));
    expected.reverse();
    assert_eq!(keys(&tree, Direction::Backward), expected);
}

/// Verifies: [DESIGN] RULES.md D31, B-link search (Lehman and Yao §6.3
/// Part 1): a reader that began at the root of a one-leaf tree, which then
/// grew to three levels, still finds every key, following right links from
/// the old root, the leftmost leaf; and scans from it in both directions
/// return every key in order.
#[test]
fn an_old_root_still_finds_every_key() {
    let (tree, _scratch) = tree("old-root");
    let old = tree.root.load(Ordering::Acquire);
    let n = 20_000;
    fill(&tree, n);
    let current = tree.root.swap(old, Ordering::AcqRel);
    for i in [1, 38, 39, 10_000, n] {
        assert_eq!(tree.get(&key(i)).unwrap(), Some(VALUE.to_vec()), "key {i}");
    }
    assert_eq!(tree.get(&key(n + 1)).unwrap(), None);
    let mut expected: Vec<Vec<u8>> = (1..=n).map(|i| key(i).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), expected);
    expected.reverse();
    assert_eq!(keys(&tree, Direction::Backward), expected);
    tree.root.store(current, Ordering::Release);
}

/// Verifies: [DESIGN] RULES.md D31, B-link robustness: a child at another
/// level than its parent says, and a root that is not a node, are refused
/// with SQLCODE -916; a node too full to split (a high key longer than its
/// page, no entries) is refused with SQLCODE -916, and the page taken for
/// its right half is given back.
#[test]
fn malformed_trees_are_refused() {
    let (tree, _scratch) = tree("malformed");
    fill(&tree, 100);
    let root = PageId(tree.root.load(Ordering::Acquire));
    let leaf = Node::parse(&tree.pool.read(tree.file, root.0).unwrap()).unwrap().first_child();
    *tree.pool.write(tree.file, leaf).unwrap() = node::build(&Shape { level: 1, high: None, right: None, first: leaf }, &[]).unwrap();
    assert_eq!(tree.get(&key(1)).err().map(|e| e.code), Some(Code::Corrupt), "a leaf at level 1");
    *tree.pool.write(tree.file, root).unwrap() = b"not a node".to_vec();
    assert_eq!(tree.get(&key(1)).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(tree.height().err().map(|e| e.code), Some(Code::Corrupt));

    let (small, _other) = self::tree("too-full");
    let small_root = PageId(small.root.load(Ordering::Acquire));
    let high = [0xFF; 5000];
    *small.pool.write(small.file, small_root).unwrap() =
        node::build(&Shape { level: 0, high: Some(&high), right: Some(small_root), first: PageId(0) }, &[]).unwrap();
    assert!(high.len() > small.pool.capacity(small.file).unwrap());
    assert_eq!(small.insert(b"a", b"1").err().map(|e| e.code), Some(Code::Corrupt));
}

/// The leaves of a tree into which `runs` runs of keys are inserted taking
/// turns, one key each, every run taking blocks of 4,096 keys from one
/// counter as threads take row ids (`rows/ids.rs`); or with `scattered`,
/// the same keys in a scattered order.
fn leaves_after(runs: u64, per_run: u64, scattered: bool) -> usize {
    let (tree, _scratch) = tree(&format!("fill-{runs}-{scattered}"));
    let value = [5_u8; 40];
    let mut next = 0_u64;
    let mut blocks = vec![(0_u64, 0_u64); usize::try_from(runs).unwrap()];
    let mut keys = Vec::new();
    for _ in 0..per_run {
        for block in &mut blocks {
            if block.0 >= block.1 {
                *block = (next, next.saturating_add(4096));
                next = next.saturating_add(4096);
            }
            keys.push(block.0);
            block.0 = block.0.saturating_add(1);
        }
    }
    if scattered {
        let n = keys.len();
        keys = (0..n).map(|i| keys[i.saturating_mul(7919).rem_euclid(n)]).collect();
    }
    for k in keys {
        assert!(tree.insert(&key(k), &value).unwrap());
    }
    let mut path = Path::default();
    let (mut id, ()) = tree.descend(Target::First, 0, &mut path, |_| Ok(())).unwrap();
    let mut leaves = 1_usize;
    while let Some(right) = Node::parse(&tree.pool.read(tree.file, id.0).unwrap()).unwrap().right {
        id = right;
        leaves = leaves.saturating_add(1);
    }
    leaves
}

/// Verifies: [DESIGN] RULES.md D31, where nodes split (`node.rs`, an
/// extension of Lehman and Yao's even split, §3.1.3(b)): runs of keys
/// inserted in order fill their leaves though they take turns, each run's
/// first key past the end of the rightmost leaf starting a leaf of its own:
/// two runs take no more than 2% more leaves than one run's keys alone,
/// four no more than 20%; the same keys scattered still split in the
/// middle, taking at least 20% more.
#[test]
fn runs_of_keys_fill_their_leaves() {
    let one = leaves_after(1, 8000, false);
    let two = leaves_after(2, 4000, false);
    let four = leaves_after(4, 2000, false);
    assert!(two * 100 <= one * 102, "{two} leaves for two runs, {one} for one");
    assert!(four * 100 <= one * 120, "{four} leaves for four runs, {one} for one");
    let scattered = leaves_after(1, 8000, true);
    assert!(scattered * 100 >= one * 120, "{scattered} leaves scattered, {one} in order");
}

/// Verifies: [DESIGN] RULES.md D31, bulk loading: entries given in key
/// order make a tree of full leaves and the levels above, which reads
/// back every entry, in order, and takes inserts after; a tree not empty,
/// or entries out of order, are refused; none at all leave it empty.
#[test]
fn a_tree_is_loaded_from_entries_in_order() {
    let (loaded, _scratch) = tree("load");
    let entries = (1..=5_000_u64).map(|i| Ok((key(i * 2).to_vec(), VALUE.to_vec())));
    loaded.load(&mut entries.into_iter()).unwrap();
    assert!(loaded.height().unwrap() > 1);
    let keys: Vec<Vec<u8>> = loaded.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).map(|e| e.unwrap().0).collect();
    assert_eq!(keys, (1..=5_000_u64).map(|i| key(i * 2).to_vec()).collect::<Vec<_>>());
    assert!(loaded.insert(&key(3), &VALUE).unwrap(), "an insert after");
    assert_eq!(loaded.get(&key(3)).unwrap().as_deref(), Some(&VALUE[..]));
    assert_eq!(loaded.get(&key(10_000)).unwrap().as_deref(), Some(&VALUE[..]));
    let refused = loaded.load(&mut std::iter::once(Ok((key(1).to_vec(), Vec::new()))));
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Api), "not empty");
    let (unordered, _other) = tree("load-unordered");
    let backwards = [Ok((key(2).to_vec(), Vec::new())), Ok((key(1).to_vec(), Vec::new()))];
    assert_eq!(unordered.load(&mut backwards.into_iter()).err().map(|e| e.code), Some(Code::Api));
    let (empty, _third) = tree("load-empty");
    empty.load(&mut std::iter::empty()).unwrap();
    assert_eq!(empty.height().unwrap(), 1);
}

/// Verifies: [DESIGN] RULES.md D31, taking out emptied leaves (Lanin and
/// Shasha 1986), its stale paths made on one thread: a writer and a
/// deleter given a leaf taken out since follow its link to the leaf that
/// took its range; a scan whose next leaf was taken out goes on from that
/// leaf, returning nothing twice; the page taken out is freed only once no
/// operation that began before can hold it.
#[test]
fn stale_paths_follow_a_leaf_taken_out() {
    let (tree, _scratch) = tree("unlink-stale");
    fill(&tree, 400);
    // A leaf in the middle, and a scan stopped just before it.
    let mut path = Path::default();
    let (leaf, low) = tree.descend(Target::Key(&key(200)), 0, &mut path, |n| Ok(n.key(0)?.to_vec())).unwrap();
    let first_of_leaf = u64::from_be_bytes(<[u8; 8]>::try_from(low.as_slice()).unwrap());
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for entry in scan.by_ref() {
        let found = entry.unwrap().0;
        let last = found == key(first_of_leaf - 1);
        seen.push(found);
        if last {
            break;
        }
    }
    let held = tree.epochs.guard();
    // Empty the leaf: it is taken out, its page kept while `held` lives.
    let in_leaf: Vec<u64> = tree
        .scan(Bound::Included(&key(first_of_leaf)), Bound::Unbounded, Direction::Forward)
        .map(|e| u64::from_be_bytes(<[u8; 8]>::try_from(e.unwrap().0.as_slice()).unwrap()))
        .take_while(|&k| tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0 == leaf)
        .collect();
    for k in &in_leaf {
        assert!(tree.delete(&key(*k)).unwrap().is_some());
    }
    assert_eq!(tree.counts().unlinked, 1);
    let page = tree.pool.read(tree.file, leaf.0).unwrap();
    assert!(Node::parse(&page).unwrap().deleted, "kept, marked, while an operation may hold it");
    drop(page);
    let moved = in_leaf[0];
    assert!(matches!(tree.write_from(path, leaf, (&key(moved), &VALUE), Mode::Insert).unwrap(), Written::Done(None)));
    assert_eq!(tree.get(&key(moved)).unwrap(), Some(VALUE.to_vec()));
    assert_eq!(tree.delete_from(&Path::default(), leaf, &key(moved), None).unwrap(), Some(VALUE.to_vec()));
    seen.extend(scan.map(|e| e.unwrap().0));
    let left: Vec<Vec<u8>> = (1..=400_u64).filter(|k| !in_leaf.contains(k)).map(|k| key(k).to_vec()).collect();
    assert_eq!(seen, left, "the scan went on past the leaf taken out");
    let early: Vec<PageId> = (0..3_u8).flat_map(|_| tree.epochs.freeable()).collect();
    assert!(!early.contains(&leaf), "not while an operation that began before may hold it");
    drop(held);
    let freed: Vec<PageId> = (0..3_u8).flat_map(|_| tree.epochs.freeable()).collect();
    assert_eq!(freed, [leaf], "once none may");
}

/// Verifies: [DESIGN] CONFORMANCE.md, Limits: the longest entry a B-link
/// tree holds is 996 bytes at 4 KiB pages, 2,020 at 8 KiB and 16,356 at
/// 64 KiB, with CRC-32C page checksums.
#[test]
fn the_longest_entry_is_as_documented() {
    use crate::disk::memory::MemoryFileSystem;
    for (page_size, most) in [(4_096_u32, 996_usize), (8_192, 2_020), (65_536, 16_356)] {
        let pager = Pager::create(
            &MemoryFileSystem::new(),
            std::path::Path::new("/t"),
            Arc::new(Crc::new(CRC32_ISCSI).unwrap()),
            Pipeline::none(),
            page_size,
        )
        .unwrap();
        let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
        let file = pool.add_pager(Arc::new(pager));
        assert_eq!(BLinkTree::create(pool, file).unwrap().max_entry(), most);
    }
}
