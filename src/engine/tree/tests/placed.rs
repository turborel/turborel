//! Unit tests of changes in place from the leaf a write left its entry in
//! (RULES.md D843, `placed.rs`), and of writes from where a read found
//! its key (RULES.md D446, `write.rs`).

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

/// Verifies: [DESIGN] RULES.md D446: a write from where a read's descent
/// found its key replaces the value it read, though the leaf split since,
/// and refuses one changed since; one whose leaf was retired since, its
/// page freed and taken again, descends again.
#[test]
fn a_write_starts_where_its_read_found_the_key() {
    let (tree, _scratch) = tree("found-writes");
    tree.insert(&key(0), b"zero").unwrap();
    let (old, found) = tree.get_for_write(&key(0)).unwrap().unwrap();
    assert_eq!(old, b"zero");
    fill(&tree, 5_000);
    assert!(tree.replace_found(&found, &key(0), (b"zero", b"ZERO")).unwrap(), "moved right past the splits");
    assert_eq!(tree.get(&key(0)).unwrap(), Some(b"ZERO".to_vec()));
    let (_, six) = tree.get_for_write(&key(6)).unwrap().unwrap();
    tree.replace(&key(6), b"six").unwrap();
    assert!(!tree.replace_found(&six, &key(6), (&VALUE, b"SIX")).unwrap(), "changed since it was read");
    let (_, middle) = tree.get_for_write(&key(2_500)).unwrap().unwrap();
    for n in 2_000..3_000 {
        tree.delete(&key(n)).unwrap();
    }
    tree.free_retired().unwrap();
    assert!(tree.insert(&key(2_500), &VALUE).unwrap());
    for n in 10_000..14_000 {
        assert!(tree.insert(&key(n), &VALUE).unwrap());
    }
    assert!(tree.replace_found(&middle, &key(2_500), (&VALUE, b"middle")).unwrap(), "its leaf freed and taken again: descended");
    assert_eq!(tree.get(&key(2_500)).unwrap(), Some(b"middle".to_vec()));
    assert!(tree.get_for_write(&key(4_000_000)).unwrap().is_none());
}

/// Verifies: [DESIGN] RULES.md D856: a delete from a leaf split since
/// finds its entry to the right, and removes it only if its value is the
/// one expected; an update is shown its key's value, or none, and keeps,
/// puts or removes the entry as it says.
#[test]
fn a_delete_and_an_update_change_the_leaf_covering_their_key() {
    let (tree, _scratch) = tree("covering");
    assert!(tree.insert(&key(3_000), &VALUE).unwrap());
    let leaf = tree.descend(Target::Key(&key(3_000)), 0, &mut Path::default(), |_| Ok(())).unwrap().0;
    fill(&tree, 2_999);
    assert_eq!(tree.delete_from(&Path::default(), leaf, &key(3_000), Some(b"other")).unwrap(), None, "not the value expected");
    assert!(tree.delete_from(&Path::default(), leaf, &key(3_000), Some(&VALUE)).unwrap().is_some());
    assert_eq!(tree.get(&key(3_000)).unwrap(), None);
    let mut shown = None;
    let keep = |old: Option<&[u8]>| {
        shown = old.map(<[u8]>::to_vec);
        Ok(Update::Keep)
    };
    tree.update(&key(7), keep).unwrap();
    assert_eq!(shown, Some(VALUE.to_vec()), "the value where it lies");
    tree.update(&key(7), |_| Ok(Update::Put(b"seven".to_vec()))).unwrap();
    assert_eq!(tree.get(&key(7)).unwrap(), Some(b"seven".to_vec()));
    tree.update(&key(7), |_| Ok(Update::Remove)).unwrap();
    assert_eq!(tree.get(&key(7)).unwrap(), None);
    let put = |old: Option<&[u8]>| Ok(if old.is_none() { Update::Put(VALUE.to_vec()) } else { Update::Keep });
    tree.update(&key(7), put).unwrap();
    assert_eq!(tree.get(&key(7)).unwrap(), Some(VALUE.to_vec()));
}

