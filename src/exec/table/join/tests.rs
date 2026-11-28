//! Unit tests of a join's table: its slots and tags, the order of a key's
//! rows, the hashes of each kind of vector against those of its values,
//! and its keys compared in place against `key_equal` (RULES.md D368,
//! D369, R-TEST-1, R-TEST-8).

use super::keys::{nearest, value_hash};
use super::*;
use crate::exec::table::Seed;
use crate::kernels::Kernels;
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
/// a value of 2^51 or more hashes as its values do too (RULES.md D1288).
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

/// Verifies: [DESIGN] RULES.md D1288, whole numbers apart hash apart:
/// 1,048,576 keys one after another from 9,000,000,000,000,000,000, whose
/// nearest `f64`s are 1,025, have 1,048,576 hashes; 4,096 keys from each
/// of that number, 2^51, the least and near the greatest 64 bits hold and
/// across 2^53 have 4,096 hashes, as a vector of integers on no kernels
/// and on the portable ones, and as values; and each hashes as the
/// same number written at scale 2, as a decimal floating-point number
/// and, where an `f64` is it exactly, as an approximate number, in a
/// vector and as a value.
#[test]
fn whole_numbers_apart_hash_apart() {
    let portable = crate::kernels::Portable;
    let given_kernels: &dyn Kernels = &portable;
    let plain = Hashing { seed: &seed(), keyed: false, direct: false, kernels: None };
    let of = |v: Value| value_hash(&v, false, plain).unwrap().unwrap();
    let million: Vec<i64> = (0..1_048_576_i64).map(|i| 9_000_000_000_000_000_000_i64.wrapping_add(i)).collect();
    let mut hashes = hashed(Column::Ints(Ints::new(million, 0, None)), false).hashes;
    hashes.sort_unstable();
    hashes.dedup();
    assert_eq!(hashes.len(), 1_048_576, "a hash a key");
    for base in [9_000_000_000_000_000_000_i64, 1_i64 << 51_u32, i64::MIN, i64::MAX.wrapping_sub(4095), (1_i64 << 53_u32) - 2048] {
        let units: Vec<i64> = (0..4096_i64).map(|i| base.wrapping_add(i)).collect();
        let batch = Batch { columns: vec![Column::Ints(Ints::new(units.clone(), 0, None))], rows: units.len() };
        let mut out = KeyHashes::default();
        out.of_batch((&batch, None), (&[0], &[false]), plain).unwrap();
        let distinct: std::collections::BTreeSet<u64> = out.hashes.iter().copied().collect();
        assert_eq!(distinct.len(), 4096, "from {base}");
        let mut vectored = KeyHashes::default();
        vectored.of_batch((&batch, None), (&[0], &[false]), Hashing { kernels: Some(given_kernels), ..plain }).unwrap();
        assert_eq!(vectored.hashes, out.hashes, "on the kernels, from {base}");
        for (&u, &h) in units.iter().zip(&out.hashes).step_by(97) {
            let wide = i128::from(u);
            assert_eq!(of(Value::Exact(Decimal::new(wide, 0))), h, "{u} as a value");
            assert_eq!(of(Value::Exact(Decimal::new(wide.wrapping_mul(100), 2))), h, "{u} at scale 2");
            assert_eq!(of(Value::DecFloat(crate::value::DecFloat::of_decimal(&Decimal::new(wide, 0)).unwrap())), h, "{u} as a DECFLOAT");
        }
    }
    let exact: Vec<i64> =
        vec![1_i64 << 51_u32, (1_i64 << 52_u32) + 1, -(1_i64 << 53_u32) - 2, 1_i64 << 62_u32, i64::MIN, (1_i64 << 62_u32) + 1024];
    let numbers: Vec<f64> = vec![
        2_251_799_813_685_248.0_f64,
        4_503_599_627_370_497.0_f64,
        -9_007_199_254_740_994.0_f64,
        2.0_f64.powi(62),
        -(2.0_f64.powi(63)),
        2.0_f64.powi(62) + 1024.0_f64,
    ];
    let ints = hashed(Column::Ints(Ints::new(exact.clone(), 0, None)), false);
    let floats = hashed(Column::Floats(Floats::new(numbers.clone(), None)), false);
    assert_eq!(floats.hashes, ints.hashes, "an approximate number that is a whole number of 64 bits");
    assert_eq!(numbers.iter().map(|&f| of(Value::Approx(f))).collect::<Vec<_>>(), ints.hashes);
}

