//! Unit tests of a join's table: its slots and tags, the order of a key's
//! rows, the hashes of each kind of vector against those of its values,
//! and its keys compared in place against `key_equal` (RULES.md D368,
//! D369, R-TEST-1, R-TEST-8).

use super::keys::{nearest, value_hash};
use super::*;
use crate::exec::table::Seed;
use crate::storage::{Batch, Column, Floats, Ints, Mask, Texts};
use crate::value::{Decimal, Value, key_equal};

/// A seed of known hashes.
fn seed() -> Seed {
    Seed::fixed(0x5EED)
}

/// The rows a probe of hash `h` finds of `table`, in order.
fn found(table: &JoinTable, h: u64) -> Vec<usize> {
    table.candidates(h).collect()
}

/// Verifies: [DESIGN] RULES.md D368: a table finds each hash's rows in
/// the order they were built, of a slot of a few read in turn and of one
/// of more than `LINEAR` sorted and searched, a key of many rows beside
/// keys of one in one slot; a miss is never found; a hash none holds
/// finds none.
#[test]
fn a_table_finds_a_hashs_rows_in_build_order() {
    // All in slot 0 of 16: hashes below 2^60.
    let mut hashes = vec![7_u64, 3, 7, 9, 7];
    let mut misses = vec![false, false, false, false, true];
    assert_eq!(found(&JoinTable::of(&hashes, &misses), 7), [0, 2]);
    assert_eq!(found(&JoinTable::of(&hashes, &misses), 8), Vec::<usize>::new());
    for n in 0..20_u64 {
        hashes.push(if n % 2 == 0 { 7 } else { 100 + n });
        misses.push(false);
    }
    let table = JoinTable::of(&hashes, &misses);
    let sevens: Vec<usize> = hashes.iter().zip(&misses).enumerate().filter(|(_, (h, m))| **h == 7 && !**m).map(|(r, _)| r).collect();
    assert_eq!(found(&table, 7), sevens);
    assert_eq!(found(&table, 103), [8]);
    assert_eq!(found(&table, 104), Vec::<usize>::new());
}

/// Verifies: [DESIGN] RULES.md D368: a probe whose tag bit no row of its
/// slot set is turned away by the directory word; one whose bit another
/// hash set reads the slot and finds none of its own.
#[test]
fn tags_turn_away_a_probe_of_no_bit() {
    let table = JoinTable::of(&[0x10_u64], &[false]);
    let at = table.slot(0x10);
    let word = table.parts[0].directory[at + 1];
    assert_eq!(word & tag(0x11), 0);
    assert_ne!(word & tag(0x10), 0);
    assert_eq!(found(&table, 0x11), Vec::<usize>::new());
    assert_eq!(found(&table, 0x20), Vec::<usize>::new(), "the same tag bit, another hash");
}

/// Verifies: [DESIGN] RULES.md D368: a table of many rows has a slot for
/// each two at least, a power of two, every row found by its hash.
#[test]
fn a_table_has_two_slots_a_row() {
    let hashes: Vec<u64> = (0..1_000_u64).map(crate::hash::murmur::fmix64).collect();
    let table = JoinTable::of(&hashes, &vec![false; hashes.len()]);
    let slots = table.parts[0].directory.len() - 1;
    assert!(slots.is_power_of_two() && slots >= 2_000);
    assert!(hashes.iter().enumerate().all(|(r, &h)| found(&table, h).contains(&r)));
}

/// Verifies: [DESIGN] RULES.md D368: the nearest `f64` of an exact number
/// is the one its digits give, of small integers and scales exactly, of
/// large ones and scales by the value's bucket.
#[test]
fn nearest_is_a_numbers_digits() {
    for (units, scale) in
        [(0_i128, 0_u32), (-7, 0), (12_345, 2), (1_i128 << 52_u32, 0), (i128::from(i64::MAX), 0), (1, 23), (-999_999, 6), (3, 22)]
    {
        let want: f64 = format!("{units}e-{scale}").parse().unwrap();
        assert_eq!(nearest(units, scale).to_bits(), want.to_bits(), "{units}e-{scale}");
    }
}

/// The hashes of the key of one column `column` of every row.
fn hashed(column: Column, nulls: bool) -> KeyHashes {
    let rows = match &column {
        Column::Ints(i) => i.units.len(),
        Column::Floats(f) => f.numbers.len(),
        Column::Texts(t) => t.len(),
        Column::Values(v) => v.len(),
        Column::Datetimes(_) | Column::Bools(_) | Column::Nulls(_) | Column::Late(_) => 0,
    };
    let batch = Batch { columns: vec![column], rows };
    let mut out = KeyHashes::default();
    out.of_batch((&batch, None), (&[0], &[nulls]), Hashing { seed: &seed(), keyed: false, direct: false, kernels: None }).unwrap();
    out
}

