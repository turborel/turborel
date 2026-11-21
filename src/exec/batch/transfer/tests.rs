//! Unit tests of a phase of predicate transfer run: its leaves held within
//! the work area, and the filters made of them (RULES.md D668, R-TEST-1).

use super::*;
use crate::storage::{Column, Ints, Sel};

/// A batch of a key of `keys` and a wider vector beside it.
fn batch(keys: &[i64]) -> Kept {
    let wide: Vec<i64> = keys.iter().map(|k| k * 1_000).collect();
    let columns = vec![Column::Ints(Ints::new(wide, 0, None)), Column::Ints(Ints::new(keys.to_vec(), 0, None))];
    (Batch { columns, rows: keys.len() }, None)
}

/// The batches of `kept`, as a leaf gives them.
fn read(kept: Vec<Kept>) -> Batches {
    Box::new(kept.into_iter().map(Ok))
}

/// Verifies: [DESIGN] RULES.md D668 (MC/DC): a leaf's batches within what
/// is left of the work area are held whole and take it; past it, their key
/// vector alone is kept, to make a filter of, and the room is left.
#[test]
fn a_leaf_is_held_within_its_room() {
    let mut room = 1_usize << 20_u32;
    let (kept, whole) = held(read(vec![batch(&[1, 2]), batch(&[3])]), 1, &mut room).unwrap();
    assert!(whole);
    assert_eq!(kept.first().map(|(b, _)| b.columns.len()), Some(2_usize));
    assert!(room < 1_usize << 20_u32, "the held batches take their bytes");
    let mut tight = 8_usize;
    let (kept, whole) = held(read(vec![batch(&[1, 2]), batch(&[3])]), 1, &mut tight).unwrap();
    assert!(!whole);
    assert!(kept.iter().all(|(b, _)| b.columns.len() == 1), "the key's vector alone");
    assert_eq!(tight, 8_usize);
}

/// Verifies: [DESIGN] RULES.md D668, D369: a filter made of the rows a
/// leaf kept keeps every key they hold, and of no row, it keeps none.
#[test]
fn a_filter_is_made_of_the_rows_kept() {
    let db = crate::db::Database::new();
    let st = db.snapshot().unwrap();
    let kept = vec![(batch(&[4, 9, 16]).0, Some(Sel::of(vec![0, 2]))), batch(&[25])];
    let filter = filter_of(&kept, (1, false), &st).unwrap().unwrap();
    let probe = Batch { columns: vec![Column::Ints(Ints::new(vec![4, 9, 16, 25], 0, None))], rows: 4 };
    let held = filter.kept((&probe, 0), (None, (true, true)), st.functions.kernels()).unwrap();
    assert!(held.contains(&0_u32) && held.contains(&2_u32) && held.contains(&3_u32), "every key held: {held:?}");
    let none = filter_of(&[], (0, false), &st).unwrap().unwrap();
    assert!(none.kept((&probe, 0), (None, (false, true)), st.functions.kernels()).unwrap().is_empty());
}
