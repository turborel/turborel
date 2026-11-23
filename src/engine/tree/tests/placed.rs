//! Unit tests of changes in place from the leaf a write left its entry in
//! (RULES.md D843, `placed.rs`).

use super::*;

/// Verifies: [DESIGN] RULES.md D843: a change from where an insert left
/// its entry finds it there, or to the right after the leaf split, and
/// changes it in place; one of a key not there, or after a leaf was
/// retired, is left to a descent.
#[test]
fn a_change_starts_where_the_write_left_the_entry() {
    let (tree, _scratch) = tree("placed-changes");
    let placed = tree.insert_placed(&key(3_000), &VALUE).unwrap().unwrap();
    assert!(placed.is_some());
    assert_eq!(tree.insert_placed(&key(3_000), &VALUE).unwrap(), None, "already there");
    fill(&tree, 2_999);
    let flip = |bytes: &mut [u8]| {
        bytes[0] = 1;
        Ok(bytes.len())
    };
    assert_eq!(tree.change_placed(placed, &key(3_000), flip).unwrap(), Some(VALUE.len()), "moved right past the splits");
    let mut changed = VALUE;
    changed[0] = 1;
    assert_eq!(tree.get(&key(3_000)).unwrap(), Some(changed.to_vec()));
    assert_eq!(tree.change_placed(placed, &key(9_999), flip).unwrap(), None, "no such key");
    assert_eq!(tree.change_placed(Placed::default(), &key(3_000), flip).unwrap(), None, "no leaf");
    let (_, found) = tree.get_for_write(&key(5)).unwrap().unwrap();
    let five = tree.replace_found_placed(&found, &key(5), (&VALUE, &changed)).unwrap().unwrap();
    assert_eq!(tree.replace_found_placed(&found, &key(5), (&VALUE, &changed)).unwrap(), None, "changed since it was read");
    for n in 1_000..2_000 {
        tree.delete(&key(n)).unwrap();
    }
    assert_eq!(tree.change_placed(five, &key(5), flip).unwrap(), None, "a leaf was retired since");
}