/// Verifies: [DESIGN] RULES.md D1288, a whole number's word, as a direct
/// table is given it: below 2^51 the bits of its `f64`, from 2^51 its 64
/// bits crossed with a null's number, so no key's word is a null's, of
/// the key whose bits are a null's number too, and the hashes a build
/// makes of the words are those a probe makes; and the one wide key whose
/// word is a small number's, 5's, shares it and does not match it, while
/// the key after it has a word of its own.
#[test]
fn a_wide_keys_word_is_no_nulls_and_strays_to_one_small_key_at_most() {
    let plain = Hashing { seed: &seed(), keyed: false, direct: false, kernels: None };
    let direct = Hashing { direct: true, ..plain };
    let nulls_number = i64::from_le_bytes(0x7FF8_0000_0000_0E11_u64.to_le_bytes());
    let wide = Batch { columns: vec![Column::Ints(Ints::new(vec![nulls_number, 5, i64::MAX], 0, None))], rows: 3 };
    let mut words = KeyHashes::default();
    words.of_batch((&wide, None), (&[0], &[false]), direct).unwrap();
    assert_eq!(words.hashes[1], 5.0_f64.to_bits(), "a narrow key's word is its number's bits");
    let edge = Batch { columns: vec![Column::Ints(Ints::new(vec![(1_i64 << 51_u32) - 1, 1_i64 << 51_u32], 0, None))], rows: 2 };
    let mut edges = KeyHashes::default();
    edges.of_batch((&edge, None), (&[0], &[false]), direct).unwrap();
    assert_eq!(edges.hashes, [2_251_799_813_685_247.0_f64.to_bits(), (1_u64 << 51_u32) ^ 0x7FF8_0000_0000_0E11_u64], "by itself from 2^51");
    assert!(words.hashes.iter().all(|&w| w != 0x7FF8_0000_0000_0E11_u64), "no key's word is a null's: {:x?}", words.hashes);
    assert_eq!(numbers_hashed(&words.hashes, &seed()), [of_units(nulls_number, plain), of_units(5, plain), of_units(i64::MAX, plain)]);
    // A wide key's 64 bits, crossed, may be one small number's `f64`: a stray pair, told apart as keys are compared.
    let stray = i64::from_le_bytes((5.0_f64.to_bits() ^ 0x7FF8_0000_0000_0E11_u64).to_le_bytes());
    let pair = Batch { columns: vec![Column::Ints(Ints::new(vec![stray, 5, stray.wrapping_add(1)], 0, None))], rows: 3 };
    words.of_batch((&pair, None), (&[0], &[false]), direct).unwrap();
    assert!(stray > 1_i64 << 51_u32 && words.hashes[0] == words.hashes[1] && words.hashes[2] != words.hashes[1], "{:x?}", words.hashes);
    assert!(!keys_equal((&pair, &[0], 0), (&pair, &[0], 1), &[false]).unwrap(), "compared, they differ");
}

/// Verifies: [DESIGN] RULES.md D1288, what keys by its nearest `f64`:
/// an approximate number with a half below 2^52, one of 2^63 and one
/// past it, an infinity and a not-a-number hash by their bits, as an
/// exact number of a fraction or past 64 bits equal to one of them does;
/// and an exact number and an approximate one that differ past the 53
/// bits an `f64` keeps do not match, so their hashes need not. MC/DC of
/// a whole `f64` of 64 bits: its magnitude below 2^51 (its bits), from
/// 2^51 with a half (its bits), whole (the number), 2^63 negative (the
/// number), 2^63 positive (its bits).
#[test]
fn numbers_no_whole_number_of_64_bits_hash_by_their_nearest_f64() {
    let how = Hashing { seed: &seed(), keyed: false, direct: true, kernels: None };
    let word = |v: Value| value_hash(&v, false, how).unwrap().unwrap();
    let bits = |f: f64| word(Value::Approx(f));
    let half = 2_251_799_813_685_248.5_f64;
    let past = 2.0_f64.powi(63);
    for f in [half, 1.5_f64, past, 1e300_f64, -1e19_f64, f64::INFINITY, 0.1_f64, 3.0_f64] {
        assert_eq!(bits(f), f.to_bits(), "{f}");
    }
    assert_eq!(bits(-0.0_f64), 0.0_f64.to_bits(), "-0 as 0");
    assert_eq!(bits(f64::NAN), f64::NAN.to_bits());
    assert_ne!(bits(4_503_599_627_370_496.0_f64), 4_503_599_627_370_496.0_f64.to_bits(), "a whole number from 2^51 keys by itself");
    assert_eq!(bits(-past), word(Value::Exact(Decimal::new(i128::from(i64::MIN), 0))));
    assert_eq!(word(Value::Exact(Decimal::new(i128::from(i64::MAX).wrapping_add(1), 0))), past.to_bits(), "past 64 bits");
    assert_eq!(word(Value::Exact(Decimal::new(22_517_998_136_852_485, 1))), half.to_bits(), "a fraction");
    assert_eq!(word(Value::Exact(Decimal::new(10_i128.pow(37), 0))), 1e37_f64.to_bits(), "38 digits");
    let (odd, even) = (Value::Exact(Decimal::new(9_007_199_254_740_993, 0)), Value::Approx(9_007_199_254_740_992.0_f64));
    assert!(!key_equal(&odd, &even, false).unwrap(), "they differ by algebraic value");
    assert_ne!(word(odd), word(even.clone()));
    assert_eq!(word(Value::Exact(Decimal::new(9_007_199_254_740_992, 0))), word(even), "and the one equal to it keys as it");
}
