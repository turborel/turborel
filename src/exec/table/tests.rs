//! Unit tests of the executor's table of groups (RULES.md D356, D357):
//! groups found as their keys are equal, numbered in the order made, past
//! growth and the keyed hash; hashes alike of a key in any vector.

use super::*;
use crate::kernels::Portable;
use crate::storage::{Cell, Coded, Column, Floats, Ints, Mask, Texts};
use crate::value::{Decimal, Value};

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// A table of keys of `width` columns, no set function.
fn table(width: usize) -> GroupTable {
    GroupTable::new(width, &[], &Seed::fixed(0x5EED), usize::MAX)
}

/// Verifies: [DESIGN] (RULES.md D356) a key's hash is one of its value,
/// whatever vector holds it: of a batch's integers, numbers and texts,
/// nulls among them, of values, and of a key of no column.
#[test]
fn keys_hash_alike_in_any_vector() {
    let seed = Seed::fixed(7);
    let nulls = Mask::from_bools(&[false, true, false, false]);
    let batch = Batch {
        columns: vec![
            Column::Ints(Ints::new(vec![1, 0, -3, 40], 0, Some(nulls.clone()))),
            Column::Ints(Ints::new(vec![150, 0, 7, 100], 2, Some(nulls.clone()))),
            Column::Floats(Floats::new(vec![-0.0_f64, 0.0_f64, 2.5_f64, f64::NAN], Some(nulls))),
            Column::Texts(Texts::new("ab  cdee ".to_owned(), vec![4, 4, 6, 9], None)),
        ],
        rows: 4,
    };
    let at = [0_usize, 1, 2, 3];
    for rows in [None, Some(&[1_u32, 3][..])] {
        let mut hashes = Vec::new();
        hash_rows(&batch, (&at, rows), (&seed, &Portable), &mut hashes);
        let given: Vec<usize> = rows.map_or_else(|| (0..4).collect(), |r| r.iter().map(|&n| usize::try_from(n).unwrap()).collect());
        for (&h, &r) in hashes.iter().zip(&given) {
            let values: Vec<Value> = at.iter().map(|&c| batch.cell(c, r).to_value()).collect();
            assert_eq!(h, hash_values(&seed, &values), "row {r}");
        }
    }
    assert_eq!(hash_values(&seed, &[exact(1, 0)]), hash_values(&seed, &[exact(100, 2)]));
    assert_ne!(hash_values(&seed, &[exact(15, 1)]), hash_values(&seed, &[exact(15, 0)]), "1.5 is not 15");
    assert_eq!(hash_values(&seed, &[Value::Char("a ".into())]), hash_values(&seed, &[Value::Char("a".into())]));
    let mut none = Vec::new();
    hash_rows(&batch, (&[], None), (&seed, &Portable), &mut none);
    assert_eq!(none, vec![hash_values(&seed, &[]); 4]);
}

/// Verifies: [DESIGN] (RULES.md D573) a key's hash is XXH3-64 by the
/// seed: a word's of its 8 bytes, a text's of its bytes less its trailing
/// spaces, a key of two columns that of the 16 bytes of their hashes; and
/// texts coded by a dictionary hash as their entries, nulls as null,
/// of fewer rows than entries and of more.
#[test]
fn keys_hash_by_xxh3_in_any_vector() {
    use crate::hash::xxhash::xxh3_64_with_seed;
    let seed = Seed::fixed(0x00C0_FFEE);
    let word = xxh3_64_with_seed(&(-5_i64).to_le_bytes(), 0x00C0_FFEE);
    assert_eq!(hash_values(&seed, &[exact(-5, 0)]), word);
    let long = "z".repeat(300);
    let text = xxh3_64_with_seed(long.as_bytes(), 0x00C0_FFEE);
    assert_eq!(hash_values(&seed, &[Value::Char(format!("{long}   ").into())]), text);
    let both: Vec<u8> = word.to_le_bytes().into_iter().chain(text.to_le_bytes()).collect();
    assert_eq!(hash_values(&seed, &[exact(-5, 0), Value::Char(long.clone().into())]), xxh3_64_with_seed(&both, 0x00C0_FFEE));
    let entries = std::sync::Arc::new(Texts::new(format!("ab{long}c  "), vec![2, 302, 305], None));
    let nulls = Mask::from_bools(&[false, true, false, false, false]);
    let coded = Texts::coded(Coded { codes: vec![1, 0, 2, 0, 1], entries }, Some(nulls));
    let batch = Batch { columns: vec![Column::Texts(coded)], rows: 5 };
    for rows in [None, Some(&[4_u32, 1][..]), Some(&[2_u32][..])] {
        let mut hashes = Vec::new();
        hash_rows(&batch, (&[0], rows), (&seed, &Portable), &mut hashes);
        let given: Vec<usize> = rows.map_or_else(|| (0..5).collect(), |r| r.iter().map(|&n| usize::try_from(n).unwrap()).collect());
        let want: Vec<u64> = given.iter().map(|&r| hash_values(&seed, &[batch.cell(0, r).to_value()])).collect();
        assert_eq!(hashes, want, "rows {rows:?}");
    }
}

