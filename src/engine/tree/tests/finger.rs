//! Unit tests of reads by key from a copy of the leaf the reads before
//! found (RULES.md D806, `near.rs`). A run of keys of one leaf is read
//! from the copy with no page read, a change to the leaf ends the copy,
//! and a split moves keys right out of it.

use super::*;

/// The keys of `1..=n` in the leaf that holds key 1.
fn first_leaf(tree: &BLinkTree, n: u64) -> Vec<u64> {
    let leaf = leaf_of(tree, 1);
    (1..=n).filter(|&k| leaf_of(tree, k) == leaf).collect()
}

/// Verifies: [DESIGN] RULES.md D806, D502: keys of one leaf read one
/// after another read the leaf from the pool twice, then from the copy
/// kept, with no page read; a value replaced in the leaf moves its frame's
/// version, so the next read takes the leaf again and finds the new value.
#[test]
fn a_run_of_keys_of_one_leaf_is_read_from_its_copy() {
    let (tree, _scratch) = tree("finger-run");
    fill(&tree, 300);
    let run = first_leaf(&tree, 300);
    assert!(run.len() > 8, "a leaf of {} keys", run.len());
    let mut near = Near::default();
    let hits = tree.pool.counts().hits;
    for &k in run.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    assert!(tree.pool.counts().hits > hits, "the leaf read from the pool");
    let copied = tree.pool.counts().hits;
    for &k in run.iter().skip(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
        assert_eq!(near.bytes, VALUE, "key {k}");
    }
    assert_eq!(tree.pool.counts().hits, copied, "the rest from the copy");
    let last = *run.last().unwrap();
    let mut newer = VALUE;
    newer[0] = 9;
    assert!(tree.replace(&key(last), &newer).unwrap().is_some());
    assert!(tree.get_near(&key(last), &mut near).unwrap());
    assert_eq!(near.bytes, newer, "the change seen, the copy dropped");
}

/// Verifies: [DESIGN] RULES.md D806, Lehman and Yao §5: a copy kept of a
/// leaf that splits since is not read. Keys that moved right are found
/// in the right half, and keys inserted are found.
#[test]
fn a_copy_of_a_leaf_split_since_is_not_read() {
    let (tree, _scratch) = tree("finger-split");
    for k in (2..=200).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    let run: Vec<u64> = first_leaf(&tree, 200).into_iter().filter(|k| k.is_multiple_of(2)).collect();
    let mut near = Near::default();
    for &k in run.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    let splits = tree.counts().splits;
    let end = *run.last().unwrap();
    for k in (1..end).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    assert!(tree.counts().splits > splits, "the leaf split");
    for k in 1..=end {
        assert!(tree.get_near(&key(k), &mut near).unwrap(), "key {k}");
    }
    assert!(!tree.get_near(&key(end.checked_add(1).unwrap()), &mut near).unwrap(), "odd keys past it are not in");
}

/// Verifies: [DESIGN] RULES.md D808, D950: keys read in order, backwards
/// and skipping through the copy of a leaf `near` keeps are each found,
/// at their slots, with no page read, and a key not in the leaf is not.
/// For MC/DC of `in_copy`'s decision, a key at the slot after the last
/// found is taken there; one elsewhere in the copy is searched for; one
/// the copy answers for and does not hold is not found; and one past the
/// leaf is read from the next, from the pool.
#[test]
fn keys_of_a_leaf_copied_are_read_at_their_slots() {
    let (tree, _scratch) = tree("finger-parsed");
    for k in (2..=600).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    let run: Vec<u64> = first_leaf(&tree, 600).into_iter().filter(|k| k.is_multiple_of(2)).collect();
    let mut near = Near::default();
    for &k in run.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    assert!(near.kept.is_some(), "the leaf copied");
    let hits = tree.pool.counts().hits;
    let order = run.iter().skip(3).chain(run.iter().rev()).chain(run.iter().step_by(3));
    for &k in order {
        assert!(tree.get_near(&key(k), &mut near).unwrap(), "key {k}");
        assert_eq!(near.at, run.iter().position(|&held| held == k), "key {k} at its slot");
    }
    assert!(!tree.get_near(&key(3), &mut near).unwrap(), "an odd key is not in the leaf");
    assert_eq!(tree.pool.counts().hits, hits, "all from the copy");
    let past = run.last().unwrap().checked_add(2).unwrap();
    assert!(tree.get_near(&key(past), &mut near).unwrap(), "the next leaf's first key");
    assert!(tree.pool.counts().hits > hits, "read from the pool, not the copy");
}

/// Verifies: [DESIGN] RULES.md D863: a read shown its value where it lies
/// is shown the value of its key from every place a read looks in, a
/// descent, the leaf remembered, the copy of it and the leaf noted for
/// the row; a key absent shows nothing; the bytes `near` keeps are not
/// touched; and `near` is left for the next read as a copying read
/// leaves it, which then reads from the same copy.
#[test]
fn a_read_is_shown_its_value_where_it_lies() {
    let (tree, _scratch) = tree("shown");
    tree.note_rows();
    for k in 1..=200_u64 {
        assert!(tree.insert(&key(k), &[u8::try_from(k).unwrap(); 92]).unwrap());
    }
    let first_byte = |value: &[u8]| Ok((value[0], value.len()));
    let mut near = Near::default();
    near.bytes.extend_from_slice(b"kept");
    // Rows noted by the inserts are pushed out, so the first read descends.
    hints::note(10, hints::NO_LEAF);
    for k in 10..=20_u8 {
        hints::note(u64::from(k), hints::NO_LEAF);
        assert_eq!(tree.get_near_with(&key(u64::from(k)), &mut near, first_byte).unwrap(), Some((k, 92)), "key {k}");
    }
    assert!(near.kept.is_some(), "its leaf copied, the later reads from the copy");
    assert_eq!(near.bytes, b"kept", "nothing copied");
    let hits = tree.pool.counts().hits;
    assert!(tree.get_near(&key(21), &mut near).unwrap());
    assert_eq!((near.bytes[0], tree.pool.counts().hits), (21, hits), "the next read goes on from the copy");
    let mut called = false;
    let never = |_: &[u8]| {
        called = true;
        Ok(())
    };
    assert_eq!(tree.get_near_with(&key(900), &mut near, never).unwrap(), None);
    assert!(!called, "absent, nothing shown");
    let leaf = leaf_of(&tree, 150);
    hints::note(150, leaf.0);
    let hinted = tree.counts().hinted;
    assert_eq!(tree.get_near_with(&key(150), &mut Near::default(), first_byte).unwrap(), Some((150, 92)));
    assert_eq!(tree.counts().hinted, hinted + 1, "from the leaf noted");
    let refused = tree.get_near_with(&key(5), &mut near, |_| Err::<(), _>(SqlError::new(Code::Api, "[DESIGN] refused")));
    assert_eq!(refused.unwrap_err().code, Code::Api);
}