/// Verifies: [DESIGN] RULES.md D865: a read for a write leaves the value
/// in the buffer it is given, cleared first, and none for a key absent;
/// a change that writes its value into the buffer it is handed puts,
/// keeps and removes as one that returns its value does, the buffer
/// handed in empty each time; and a value put longer than the one there,
/// with room in the leaf, is written in its place.
#[test]
fn values_are_read_into_and_put_from_buffers_kept() {
    let (tree, _scratch) = tree("buffers");
    fill(&tree, 100);
    let mut value = b"stale".to_vec();
    let found = tree.get_for_write_into(&key(7), &mut value).unwrap().unwrap();
    assert_eq!(value, VALUE);
    assert!(tree.replace_found(&found, &key(7), (&VALUE, b"seven")).unwrap());
    assert!(tree.get_for_write_into(&key(900), &mut value).unwrap().is_none());
    assert!(value.is_empty(), "cleared");
    let put = |text: &'static [u8]| {
        move |_: Option<&[u8]>, into: &mut Vec<u8>| {
            assert!(into.is_empty(), "handed in empty");
            into.extend_from_slice(text);
            Ok(Updated::Put)
        }
    };
    tree.update_alone_into(&key(7), 0, put(b"a value longer than seven")).unwrap();
    assert_eq!(tree.get(&key(7)).unwrap(), Some(b"a value longer than seven".to_vec()));
    tree.update_alone_into(&key(500), 0, put(b"new")).unwrap();
    assert_eq!(tree.get(&key(500)).unwrap(), Some(b"new".to_vec()));
    tree.update_alone_into(&key(500), 0, |old, _| Ok(if old == Some(&b"new"[..]) { Updated::Keep } else { Updated::Remove })).unwrap();
    assert_eq!(tree.get(&key(500)).unwrap(), Some(b"new".to_vec()), "kept");
    tree.update_alone_into(&key(500), 0, |_, _| Ok(Updated::Remove)).unwrap();
    assert_eq!(tree.get(&key(500)).unwrap(), None);
    assert_eq!(keys(&tree, Direction::Forward).len(), 100);
}

/// Verifies: [DESIGN] RULES.md D866: a write shown the value it replaces
/// sees its key's value, may copy it, and puts the bytes it gives, shorter
/// or longer, telling where the entry went; and one that gives none leaves
/// the entry.
#[test]
fn a_write_is_shown_the_value_it_replaces() {
    let (tree, _scratch) = tree("replace-with");
    for k in 1..=200_u8 {
        assert!(tree.insert(&key(u64::from(k)), &[k; 92]).unwrap());
    }
    let mut value = Vec::new();
    let seven = tree.get_for_write_into(&key(7), &mut value).unwrap().unwrap();
    let (mut undo, newer) = (Vec::new(), [9_u8; 92]);
    let copy = |old: &[u8]| {
        undo.extend_from_slice(old);
        Ok(Some(&newer[..]))
    };
    let placed = tree.replace_found_with(&seven, &key(7), copy).unwrap().unwrap();
    assert_eq!((undo, tree.get(&key(7)).unwrap()), (vec![7_u8; 92], Some(newer.to_vec())));
    assert_eq!(tree.change_placed(placed, &key(7), |bytes| Ok(bytes.len())).unwrap(), Some(92), "where it went");
    assert_eq!(tree.replace_found_with(&seven, &key(7), |_| Ok(None)).unwrap(), None, "left");
    let longer = [5_u8; 300];
    assert!(tree.replace_found_with(&seven, &key(7), |old| Ok((old == newer).then_some(&longer[..]))).unwrap().is_some());
    assert_eq!(tree.get(&key(7)).unwrap(), Some(longer.to_vec()));
}

