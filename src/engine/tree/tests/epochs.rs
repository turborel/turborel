//! The epoch of a B-link tree (RULES.md D31, D1270): a leaf taken out
//! stays, marked and linked, while an operation that may hold its number
//! runs, and is freed once none does. The schedules are fixed, each made
//! on one thread, the other threads' work done between two steps of the
//! operation watched.

use super::*;
use crate::engine::tree::reclaim::Epochs;

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
    let reached = tree.descend((tree.epochs.guard().within(), Target::Key(&key(200))), 0, &mut path, |n| Ok(n.key(0)?.to_vec()));
    let (leaf, low) = reached.unwrap();
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
        .take_while(|&k| leaf_of(&tree, k) == leaf)
        .collect();
    for k in &in_leaf {
        assert!(tree.delete(&key(*k)).unwrap().is_some());
    }
    assert_eq!(tree.counts().unlinked, 1);
    let page = tree.pool.read(tree.file, leaf.0).unwrap();
    assert!(Node::parse(&page).unwrap().deleted, "kept, marked, while an operation may hold it");
    drop(page);
    let moved = in_leaf[0];
    assert!(matches!(tree.write_from((held.within(), &path), leaf, (&key(moved), &VALUE), Mode::Insert).unwrap(), Written::Done(None, _)));
    assert_eq!(tree.get(&key(moved)).unwrap(), Some(VALUE.to_vec()));
    assert_eq!(tree.delete_from((held.within(), &Path::default()), leaf, &key(moved), None).unwrap(), Some(VALUE.to_vec()));
    seen.extend(scan.map(|e| e.unwrap().0));
    let left: Vec<Vec<u8>> = (1..=400_u64).filter(|k| !in_leaf.contains(k)).map(|k| key(k).to_vec()).collect();
    assert_eq!(seen, left, "the scan went on past the leaf taken out");
    let early: Vec<PageId> = (0..3_u8).flat_map(|_| tree.epochs.freeable()).collect();
    assert!(!early.contains(&leaf), "not while an operation that began before may hold it");
    drop(held);
    let freed: Vec<PageId> = (0..3_u8).flat_map(|_| tree.epochs.freeable()).collect();
    assert_eq!(freed, [leaf], "once none may");
}

/// Verifies: [DESIGN] RULES.md D1270, D31: a writer that has read its
/// leaf's number from the parent, the leaf not yet read, while other
/// operations empty that leaf, take it out and move the epoch on as far
/// as it goes, finds the page kept, marked deleted and linked right, and
/// its delete and its insert go where the range went; once the writer's
/// operation ends, the page is freed and the pool refuses a read of it.
/// The writer began in the epoch the page was retired in, an older
/// operation ending meanwhile, so the page waits two epochs, not one. A
/// regression test: the stress test made this descent outside the epoch
/// and met the refusal once in thousands of runs, which no longer
/// compiles.
#[test]
fn a_leaf_taken_out_under_a_writer_stopped_above_it_is_kept_and_leads_right() {
    let (tree, _scratch) = tree("epoch-stopped-above");
    fill(&tree, 400);
    let (keys, sparse) = (in_leaf(&tree, 400, 200), in_leaf(&tree, 400, 1));
    let (most, rest) = sparse.split_at(sparse.len() - 4);
    // An operation begun in the epoch before the writer's.
    let older = tree.epochs.guard();
    assert!(tree.epochs.freeable().is_empty(), "nothing is taken out yet: the epoch moves on");
    let op = tree.epochs.guard();
    let mut path = Path::default();
    let leaf = tree.leaf_for_write(op.within(), &key(200), &mut path).unwrap();
    for k in &keys {
        assert!(tree.delete(&key(*k)).unwrap().is_some());
    }
    assert_eq!(tree.counts().unlinked, 1, "taken out");
    drop(older);
    // Deletes that leave the first leaf sparse: each frees what it may.
    for k in most {
        assert!(tree.delete(&key(*k)).unwrap().is_some());
    }
    {
        let page = tree.page(op.within(), leaf).unwrap();
        let node = Node::parse(&page).unwrap();
        assert!(node.deleted && node.right.is_some(), "kept, marked and linked, under the writer");
    }
    assert_eq!(tree.delete_from((op.within(), &path), leaf, &key(200), None).unwrap(), None, "led right: the key is gone");
    assert!(matches!(tree.write_from((op.within(), &path), leaf, (&key(200), &VALUE), Mode::Insert).unwrap(), Written::Done(None, _)));
    assert_eq!(tree.get(&key(200)).unwrap(), Some(VALUE.to_vec()));
    drop(op);
    for k in rest {
        assert!(tree.delete(&key(*k)).unwrap().is_some());
    }
    let refused = tree.pool.read(tree.file, leaf.0).map(drop).unwrap_err();
    assert!(refused.to_string().contains("is not allocated"), "freed once no operation may hold it: {refused}");
}

/// Verifies: [DESIGN] RULES.md D31, D1270, the epochs alone: a page
/// retired is kept while an operation that began in its epoch runs, the
/// operations of the epoch before having ended, and while one that began
/// in the epoch before runs; it is freed once both have ended; and a page
/// retired before an operation began is freed under that operation, so a
/// long one holds back only what it may hold.
#[test]
fn a_page_retired_waits_for_the_operations_of_its_epoch_and_the_one_before() {
    let freed = |epochs: &Epochs| -> Vec<u64> { (0..4_u8).flat_map(|_| epochs.freeable()).map(|page| page.0).collect() };
    // In its own epoch: an older operation holds the epoch there, and ends.
    let own = Epochs::new();
    let older = own.enter();
    assert!(own.freeable().is_empty(), "the epoch moves on once");
    let same = own.enter();
    own.retire(PageId(7));
    own.leave(older);
    assert!(freed(&own).is_empty(), "an operation of its epoch runs");
    own.leave(same);
    assert_eq!(freed(&own), [7], "none runs");
    // In the epoch before: the epoch moves on once, the page is retired.
    let earlier = Epochs::new();
    let before = earlier.enter();
    assert!(earlier.freeable().is_empty(), "the epoch moves on once");
    earlier.retire(PageId(8));
    assert!(freed(&earlier).is_empty(), "an operation of the epoch before runs");
    earlier.leave(before);
    assert_eq!(freed(&earlier), [8]);
    // Retired before an operation began: freed under it.
    let later_on = Epochs::new();
    later_on.retire(PageId(9));
    assert!(later_on.freeable().is_empty(), "one epoch on");
    let later = later_on.enter();
    assert_eq!(freed(&later_on), [9], "the operation began after it was taken out");
    later_on.leave(later);
}
