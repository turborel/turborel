//! Unit tests of the node layout of RULES.md D500: prefixes kept once,
//! key heads beside the slots, hints over the heads, and the tree a node
//! names, read back and searched as the keys they stand for.

use super::*;

/// A fixed sequence of numbers, the same every run (RULES.md R-TEST-5).
struct Seq(u64);

impl Seq {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0.wrapping_shr(33).checked_rem(below).unwrap()
    }

    /// A key of 0 to 11 bytes from a small alphabet, so keys share
    /// prefixes and heads often.
    fn key(&mut self) -> Vec<u8> {
        let len = self.next(12);
        (0..len).map(|_| b"aab\x00z"[usize::try_from(self.next(5)).unwrap()]).collect()
    }
}

/// Where `key` is among `keys`, sorted, as `Node::search` says it.
fn place(keys: &[Vec<u8>], key: &[u8]) -> std::result::Result<usize, usize> {
    keys.binary_search_by(|k| k.as_slice().cmp(key))
}

/// That every key of `keys` and every key of `probes` is found where it
/// is, or would go, in the node `content` holds, and that it holds `keys`.
fn finds(content: &[u8], keys: &[Vec<u8>], probes: &[Vec<u8>]) {
    let node = Node::parse(content).unwrap();
    assert_eq!(super::keys(content), keys);
    for key in keys.iter().chain(probes) {
        assert_eq!(node.search(key).unwrap(), place(keys, key), "key {key:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D500, prefix truncation: a node keeps once
/// what its first key shares with its last, after its high key, and gives
/// every key back whole; a key without the prefix is found below or above
/// every entry; a key inserted below the first or above the last that
/// shares less shortens the prefix, the node built again, its keys still
/// whole; the rightmost node keeps a prefix too.
#[test]
fn prefixes_are_kept_once_and_keys_read_whole() {
    let mut content = leaf(Some(b"index 0018"), Some(PageId(9)), &[(b"index 0011 a", b"1"), (b"index 0012 b", b"2")]);
    let node = Node::parse(&content).unwrap();
    assert_eq!(node.prefix, b"index 001");
    // The header, the high key, the prefix, and each entry's tail of 3
    // bytes, value of 1, two lengths and a slot.
    assert_eq!(content.len(), HEADER + 10 + 9 + 2 * (3 + 1 + 2 * LEN + SLOT));
    assert_eq!((node.search(b"apple").unwrap(), node.search(b"index 002").unwrap()), (Err(0), Err(2)));
    assert_eq!((node.search(b"index 00").unwrap(), node.search(b"index 001").unwrap()), (Err(0), Err(0)));
    let bigger = leaf(Some(b"index 0018"), Some(PageId(9)), &[(b"index 0011 a", b"1"), (b"index 0012 b", b"2")]);
    assert_eq!(grown(&Node::parse(&bigger).unwrap(), b"index 0005"), 2, "two keys, each a byte longer");
    assert_eq!(grown(&Node::parse(&bigger).unwrap(), b"index 0013"), 0);
    insert(&mut content, 0, b"index 0005", b"0").unwrap();
    insert(&mut content, 1, b"index 001", b"p").unwrap();
    let shorter = Node::parse(&content).unwrap();
    assert_eq!(shorter.prefix, b"index 00");
    assert_eq!(keys(&content), [&b"index 0005"[..], b"index 001", b"index 0011 a", b"index 0012 b"]);
    assert_eq!((shorter.search(b"index 001").unwrap(), shorter.last_insert().unwrap()), (Ok(1), Some(1)));
    let mut rightmost = leaf(None, None, &[(b"index 0011 a", b"1"), (b"index 0012 b", b"2")]);
    assert_eq!(Node::parse(&rightmost).unwrap().prefix, b"index 001");
    insert(&mut rightmost, 2, b"index 0020", b"3").unwrap();
    assert_eq!(Node::parse(&rightmost).unwrap().prefix, b"index 00", "appended past a byte's boundary");
    assert_eq!(keys(&rightmost), [&b"index 0011 a"[..], b"index 0012 b", b"index 0020"]);
}

/// Verifies: [DESIGN] RULES.md D500, key heads and hints: in nodes of
/// keys of every length from none to past a head, sharing prefixes and
/// heads, many with zero bytes, every key is found at its place and
/// every other at where it would go: as built, with hints and without,
/// and after inserts and removals in place, which sample the hints again.
#[test]
fn heads_and_hints_find_every_key() {
    let mut seq = Seq(17);
    let mut keys: Vec<Vec<u8>> = (0..400_u32).map(|_| seq.key()).collect();
    keys.sort();
    keys.dedup();
    let probes: Vec<Vec<u8>> = (0..300_u32).map(|_| seq.key()).collect();
    for count in [0, 1, HINTED - 1, HINTED, keys.len()] {
        let some = &keys[..count.min(keys.len())];
        let entries: Vec<(&[u8], &[u8])> = some.iter().map(|k| (k.as_slice(), &b"v"[..])).collect();
        finds(&leaf(Some(b"zzzz"), Some(PageId(2)), &entries), some, &probes);
    }
    let mut held: Vec<Vec<u8>> = keys[..HINTED + 3].to_vec();
    let entries: Vec<(&[u8], &[u8])> = held.iter().map(|k| (k.as_slice(), &b"v"[..])).collect();
    let mut content = leaf(Some(b"zzzz"), Some(PageId(2)), &entries);
    for round in 0..200_u32 {
        let key = seq.key();
        match (place(&held, &key), round % 3_u32) {
            (Ok(at), 0) => {
                remove(&mut content, at).unwrap();
                held.remove(at);
            }
            (Err(at), _) => {
                insert(&mut content, at, &key, b"w").unwrap();
                held.insert(at, key);
            }
            (Ok(_), _) => {}
        }
        finds(&content, &held, &probes);
    }
    finds(&compact(&content).unwrap(), &held, &probes);
}

/// Verifies: [DESIGN] RULES.md D500, a node names its tree: a node built,
/// both halves of its split, a node compacted and a leaf deleted keep the
/// tree's meta page; a damaged prefix length past the node is refused.
#[test]
fn a_node_names_its_tree() {
    let content = leaf(None, None, &[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]);
    assert_eq!(Node::parse(&content).unwrap().tree, TREE);
    let node = Node::parse(&content).unwrap();
    let halves = split(&node.shape(), &node.entries().unwrap(), None, (PageId(4), usize::MAX)).unwrap();
    assert_eq!((Node::parse(&halves.left).unwrap().tree, Node::parse(&halves.right).unwrap().tree), (TREE, TREE));
    assert_eq!(Node::parse(&compact(&content).unwrap()).unwrap().tree, TREE);
    let gone = deleted(PageId(4), TREE).unwrap();
    assert!(Node::parse(&gone).unwrap().deleted && Node::parse(&gone).unwrap().tree == TREE);
    let mut long = leaf(Some(b"m"), Some(PageId(9)), &[(b"b", b"2")]);
    long[PREFIX_AT..PREFIX_AT + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(Node::parse(&long).err().map(|e| e.code), Some(Code::Corrupt));
}