/// Verifies: [DESIGN] RULES.md D866: a write shown the value it replaces
/// is shown nothing of a key gone since its read, and told none; and
/// after a leaf was retired since the read it finds its entry by a
/// descent.
#[test]
fn a_write_shown_its_value_finds_its_entry_gone_or_by_a_descent() {
    let (tree, _scratch) = tree("replace-with-gone");
    for k in 1..=200_u8 {
        assert!(tree.insert(&key(u64::from(k)), &[k; 92]).unwrap());
    }
    let mut value = Vec::new();
    let seven = tree.get_for_write_into(&key(7), &mut value).unwrap().unwrap();
    let fifty = tree.get_for_write_into(&key(50), &mut value).unwrap().unwrap();
    assert!(tree.delete(&key(7)).unwrap().is_some());
    let mut called = false;
    let unshown = |_: &[u8]| {
        called = true;
        Ok(None)
    };
    assert_eq!(tree.replace_found_with(&seven, &key(7), unshown).unwrap(), None);
    assert!(!called, "gone, nothing shown");
    for k in 100..=200_u8 {
        assert!(tree.delete(&key(u64::from(k))).unwrap().is_some());
    }
    assert!(tree.counts().unlinked > 0, "a leaf retired since the read");
    assert!(tree.replace_found_with(&fifty, &key(50), |old| Ok((old == [50_u8; 92]).then_some(b"fifty"))).unwrap().is_some());
    assert_eq!(tree.get(&key(50)).unwrap(), Some(b"fifty".to_vec()));
}

/// Verifies: [DESIGN] RULES.md D924: a run of keys is changed in place
/// from the leaf their writes left them in, in order, each told its place
/// in the run; the run ends at the first key the node covering the first
/// does not hold, here one not in the tree and, once the leaf has split,
/// one moved to its right, the keys before it changed; with no leaf, or
/// after a leaf was retired, none is.
#[test]
fn a_run_of_keys_is_changed_from_the_leaf_that_holds_them() {
    let (tree, _scratch) = tree("placed-run");
    let placed = tree.insert_placed(&key(1), &VALUE).unwrap().unwrap();
    fill_from(&tree, 2, 20);
    let mut seen = Vec::new();
    let mut flip = |n: usize, bytes: &mut [u8]| {
        bytes[0] = 1;
        seen.push(n);
        Ok(())
    };
    let run = [key(1), key(2), key(4), key(9_999), key(5)];
    assert_eq!(tree.change_placed_run(placed, &run, &mut flip).unwrap(), 3, "up to the key not there");
    assert_eq!(tree.change_placed_run(Placed::default(), &run, &mut flip).unwrap(), 0, "no leaf");
    let none: [[u8; 8]; 0] = [];
    assert_eq!(tree.change_placed_run(placed, &none, &mut flip).unwrap(), 0, "no key");
    assert_eq!(seen, [0, 1, 2]);
    let flipped = |k: u64| tree.get(&key(k)).unwrap().unwrap()[0] == 1;
    assert_eq!([1, 2, 3, 4, 5].map(flipped), [true, true, false, true, false]);
    // The leaf splits many times: a key far to its right is no longer in
    // the node that covers the first.
    fill_from(&tree, 21, 3_000);
    seen.clear();
    let mut again = |n: usize, bytes: &mut [u8]| {
        bytes[0] = 2;
        seen.push(n);
        Ok(())
    };
    let changed = tree.change_placed_run(placed, &[key(3), key(5), key(2_900)], &mut again).unwrap();
    assert_eq!((changed, seen.as_slice()), (2, &[0, 1][..]), "the key moved right is left to its caller");
    assert_eq!(tree.get(&key(2_900)).unwrap().unwrap()[0], 7);
    for n in 1_000..2_000 {
        tree.delete(&key(n)).unwrap();
    }
    assert_eq!(tree.change_placed_run(placed, &[key(3)], |_, _| Ok(())).unwrap(), 0, "a leaf was retired since");
}

/// Inserts keys `from..=to` in order.
fn fill_from(tree: &BLinkTree, from: u64, to: u64) {
    for i in from..=to {
        assert!(tree.insert(&key(i), &VALUE).unwrap());
    }
}
