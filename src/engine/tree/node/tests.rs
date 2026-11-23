//! Unit tests of the B-link node format: nodes read back as built,
//! searched in key order, changed in place, split as `split` says, where
//! runs split, and malformed nodes refused (RULES.md R-TEST-1, R-TEST-2).

use super::*;

mod damage;
mod layout;
mod lean;
mod place;
mod search;

/// The tree the nodes of these tests are of.
const TREE: u64 = 77;

/// `entries` as keys of a node, whole.
fn refs<'a>(entries: &[(&'a [u8], &'a [u8])]) -> Vec<(KeyRef<'a>, &'a [u8])> {
    entries.iter().map(|&(k, v)| (KeyRef::whole(k), v)).collect()
}

/// Entry `i` of `node`, its key whole.
fn entry_of(node: &Node<'_>, i: usize) -> (Vec<u8>, Vec<u8>) {
    let (key, value) = node.entry(i).unwrap();
    (key.to_vec(), value.to_vec())
}

/// The child of `node` for `key`, and the key below its range, whole.
fn child_for(node: &Node<'_>, key: &[u8]) -> (PageId, Option<Vec<u8>>) {
    let (child, below) = node.child(key).unwrap();
    (child, below.map(KeyRef::to_vec))
}

fn leaf(high: Option<&[u8]>, right: Option<PageId>, entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    build(&Shape { level: 0, high, right, first: PageId(0), tree: TREE }, entries).unwrap()
}

