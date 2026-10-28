//! Unit tests of the B-link node format: nodes read back as built,
//! searched in key order, changed in place, split as `split` says, where
//! runs split, and malformed nodes refused (RULES.md R-TEST-1, R-TEST-2).
//! Split from node.rs (RULES.md R-SIZE-2).

use super::*;

fn leaf(high: Option<&[u8]>, right: Option<PageId>, entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    build(&Shape { level: 0, high, right, first: PageId(0) }, entries).unwrap()
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
    assert_eq!(Node::parse(&content).unwrap().entry(2).unwrap(), (&b"c"[..], &b"3"[..]));
    let before = content.len();
    remove(&mut content, 2).unwrap();
    let removed = Node::parse(&content).unwrap();
    assert_eq!((removed.live(), content.len()), (before - entry_size(b"c", b"3"), before - SLOT));
    assert_eq!(keys(&content), [b"a", b"b", b"d", b"e"]);
    let compacted = compact(&content).unwrap();
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
    let content = build(&Shape { level: 1, high: None, right: None, first: PageId(3) }, &[(b"f", &c5), (b"p", &c7)]).unwrap();
    let node = Node::parse(&content).unwrap();
    assert_eq!(node.child(b"a").unwrap(), (PageId(3), None));
    assert_eq!(node.child(b"f").unwrap(), (PageId(3), None));
    assert_eq!(node.child(b"g").unwrap(), (PageId(5), Some(&b"f"[..])));
    assert_eq!(node.child(b"zz").unwrap(), (PageId(7), Some(&b"p"[..])));
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
    let shape = Shape { level: 0, high: Some(b"k"), right: Some(PageId(8)), first: PageId(0) };
    let halves = split(&shape, &entries, None, PageId(4)).unwrap();
    let (left, right) = (Node::parse(&halves.left).unwrap(), Node::parse(&halves.right).unwrap());
    assert_eq!((keys(&halves.left), left.high, left.right), (vec![b"a".to_vec(), b"b".to_vec()], Some(&b"b"[..]), Some(PageId(4))));
    assert_eq!((keys(&halves.right), right.high, right.right), (vec![b"c".to_vec(), b"d".to_vec()], Some(&b"k"[..]), Some(PageId(8))));
    assert_eq!(halves.separator, b"b");
    let rightmost = Shape { high: None, right: None, ..shape };
    let appended = split(&rightmost, &entries, Some(3), PageId(4)).unwrap();
    assert_eq!((keys(&appended.right), appended.separator), (vec![b"d".to_vec()], b"c".to_vec()));
    let children: Vec<[u8; 8]> = (10..14_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let split_inner = split(&Shape { level: 1, first: PageId(9), ..rightmost }, &inner, None, PageId(4)).unwrap();
    let (inner_left, inner_right) = (Node::parse(&split_inner.left).unwrap(), Node::parse(&split_inner.right).unwrap());
    assert_eq!((split_inner.separator.as_slice(), inner_left.first_child(), inner_left.len()), (&b"b"[..], PageId(9), 1));
    assert_eq!((inner_right.first_child(), keys(&split_inner.right)), (PageId(11), vec![b"c".to_vec(), b"d".to_vec()]));
    assert!(split(&shape, &entries[..1], None, PageId(4)).is_err(), "one entry cannot split");
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
    let compacted = compact(&content).unwrap();
    assert_eq!(Node::parse(&compacted).unwrap().last_insert().unwrap(), Some(0), "compaction keeps the mark");
    let entries: Vec<(&[u8], &[u8])> = vec![(b"a", b"1"), (b"b", b"2"), (b"c", b"3"), (b"x", b"4"), (b"y", b"5")];
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0) };
    let halves = split(&shape, &entries, Some(2), PageId(4)).unwrap();
    let right = Node::parse(&halves.right).unwrap();
    assert_eq!(
        (keys(&halves.left), keys(&halves.right)),
        (vec![b"a".to_vec(), b"b".to_vec()], vec![b"c".to_vec(), b"x".to_vec(), b"y".to_vec()])
    );
    assert_eq!((halves.separator, right.last_insert().unwrap()), (b"b".to_vec(), Some(0)));
    let middle = split(&shape, &entries, None, PageId(4)).unwrap();
    assert_eq!(Node::parse(&middle.right).unwrap().last_insert().unwrap(), None);
    let children: Vec<[u8; 8]> = (10..15_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let inner_halves = split(&Shape { level: 1, first: PageId(9), ..shape }, &inner, Some(3), PageId(4)).unwrap();
    let (inner_left, inner_right) = (Node::parse(&inner_halves.left).unwrap(), Node::parse(&inner_halves.right).unwrap());
    assert_eq!((inner_halves.separator.as_slice(), inner_left.len()), (&b"c"[..], 2), "the entry before the new one goes up");
    assert_eq!((inner_right.first_child(), keys(&inner_halves.right)), (PageId(12), vec![b"x".to_vec(), b"y".to_vec()]));
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
    let last = stray.len() - 2;
    stray[last] = 0xF0;
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
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0) };
    let halves = split(&shape, &entries, None, PageId(4)).unwrap();
    assert_eq!(halves.separator, b"b");
    let left = Node::parse(&halves.left).unwrap();
    assert_eq!(left.high, Some(&b"b"[..]));
    assert!(left.covers(b"alpha two") && !left.covers(b"bravo one"));
    let children: Vec<[u8; 8]> = (10..14_u64).map(u64::to_le_bytes).collect();
    let inner: Vec<(&[u8], &[u8])> = entries.iter().zip(&children).map(|((k, _), c)| (*k, &c[..])).collect();
    let up = split(&Shape { level: 1, first: PageId(9), ..shape }, &inner, None, PageId(4)).unwrap();
    assert_eq!(up.separator, b"alpha two", "an inner separator is kept whole");
}