/// Verifies: [DESIGN] RULES.md D368: a key hashes alike in a vector of
/// integers, of approximate numbers, of texts and of values: 1, 1.0 and
/// 1.00 alike, 'a' and 'a  ' alike, -0 as 0; a null a miss but on a key
/// whose nulls match.
#[test]
fn a_key_hashes_alike_in_any_vector() {
    let how = Hashing { seed: &seed(), keyed: false, direct: false, kernels: None };
    let of = |v: Value| value_hash(&v, false, how).unwrap().unwrap();
    let ints = hashed(Column::Ints(Ints::new(vec![1, 100, 0], 2, Some(Mask::from_bools(&[false, false, true])))), false);
    assert_eq!(ints.hashes[..2], [of(Value::Exact(Decimal::new(1, 2))), of(Value::Exact(Decimal::new(1, 0)))]);
    assert_eq!(ints.misses, [false, false, true]);
    let floats = hashed(Column::Floats(Floats::new(vec![1.0_f64, -0.0_f64], None)), false);
    assert_eq!(floats.hashes, [of(Value::Exact(Decimal::new(100, 2))), of(Value::Approx(0.0))]);
    let texts = hashed(Column::Texts(Texts::new("aa  ".into(), vec![1, 4], None)), false);
    assert_eq!(texts.hashes, [of(Value::Char("a".into())), of(Value::Char("a".into()))]);
    let values = hashed(Column::Values(vec![Value::Null, Value::Exact(Decimal::new(5, 0))]), true);
    assert_eq!(values.misses, [false, false], "a null matches a null");
    assert_eq!(values.hashes[1], of(Value::Approx(5.0)));
}

/// Verifies: [DESIGN] RULES.md D368 (MC/DC): keys compared in place match
/// as `key_equal` does: integers of scales apart rescaled, one's units
/// past 64 bits at the other's scale no match; texts less trailing
/// spaces; a null, or vectors of kinds apart, compared as values.
#[test]
fn keys_compared_in_place_match_as_values_do() {
    let one = |c: Column| Batch { columns: vec![c], rows: 1 };
    let cases = [
        (Column::Ints(Ints::new(vec![15], 1, None)), Column::Ints(Ints::new(vec![150], 2, None))),
        (Column::Ints(Ints::new(vec![15], 1, None)), Column::Ints(Ints::new(vec![151], 2, None))),
        (Column::Ints(Ints::new(vec![i64::MAX], 0, None)), Column::Ints(Ints::new(vec![1], 2, None))),
        (Column::Texts(Texts::new("ab ".into(), vec![3], None)), Column::Texts(Texts::new("ab".into(), vec![2], None))),
        (Column::Ints(Ints::new(vec![2], 0, None)), Column::Floats(Floats::new(vec![2.0_f64], None))),
        (Column::Ints(Ints::new(vec![0], 0, Some(Mask::from_bools(&[true])))), Column::Values(vec![Value::Null])),
    ];
    for (a, b) in cases {
        let (x, y) = (one(a), one(b));
        for nulls in [false, true] {
            let want = key_equal(&x.cell(0, 0).to_value(), &y.cell(0, 0).to_value(), nulls).unwrap();
            assert_eq!(keys_equal((&x, &[0], 0), (&y, &[0], 0), &[nulls]).unwrap(), want, "{:?} {:?} {nulls}", x.cell(0, 0), y.cell(0, 0));
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1098: a vector of integers of scale 0
/// hashes as its values' buckets do, on no kernels and the portable ones
/// (each set of the vector crate gives the portable hash of words), of
/// every row and of some, negatives and 2^51 less 1
/// among them; a null the hash of a null, a miss but on a key whose nulls
/// match; to a direct table, each its `f64`'s bits; and a vector holding
/// a value of 2^51 or more hashed as before.
#[test]
fn integers_of_scale_zero_hash_as_their_buckets() {
    let edge = (1_i64 << 51_u32) - 1;
    let units = vec![0_i64, 1, -1, 7, edge, -edge, 42, 0];
    let nulled = Mask::from_bools(&[false, false, false, false, false, false, false, true]);
    let batch = Batch { columns: vec![Column::Ints(Ints::new(units.clone(), 0, Some(nulled)))], rows: units.len() };
    let portable = crate::kernels::Portable;
    let given_kernels: &dyn Kernels = &portable;
    for kernels in [None, Some(given_kernels)] {
        for (rows, nulls) in [(None, false), (Some(&[6_usize, 1, 7, 2][..]), true)] {
            let how = Hashing { seed: &seed(), keyed: false, direct: false, kernels };
            let mut out = KeyHashes::default();
            out.of_batch((&batch, rows), (&[0], &[nulls]), how).unwrap();
            let given: Vec<usize> = rows.map_or_else(|| (0..units.len()).collect(), <[usize]>::to_vec);
            let want: Vec<u64> = given
                .iter()
                .map(|&r| if r == 7 { value_hash(&Value::Null, true, how).unwrap().unwrap() } else { of_units(units[r], how) })
                .collect();
            assert_eq!(out.hashes, want, "rows {rows:?}");
            assert_eq!(out.misses, given.iter().map(|&r| r == 7 && !nulls).collect::<Vec<_>>());
        }
    }
    let direct = Hashing { seed: &seed(), keyed: false, direct: true, kernels: None };
    let mut out = KeyHashes::default();
    out.of_batch((&batch, Some(&[3, 2])), (&[0], &[false]), direct).unwrap();
    assert_eq!(out.hashes, [7.0_f64.to_bits(), (-1.0_f64).to_bits()]);
    let wide = Batch { columns: vec![Column::Ints(Ints::new(vec![3_i64, 1_i64 << 51_u32, -(1_i64 << 60_u32)], 0, None))], rows: 3 };
    let how = Hashing { seed: &seed(), keyed: false, direct: false, kernels: Some(given_kernels) };
    out.of_batch((&wide, None), (&[0], &[false]), how).unwrap();
    assert_eq!(out.hashes, [of_units(3, how), of_units(1_i64 << 51_u32, how), of_units(-(1_i64 << 60_u32), how)]);
}

/// The hash of the integer `units` as a value's bucket.
fn of_units(units: i64, how: Hashing<'_>) -> u64 {
    value_hash(&Value::Exact(Decimal::new(i128::from(units), 0)), false, how).unwrap().unwrap()
}