fn keys(content: &[u8]) -> Vec<Vec<u8>> {
    let node = Node::parse(content).unwrap();
    (0..node.len()).map(|i| node.key(i).unwrap().to_vec()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, B-link node: a node reads back as
/// built, its entries found by binary search (`Ok` where present, `Err`
/// where they would go), its range bounded by its high key; inserting
/// and removing in place keep key order, removed bytes counted as
/// garbage until the node is compacted.
#[test]
fn nodes_read_back_and_change_in_place() {
    let mut content = leaf(Some(b"m"), Some(PageId(9)), &[(b"b", b"2"), (b"d", b"4")]);
    let node = Node::parse(&content).unwrap();
    assert_eq!((node.level, node.high, node.right, node.len()), (0, Some(&b"m"[..]), Some(PageId(9)), 2));
    assert_eq!(node.search(b"d").unwrap(), Ok(1));
    assert_eq!((node.search(b"a").unwrap(), node.search(b"c").unwrap(), node.search(b"z").unwrap()), (Err(0), Err(1), Err(2)));
    assert!(node.covers(b"m") && !node.covers(b"n"));
    insert(&mut content, 1, b"c", b"3").unwrap();
    insert(&mut content, 0, b"a", b"").unwrap();
    insert(&mut content, 4, b"e", b"5").unwrap();
    assert_eq!(keys(&content), [b"a", b"b", b"c", b"d", b"e"]);
    assert_eq!(entry_of(&Node::parse(&content).unwrap(), 2), (b"c".to_vec(), b"3".to_vec()));
    let before = content.len();
    remove(&mut content, 2).unwrap();
    let removed = Node::parse(&content).unwrap();
    assert_eq!((removed.live(), content.len()), (before - entry_size(b"c", b"3"), before - SLOT));
    assert_eq!(keys(&content), [b"a", b"b", b"d", b"e"]);
    let compacted = compact_in(&content, 0).unwrap();
    assert_eq!((keys(&compacted), compacted.len()), (keys(&content), removed.live()));
    assert!(insert(&mut content, 9, b"z", b"").is_err(), "past its entries");
    assert!(remove(&mut content, 4).is_err(), "no entry 4");
}

/// Verifies: [DESIGN] RULES.md D31, B-link node: an inner node sends a
/// key to the first child up to the first entry's key, a key equal to
/// an entry's to the child on its left, and keys above it to its child;
/// the rightmost node of a level covers every key.
#[test]
fn inner_nodes_route_keys_to_children() {
    let (c5, c7) = (PageId(5).0.to_le_bytes(), PageId(7).0.to_le_bytes());
    let content = build(&Shape { level: 1, high: None, right: None, first: PageId(3), tree: TREE }, &[(b"f", &c5), (b"p", &c7)]).unwrap();
    let node = Node::parse(&content).unwrap();
    assert_eq!(child_for(&node, b"a"), (PageId(3), None));
    assert_eq!(child_for(&node, b"f"), (PageId(3), None));
    assert_eq!(child_for(&node, b"g"), (PageId(5), Some(b"f".to_vec())));
    assert_eq!(child_for(&node, b"zz"), (PageId(7), Some(b"p".to_vec())));
    assert_eq!((node.first_child(), node.child_at(node.len()).unwrap().0), (PageId(3), PageId(7)));
    assert!(node.covers(b"\xff\xff"));
}

/// Verifies: [DESIGN] RULES.md D31, B-link split (Lehman and Yao §4): a
/// leaf splits evenly by bytes, the left half keeping the separator as
/// its high key and linking to the new page, the right half taking the
/// old high key and link; appended past the rightmost leaf's end, the
/// right half holds only the new entry. An inner node gives its middle
/// key up as the separator, its child becoming the right half's first.
#[test]
fn splits_halve_and_link() {
    let entries: Vec<(&[u8], &[u8])> = vec![(b"a", b"1"), (b"b", b"2"), (b"c", b"3"), (b"d", b"4")];
    let shape = Shape { level: 0, high: Some(b"k"), right: Some(PageId(8)), first: PageId(0), tree: TREE };
    let halves = split(&shape, &refs(&entries), None, (PageId(4), usize::MAX)).unwrap();
    let (left, right) = (Node::parse(&halves.left).unwrap(), Node::parse(&halves.right).unwrap());
    assert_eq!((keys(&halves.left), left.high, left.right), (vec![b"a".to_vec(), b"b".to_vec()], Some(&b"b"[..]), Some(PageId(4))));
    assert_eq!((keys(&halves.right), right.high, right.right), (vec![b"c".to_vec(), b"d".to_vec()], Some(&b"k"[..]), Some(PageId(8))));
    assert_eq!(halves.separator, b"b");
    let rightmost = Shape { high: None, right: None, ..shape };
    let appended = split(&rightmost, &refs(&entries), Some(3), (PageId(4), usize::MAX)).unwrap();
    assert_eq!((keys(&appended.right), appended.separator), (vec![b"d".to_vec()], b"c".to_vec()));
    let children: Vec<[u8; 8]> = (10..14_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let split_inner = split(&Shape { level: 1, first: PageId(9), ..rightmost }, &refs(&inner), None, (PageId(4), usize::MAX)).unwrap();
    let (inner_left, inner_right) = (Node::parse(&split_inner.left).unwrap(), Node::parse(&split_inner.right).unwrap());
    assert_eq!((split_inner.separator.as_slice(), inner_left.first_child(), inner_left.len()), (&b"b"[..], PageId(9), 1));
    assert_eq!((inner_right.first_child(), keys(&split_inner.right)), (PageId(11), vec![b"c".to_vec(), b"d".to_vec()]));
    assert!(split(&shape, &refs(&entries[..1]), None, (PageId(4), usize::MAX)).is_err(), "one entry cannot split");
}

/// Verifies: [DESIGN] RULES.md D31, where nodes split: an insert marks
/// its entry as the last, which compaction keeps; a node whose inserts
/// run in order splits where the new entry goes, the entries before it
/// staying and the new one starting the right half, marked there; an
/// inner node gives up the entry before the new one, whose child starts
/// the right half; without a run, a node splits in the middle and marks
/// nothing.
#[test]
fn runs_split_where_the_entry_goes() {
    let mut content = leaf(None, None, &[(b"a", b"1"), (b"c", b"3")]);
    assert_eq!(Node::parse(&content).unwrap().last_insert().unwrap(), None, "a node built marks nothing");
    insert(&mut content, 1, b"b", b"2").unwrap();
    assert_eq!(Node::parse(&content).unwrap().last_insert().unwrap(), Some(1));
    remove(&mut content, 0).unwrap();
    let compacted = compact_in(&content, 0).unwrap();
    assert_eq!(Node::parse(&compacted).unwrap().last_insert().unwrap(), Some(0), "compaction keeps the mark");
    let entries: Vec<(&[u8], &[u8])> = vec![(b"a", b"1"), (b"b", b"2"), (b"c", b"3"), (b"x", b"4"), (b"y", b"5")];
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: TREE };
    let halves = split(&shape, &refs(&entries), Some(2), (PageId(4), usize::MAX)).unwrap();
    let right = Node::parse(&halves.right).unwrap();
    assert_eq!(
        (keys(&halves.left), keys(&halves.right)),
        (vec![b"a".to_vec(), b"b".to_vec()], vec![b"c".to_vec(), b"x".to_vec(), b"y".to_vec()])
    );
    assert_eq!((halves.separator, right.last_insert().unwrap()), (b"b".to_vec(), Some(0)));
    let middle = split(&shape, &refs(&entries), None, (PageId(4), usize::MAX)).unwrap();
    assert_eq!(Node::parse(&middle.right).unwrap().last_insert().unwrap(), None);
    let children: Vec<[u8; 8]> = (10..15_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let inner_halves = split(&Shape { level: 1, first: PageId(9), ..shape }, &refs(&inner), Some(3), (PageId(4), usize::MAX)).unwrap();
    let (inner_left, inner_right) = (Node::parse(&inner_halves.left).unwrap(), Node::parse(&inner_halves.right).unwrap());
    assert_eq!((inner_halves.separator.as_slice(), inner_left.len()), (&b"c"[..], 2), "the entry before the new one goes up");
    assert_eq!((inner_right.first_child(), keys(&inner_halves.right)), (PageId(12), vec![b"x".to_vec(), b"y".to_vec()]));
}

/// Verifies: [DESIGN] RULES.md D31, D500: a run's split that moves left
/// to fit, the rightmost leaf full to its page without the high key its
/// left half then takes, keeps the new entry marked inserted last in the
/// right half, so the run goes on there.
#[test]
fn a_run_split_moved_left_keeps_the_new_entry_marked() {
    let entries: Vec<(&[u8], &[u8])> = vec![(b"a", b"1"), (b"b", b"2"), (b"c", b"3"), (b"d", b"4"), (b"e", b"5")];
    let (full, _) = entries.split_at(4);
    let capacity = leaf(None, None, full).len();
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: TREE };
    let halves = split(&shape, &refs(&entries), Some(4), (PageId(4), capacity)).unwrap();
    assert!(halves.left.len() <= capacity && halves.right.len() <= capacity);
    assert_eq!(keys(&halves.right), vec![b"d".to_vec(), b"e".to_vec()], "moved left of the new entry");
    assert_eq!(Node::parse(&halves.right).unwrap().last_insert().unwrap(), Some(1), "the new entry, not the one before it");
}

/// Verifies: [DESIGN] RULES.md D31, B-link node: content that is not a
/// node is refused with SQLCODE -916: too short for its header, slots
/// overlapping the header, a high key without a right neighbour, an
/// entry past the heap, a child that is not a page number.
#[test]
fn malformed_nodes_are_refused() {
    let code = |bytes: &[u8]| Node::parse(bytes).err().map(|e| e.code);
    assert_eq!(code(&[0; 10]), Some(Code::Corrupt));
    let mut many = leaf(None, None, &[]);
    many[COUNT_AT] = 200;
    assert_eq!(code(&many), Some(Code::Corrupt));
    let mut lopsided = leaf(None, None, &[]);
    lopsided[FLAGS_AT] = HAS_HIGH;
    assert_eq!(code(&lopsided), Some(Code::Corrupt));
    let mut stray = leaf(None, None, &[(b"a", b"1")]);
    let last = stray.len() - SLOT;
    stray[last + 1] = 0xF0;
    assert_eq!(Node::parse(&stray).unwrap().entry(0).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(child_of(b"short").err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D31, suffix truncation: a leaf split keeps
/// as its high key the shortest key at or above its last and below the
/// right half's first: a prefix of that first key, one byte past what the
/// two share; its last key itself when that is a prefix of the first, or
/// when no shorter key lies between; an inner node's separator is kept
/// whole. Every key still goes to the half that holds it.
#[test]
fn leaf_separators_are_truncated() {
    assert_eq!(shortest_between(b"apple pie", b"banana split"), b"b");
    assert_eq!(shortest_between(b"index 0017 row 9", b"index 0018 row 1"), b"index 0018");
    assert_eq!(shortest_between(b"ab", b"abc"), b"ab", "a prefix of the first");
    assert_eq!(shortest_between(b"abc", b"abd"), b"abc", "nothing shorter between");
    let entries: Vec<(&[u8], &[u8])> = vec![(b"alpha one", b"1"), (b"alpha two", b"2"), (b"bravo one", b"3"), (b"bravo two", b"4")];
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: TREE };
    let halves = split(&shape, &refs(&entries), None, (PageId(4), usize::MAX)).unwrap();
    assert_eq!(halves.separator, b"b");
    let left = Node::parse(&halves.left).unwrap();
    assert_eq!(left.high, Some(&b"b"[..]));
    assert!(left.covers(b"alpha two") && !left.covers(b"bravo one"));
    let children: Vec<[u8; 8]> = (10..14_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let up = split(&Shape { level: 1, first: PageId(9), ..shape }, &refs(&inner), None, (PageId(4), usize::MAX)).unwrap();
    assert_eq!(up.separator, b"alpha two", "an inner separator is kept whole");
}

/// Verifies: [DESIGN] RULES.md D31, D500, a value written over in place
/// (a version stamped committed): its entry reads back with it, the
/// node's live bytes, garbage and last insert as they were; a shorter one
/// is written in place too, the bytes it leaves counted as garbage, which
/// building the node again reclaims; a longer one is refused.
#[test]
fn values_no_longer_are_written_over_in_place() {
    let mut content = leaf(None, None, &[(b"a", b"11"), (b"c", b"33")]);
    insert(&mut content, 1, b"b", b"22").unwrap();
    let before = {
        let node = Node::parse(&content).unwrap();
        (node.live(), node.garbage, node.last_insert().unwrap(), content.len())
    };
    overwrite(&mut content, 1, b"xy").unwrap();
    let node = Node::parse(&content).unwrap();
    assert_eq!(entry_of(&node, 1), (b"b".to_vec(), b"xy".to_vec()));
    assert_eq!((node.live(), node.garbage, node.last_insert().unwrap(), content.len()), before);
    assert_eq!(overwrite(&mut content, 1, b"xyz").err().map(|e| e.code), Some(Code::Corrupt), "longer");
    assert_eq!(overwrite(&mut content, 3, b"xy").err().map(|e| e.code), Some(Code::Corrupt), "no such entry");
    overwrite(&mut content, 1, b"z").unwrap();
    let shorter = Node::parse(&content).unwrap();
    assert_eq!(entry_of(&shorter, 1), (b"b".to_vec(), b"z".to_vec()), "a shorter value, in place");
    assert_eq!(
        (shorter.live(), shorter.garbage, content.len()),
        (before.0.checked_sub(1).unwrap(), before.1.checked_add(1).unwrap(), before.3),
        "its byte left is garbage"
    );
    assert_eq!((entry_of(&shorter, 0), entry_of(&shorter, 2)), ((b"a".to_vec(), b"11".to_vec()), (b"c".to_vec(), b"33".to_vec())));
    assert_eq!(
        compact_in(&content, 0).unwrap().len(),
        content.len().checked_sub(1).unwrap(),
        "and is reclaimed as the node is built again"
    );
}
