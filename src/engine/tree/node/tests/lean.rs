//! Unit tests of a key's value read from a leaf with no node made of it
//! (RULES.md D950, `lean.rs`): it finds what a search of the node parsed
//! finds, wherever it is told to look first, and nothing in bytes that
//! are no leaf of the tree, in it.

use super::*;

/// A leaf of `n` keys of the shape `p{i:04}x`, `i` even, each with its
/// number as its value.
fn leaf_of(n: u16) -> Vec<u8> {
    let entries: Vec<(Vec<u8>, Vec<u8>)> =
        (0..n.checked_mul(2).unwrap()).step_by(2).map(|i| (format!("p{i:04}x").into_bytes(), i.to_le_bytes().to_vec())).collect();
    let borrowed: Vec<(&[u8], &[u8])> = entries.iter().map(|(k, v)| (k.as_slice(), v.as_slice())).collect();
    leaf(Some(b"p9999x"), Some(PageId(9)), &borrowed)
}

/// Verifies: [DESIGN] RULES.md D950: of leaves of 1, 31, 32 and 400
/// entries, every key present is found at the slot and with the value a
/// search of the node parsed gives, and every key between two, below or
/// above all, or without the leaf's prefix is not found, absent if the
/// leaf's high key is at or past it and unknown if not; told to look
/// first at the key's slot, at another, or past the last, it finds the
/// same. MC/DC of the slot tried first: (given, holds the key) taken,
/// against given and of another key, and not given, each searched.
#[test]
fn a_key_is_found_in_a_leafs_bytes_as_a_search_finds_it() {
    for n in [1_u16, 31, 32, 400] {
        let content = leaf_of(n);
        let node = Node::parse(&content).unwrap();
        let mut probes: Vec<Vec<u8>> = (0..=n.checked_mul(2).unwrap()).map(|i| format!("p{i:04}x").into_bytes()).collect();
        probes.extend([b"".to_vec(), b"a".to_vec(), b"p".to_vec(), b"p0000".to_vec(), b"p0000xx".to_vec(), b"q".to_vec(), vec![0xFF; 9]]);
        for probe in &probes {
            let found = node.search(probe).unwrap().ok();
            let at = found.unwrap_or(0);
            let covered = if probe.as_slice() <= b"p9999x".as_slice() { Held::Absent } else { Held::Unknown };
            let expected = found.map_or(covered, |slot| Held::At(slot, node.value(slot).unwrap()));
            for first in [None, Some(at), Some(at.wrapping_add(1)), Some(usize::from(n)), Some(usize::MAX)] {
                assert_eq!(
                    held(&content, TREE, probe, first),
                    expected,
                    "{n} entries, {:?}, first {first:?}",
                    String::from_utf8_lossy(probe)
                );
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D950, D500: a key is found only in a leaf
/// of the tree asked for that is in it. MC/DC of the leaf's check: (a
/// leaf, not taken out, of the tree) read, against an inner node, a leaf
/// marked taken out, and a leaf of another tree, each holding the key and
/// each refused; so are bytes shorter than a header, a slot leading past
/// the heap, and more slots than bytes, with no panic; and in slots that
/// do not fill the end, out of place, the key is not found.
#[test]
fn a_key_is_not_found_in_bytes_that_are_no_leaf_of_the_tree() {
    let content = leaf_of(40);
    let key = b"p0010x";
    assert!(matches!(held(&content, TREE, key, None), Held::At(..)));
    assert_eq!(held(&content, TREE + 1, key, None), Held::Unknown, "another tree's");
    let mut inner = content.clone();
    inner[LEVEL_AT] = 1;
    assert_eq!(held(&inner, TREE, key, None), Held::Unknown, "an inner node");
    let mut gone = content.clone();
    gone[FLAGS_AT] |= DELETED;
    assert_eq!(held(&gone, TREE, key, Some(5)), Held::Unknown, "taken out");
    assert_eq!(held(&content[..HEADER - 1], TREE, key, None), Held::Unknown, "shorter than a header");
    assert!(!matches!(held(&content[..content.len() - 1], TREE, key, None), Held::At(..)), "slots cut short");
    let mut past = content.clone();
    let slot = past.len() - 40 * SLOT + 5 * SLOT;
    past[slot] = 0xFF;
    past[slot + 1] = 0x7F;
    assert_eq!(held(&past, TREE, key, None), Held::Unknown, "a slot past the heap");
    assert_eq!(held(&past, TREE, key, Some(5)), Held::Unknown, "tried first");
    let mut many = content.clone();
    many[COUNT_AT] = 0xFF;
    many[COUNT_AT + 1] = 0xFF;
    assert_eq!(held(&many, TREE, key, None), Held::Unknown, "more slots than bytes");
}
