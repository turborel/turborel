//! Unit tests of a node's search over its slots (RULES.md D840): every
//! key, present or not, is placed where a search of the entries one by
//! one places it, through the hints and without, under a shared prefix
//! and past it; a node read again from its header as parsed searches the
//! same; a slot leading past the heap is refused.

use super::*;

/// A leaf of `n` keys of the shape `p{i:04}x`, `i` even, so the odd ones
/// fall between them; `prefix` long keys share it.
fn leaf_of(n: u16) -> Vec<u8> {
    let keys: Vec<Vec<u8>> = (0..n.checked_mul(2).unwrap()).step_by(2).map(|i| format!("p{i:04}x").into_bytes()).collect();
    let entries: Vec<(&[u8], &[u8])> = keys.iter().map(|k| (k.as_slice(), &b"v"[..])).collect();
    leaf(None, None, &entries)
}

/// Where `key` goes among `node`'s entries, found one by one.
fn linear(node: &Node<'_>, key: &[u8]) -> std::result::Result<usize, usize> {
    for i in 0..node.len() {
        match node.key(i).map_or(std::cmp::Ordering::Greater, |k| k.cmp_bytes(key)) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => return Ok(i),
            std::cmp::Ordering::Greater => return Err(i),
        }
    }
    Err(node.len())
}

/// Verifies: [DESIGN] RULES.md D840, D500: of nodes of 1, 31 (no hints),
/// 32 and 400 entries (hints), every key present, every key between two,
/// keys below and above all, and keys without the node's prefix are
/// placed by the search where the entries one by one place them; a node
/// rebuilt from its header as parsed places them the same.
#[test]
fn a_search_over_the_slots_places_keys_as_the_entries_do() {
    for n in [1_u16, 31, 32, 400] {
        let content = leaf_of(n);
        let node = Node::parse(&content).unwrap();
        let again = Node::parsed_again(&content, &node.parsed()).unwrap();
        let mut probes: Vec<Vec<u8>> = (0..=n.checked_mul(2).unwrap()).map(|i| format!("p{i:04}x").into_bytes()).collect();
        probes.extend([b"".to_vec(), b"a".to_vec(), b"p".to_vec(), b"p0000".to_vec(), b"p0000xx".to_vec(), b"q".to_vec(), vec![0xFF; 9]]);
        for probe in &probes {
            let expected = linear(&node, probe);
            assert_eq!(node.search(probe).unwrap(), expected, "{n} entries, {:?}", String::from_utf8_lossy(probe));
            assert_eq!(again.search(probe).unwrap(), expected, "read again, {n} entries");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D840, R-ERR-2: a slot whose offset leads
/// past the heap fails the search that reads its tail with SQLCODE -916,
/// never a panic; a node whose slots do not fill its end in whole slots
/// is not rebuilt from a header parsed of other bytes.
#[test]
fn a_slot_past_the_heap_is_refused_by_the_search() {
    let mut content = leaf_of(3);
    let first = content.len() - 3 * SLOT;
    content[first] = 0xFF;
    content[first + 1] = 0x7F;
    let node = Node::parse(&content).unwrap();
    let probe = node.key(1).unwrap().to_vec();
    // Heads equal to the first entry's make the search read its tail.
    let low = node.search(b"p0000x").err().map(|e| e.code);
    assert_eq!(low, Some(Code::Corrupt));
    assert_eq!(node.search(&probe).unwrap(), Ok(1), "a slot not read is not checked");
    let parsed = node.parsed();
    assert!(Node::parsed_again(&content[..content.len() - 1], &parsed).is_none(), "not whole slots");
}
