//! Unit tests of the entries a forward scan copies at each read (RULES.md
//! D855, `scan.rs`): one, then a few, then whole leaves, each entry given
//! once and in order whatever is read again.

use super::*;

/// Verifies: [DESIGN] RULES.md D855: a forward scan copies one entry at
/// its first read, sixteen at its second, and the rest of the leaf at its
/// third, giving every key once, in order; a scan of one key copies one
/// entry and ends; a backward scan takes whole leaves.
#[test]
fn a_forward_scan_copies_one_entry_then_a_few_then_leaves() {
    let (tree, _scratch) = tree("taken-forward");
    fill(&tree, 300);
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let mut seen = vec![scan.next().unwrap().unwrap().0];
    assert_eq!(scan.held(), 1, "the first read");
    seen.push(scan.next().unwrap().unwrap().0);
    assert_eq!(scan.held(), 16, "the second");
    seen.extend(scan.by_ref().take(15).map(|e| e.unwrap().0));
    assert_eq!(scan.held(), 16, "all given, none read yet");
    seen.push(scan.next().unwrap().unwrap().0);
    assert!(scan.held() > 16, "the rest of the leaf");
    seen.extend(scan.map(|e| e.unwrap().0));
    assert_eq!(seen, (1..=300).map(|k| key(k).to_vec()).collect::<Vec<_>>());
    let mut one = tree.scan(Bound::Included(&key(7)), Bound::Included(&key(7)), Direction::Forward);
    assert_eq!(one.next().unwrap().unwrap().0, key(7));
    assert_eq!(one.held(), 1);
    assert!(one.next().is_none());
    let mut back = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Backward);
    assert_eq!(back.next().unwrap().unwrap().0, key(300));
    assert!(back.held() > 1, "a whole leaf");
    assert_eq!(back.count(), 299);
}

/// Verifies: [DESIGN] RULES.md D855, Lehman and Yao 1981 §4: a leaf that
/// splits, or loses entries, between a scan's reads of it gives each key
/// it still holds past the last one given once, in order, from it or from
/// the leaves to its right.
#[test]
fn a_leaf_changed_between_a_scans_reads_of_it_gives_each_key_once() {
    let (tree, _scratch) = tree("taken-split");
    for k in 1..=40 {
        assert!(tree.insert(&key(k * 100), &VALUE).unwrap());
    }
    let splits = tree.counts().splits;
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let mut seen = vec![scan.next().unwrap().unwrap().0];
    for k in 1..=40 {
        for n in 1..=5 {
            assert!(tree.insert(&key(k * 100 + n), &VALUE).unwrap());
        }
    }
    assert!(tree.counts().splits > splits, "split since its first read");
    assert!(tree.delete(&key(200)).unwrap().is_some());
    seen.extend(scan.map(|e| e.unwrap().0));
    let want: Vec<Vec<u8>> = (1..=40_u64).flat_map(|k| (0..=5).map(move |n| k * 100 + n)).filter(|&k| k != 200).map(|k| key(k).to_vec()).collect();
    assert_eq!(seen, want);
}
