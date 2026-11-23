//! Unit tests of what a node changed in place keeps (RULES.md D864): a
//! value replaced by a longer one, the hints after an entry is inserted
//! or removed, and the room a node is built with.

use super::*;

/// A leaf of `count` keys with a prefix, `k` then a number, each of value
/// `value`.
fn numbered(count: u16, value: &[u8]) -> Vec<u8> {
    let named: Vec<Vec<u8>> = (0..count).map(|n| [&b"k"[..], &n.wrapping_mul(3).to_be_bytes()].concat()).collect();
    let entries: Vec<(&[u8], &[u8])> = named.iter().map(|k| (k.as_slice(), value)).collect();
    leaf(Some(b"z"), Some(PageId(9)), &entries)
}

/// Verifies: [DESIGN] RULES.md D864: a value replaced by a longer one in
/// place leaves the node byte for byte as removing the entry and
/// inserting it again does, its old bytes garbage, its entry marked as
/// inserted last, its hints as they were, for the first, a middle and the
/// last entry of a node with a prefix and hints.
#[test]
fn a_longer_value_written_in_place_is_the_entry_removed_and_inserted() {
    for at in [0_usize, 17, 39] {
        let mut content = numbered(40, b"old");
        let key = Node::parse(&content).unwrap().key(at).unwrap().to_vec();
        let mut again = content.clone();
        remove(&mut again, at).unwrap();
        insert(&mut again, at, &key, b"a longer value").unwrap();
        lengthen(&mut content, at, b"a longer value").unwrap();
        assert_eq!(content, again, "entry {at}");
        let node = Node::parse(&content).unwrap();
        assert_eq!(entry_of(&node, at), (key.clone(), b"a longer value".to_vec()));
        assert_eq!((node.last_insert().unwrap(), node.search(&key).unwrap()), (Some(at), Ok(at)));
        assert!(node.garbage() > 0);
    }
    assert!(lengthen(&mut numbered(4, b"v"), 4, b"longer").is_err(), "no such entry");
}

/// Verifies: [DESIGN] RULES.md D864, D500: after every insert and remove,
/// at the end of a node, at its start and in its middle, across the count
/// at which hints begin and the counts at which their step changes, the
/// hints are those sampling them again gives, and every key is found.
#[test]
fn hints_after_a_change_are_those_sampled_again() {
    let mut content = numbered(20, b"v");
    let key = |n: u16| [&b"k"[..], &n.to_be_bytes()].concat();
    let check = |held: &Vec<u8>, what: &str| {
        let mut sampled = held.clone();
        build::hint(&mut sampled).unwrap();
        assert_eq!(*held, sampled, "{what}");
        let node = Node::parse(held).unwrap();
        for i in 0..node.len() {
            assert_eq!(node.search(&node.key(i).unwrap().to_vec()).unwrap(), Ok(i), "{what}, entry {i}");
        }
    };
    // Appended, past the last hint: 20 keys to 120.
    for n in 20..120_u16 {
        let at = Node::parse(&content).unwrap().len();
        insert(&mut content, at, &key(n.wrapping_mul(3)), b"v").unwrap();
        check(&content, "appended");
    }
    // In the middle and at the start, between the keys there.
    for n in [1_u16, 100, 200, 301, 4, 250, 349] {
        let at = Node::parse(&content).unwrap().search(&key(n)).unwrap().unwrap_err();
        insert(&mut content, at, &key(n), b"v").unwrap();
        check(&content, "inserted");
    }
    // Removed from the end, the start and the middle, down past 32.
    while Node::parse(&content).unwrap().len() > 20 {
        let count = Node::parse(&content).unwrap().len();
        let at = [count - 1, 0, count >> 1_u32][count % 3];
        remove(&mut content, at).unwrap();
        check(&content, "removed");
    }
}

/// Verifies: [DESIGN] RULES.md D864: a node built with its page's room
/// takes entries in the buffer it was built in, and built again for a key
/// without its prefix it keeps that room; one built to its own size has
/// none to spare.
#[test]
fn a_node_built_with_room_takes_entries_where_it_is() {
    let shape = Shape { level: 0, high: None, right: None, first: PageId(0), tree: TREE };
    let entries = refs(&[(b"ka", b"1"), (b"kb", b"2")]);
    let mut roomy = built_in(4096, &shape, &entries, None, None).unwrap();
    assert!(roomy.capacity() >= 4096);
    assert_eq!(roomy, built_in(0, &shape, &entries, None, None).unwrap(), "the same node");
    let buffer = roomy.as_ptr();
    insert(&mut roomy, 2, b"kc", b"3").unwrap();
    assert_eq!(roomy.as_ptr(), buffer, "in place");
    insert(&mut roomy, 0, b"a", b"0").unwrap();
    assert!(roomy.capacity() >= 4096, "built again without its prefix, with its room");
    assert_eq!(keys(&roomy), [&b"a"[..], b"ka", b"kb", b"kc"]);
    let compacted = compact_in(&roomy, 4096).unwrap();
    assert!(compacted.capacity() >= 4096 && keys(&compacted) == keys(&roomy));
}