/// Verifies: [DESIGN] (RULES.md D356) groups are found as their keys'
/// bytes are equal, numbered in the order they were made, each finished
/// with its first row's number and first value, past the slots' growth.
#[test]
fn groups_are_found_in_the_order_made() {
    let mut groups = table(2);
    let keys = [
        vec![exact(1, 0), Value::Char("a".into())],
        vec![exact(10, 1), Value::Char("a  ".into())],
        vec![Value::Null, Value::Null],
        vec![exact(1, 0), Value::Null],
        vec![Value::Null, Value::Null],
    ];
    let found: Vec<u32> =
        (0_u64..).zip(&keys).map(|(n, key)| groups.found_or_made(hash_values(groups.seed(), key), Key::Values(key), n)).collect();
    assert_eq!(found, [0, 0, 1, 2, 1]);
    let finished = groups.finished().unwrap();
    assert_eq!(finished.iter().map(|g| g.0).collect::<Vec<_>>(), [0, 2, 3]);
    assert_eq!(format!("{:?}", finished[0].1), format!("{:?}", keys[0]));
    let mut many = table(1);
    for n in 0_i128..10_000 {
        let key = [exact(n, 0)];
        assert_eq!(many.found_or_made(hash_values(many.seed(), &key), Key::Values(&key), 0), u32::try_from(n).unwrap());
    }
    for n in (0_i128..10_000).rev() {
        let key = [exact(n, 0)];
        assert_eq!(many.find(hash_values(many.seed(), &key), Key::Values(&key)), Some(u32::try_from(n).unwrap()));
    }
}

/// Verifies: [DESIGN] (RULES.md D357) keys whose hashes all collide make
/// a probe past `MAX_PROBE`, and the table hashes its keys by SipHash
/// keyed by its seed from then on: every group is found again, in the
/// order made, and new groups are made after them. Keys equal never
/// collide, so the probe bound is what turns it, not their count.
#[test]
fn colliding_keys_turn_the_table_keyed() {
    let mut groups = table(1);
    let colliding = 0_u64;
    for n in 0_i128..300 {
        let key = [exact(n, 0)];
        groups.found_or_made(colliding, Key::Values(&key), 0);
    }
    assert!(groups.is_keyed());
    for n in 0_i128..300 {
        let key = [exact(n, 0)];
        assert_eq!(groups.find(colliding, Key::Values(&key)), Some(u32::try_from(n).unwrap()));
    }
    let new = [Value::Char("new".into())];
    assert_eq!(groups.found_or_made(colliding, Key::Values(&new), 0), 300);
    let mut spread = table(1);
    for n in 0_i128..300 {
        let key = [exact(n, 0)];
        spread.found_or_made(hash_values(spread.seed(), &key), Key::Values(&key), 0);
    }
    assert!(!spread.is_keyed());
}

/// Verifies: [DESIGN] (RULES.md D356) a table kept to its first groups
/// finds those as before and no other, makes new ones after them, gives
/// back the room of the others, its slots' too, and is full only of a
/// group past its limit, never empty.
#[test]
fn a_table_truncated_keeps_its_first_groups() {
    let mut groups = GroupTable::new(1, &[], &Seed::fixed(1), 1);
    assert!(!groups.full(), "an empty table is never full");
    for n in 0_i128..100 {
        let key = [exact(n, 0)];
        groups.found_or_made(hash_values(groups.seed(), &key), Key::Values(&key), 0);
    }
    assert!(groups.full());
    let before = groups.bytes();
    groups.truncate(10);
    assert!(groups.bytes().saturating_mul(4) < before, "{} of {before}", groups.bytes());
    let (kept, gone) = ([exact(9, 0)], [exact(10, 0)]);
    assert_eq!(groups.find(hash_values(groups.seed(), &kept), Key::Values(&kept)), Some(9));
    assert_eq!(groups.find(hash_values(groups.seed(), &gone), Key::Values(&gone)), None);
    assert_eq!(groups.found_or_made(hash_values(groups.seed(), &gone), Key::Values(&gone), 0), 10);
    let mut bytes = Vec::new();
    groups.key_bytes(10, &mut bytes);
    let mut expected = Vec::new();
    crate::exec::batch::put_key(Cell::Value(&gone[0]), &mut expected);
    assert_eq!(bytes, expected);
}
