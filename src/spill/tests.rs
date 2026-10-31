//! Unit tests of normalized keys, and of the bytes a row is counted as
//! holding (`spill.rs`).

use super::{bucket_key, heap_bytes, norm_key, put_norm, row_bytes};
use crate::value::datetime::{Field, Moment};
use crate::value::interval::{Interval, Qualifier};
use crate::value::types::unregistered;
use crate::value::{Bits, Datetime, Decimal, Value, bucket};

/// Verifies: [DESIGN] RULES.md D38, normalized keys: a value's key made in
/// place (`put_norm`) is the bytes of the key of its normalized value
/// (`norm_key`), for nulls, texts with and without trailing spaces, exact
/// numbers with and without trailing zeros, integers within 64 bits and
/// past them, approximate zeros of either sign and other numbers, and bit
/// strings; values the comparison rules find equal have one key; and an
/// integer within 64 bits keys by a fixed width.
#[test]
fn keys_made_in_place_are_normalized_keys() {
    let values = [
        Value::Null,
        Value::Char("ab".into()),
        Value::Char("ab   ".into()),
        Value::Char(String::new()),
        Value::Exact(Decimal::new(150, 2)),
        Value::Exact(Decimal::new(15, 1)),
        Value::Exact(Decimal::new(-7, 0)),
        Value::Approx(0.0),
        Value::Approx(-0.0),
        Value::Approx(1.5),
        Value::Bits(Bits::of_bits("101").unwrap()),
        Value::Exact(Decimal::new(70_i128, 1_u32)),
        Value::Exact(Decimal::new(i128::from(i64::MAX) + 1_i128, 0_u32)),
        Value::Exact(Decimal::new(i128::from(i64::MIN), 0_u32)),
    ];
    for v in &values {
        let mut made = Vec::new();
        put_norm(v, &mut made);
        assert_eq!(made, norm_key(&[v.norm()]), "{v:?}");
    }
    let key = |v: &Value| {
        let mut out = Vec::new();
        put_norm(v, &mut out);
        out
    };
    assert_eq!(key(&values[1]), key(&values[2]), "trailing spaces");
    assert_eq!(key(&values[4]), key(&values[5]), "trailing zeros");
    assert_eq!(key(&values[7]), key(&values[8]), "zeros of either sign");
    assert_ne!(key(&values[0]), key(&values[3]), "null is not empty");
    // An integer within 64 bits keys by those 8 bytes beside its tag, of
    // any scale it is written at; one past them by its units and scale.
    assert_eq!(key(&values[11]), key(&Value::Exact(Decimal::new(7_i128, 0_u32))), "7.0 is 7");
    assert_eq!((key(&values[6]).len(), key(&values[13]).len()), (9_usize, 9_usize), "integers of a fixed width");
    assert_eq!(key(&values[12]).len(), 21_usize, "past 64 bits: units and scale");
    assert_ne!(key(&values[12]), key(&Value::Exact(Decimal::new(i128::from(i64::MAX), 0_u32))));
    assert_ne!(key(&values[4]), key(&Value::Exact(Decimal::new(15_i128, 0_u32))), "1.5 is not 15");
}

/// Verifies: [DESIGN] RULES.md D38, normalized keys and buckets of the
/// other kinds: datetimes, intervals, bit strings and values of a kind
/// beyond the standard key as their normalized values do, and the keys of
/// the buckets of those that have them, as a hash join's spilled rows are
/// keyed, are alike of values alike and differ of values that differ.
#[test]
fn keys_of_every_kind() {
    let day_hour = Qualifier { start: Field::Day, end: Field::Hour, leading: 2, fraction: 0 };
    let values = [
        Value::Datetime(Datetime::of_local(Moment::Timestamp, 1_000_000, 0, false, 0)),
        Value::Datetime(Datetime::of_local(Moment::Timestamp, 5_000_000, 60, true, 6)),
        Value::Interval(Interval { units: 90, qualifier: day_hour }),
        Value::Interval(Interval { units: 91, qualifier: day_hour }),
        Value::Bits(Bits::of_bits("1101").unwrap()),
        unregistered("POINT", "(1 2)", &[1, 2]),
        unregistered("POINT", "(3 4)", &[3, 4]),
    ];
    for v in &values {
        let mut made = Vec::new();
        put_norm(v, &mut made);
        assert_eq!(made, norm_key(&[v.norm()]), "{v:?}");
    }
    // Of those with buckets: an unregistered kind gives none, so is never
    // hashed.
    let keyed: Vec<(usize, Vec<u8>)> = values.iter().enumerate().filter_map(|(n, v)| bucket(v).map(|b| (n, bucket_key(&[b])))).collect();
    assert!(keyed.len() >= 5, "datetimes, intervals and bit strings have buckets");
    for (n, key) in &keyed {
        assert_eq!(Some(key.clone()), bucket(&values[*n].clone()).map(|b| bucket_key(&[b])), "alike of {n}");
        assert!(keyed.iter().all(|(m, other)| m == n || other != key), "{n} differs of the others");
    }
}

/// Verifies: [DESIGN] RULES.md D38, a row's bytes: what a value holds
/// apart from itself counts toward its row's, as an operator bounds its
/// work area by them, of strings, bit strings and values of a kind beyond
/// the standard alike; the values that hold nothing apart count none.
/// Regression test (R-TEST-7): a bit string or a plug-in's value counted
/// as the size of a value alone, however many bytes it held, so rows of
/// them overran MEMORY.WORK_BYTES.
#[test]
fn rows_count_the_bytes_their_values_hold() {
    let long = "1".repeat(8000usize);
    let bits = Value::Bits(Bits::of_bits(&long).unwrap());
    assert_eq!(heap_bytes(&bits), 1000usize);
    let blob = unregistered("BLOB", "x", &[7u8; 4096]);
    let Value::Extension(e) = &blob else { panic!("an extension value") };
    let held = e.bytes.len();
    assert!(held >= 4096usize);
    assert_eq!(heap_bytes(&blob), held);
    assert_eq!(heap_bytes(&Value::Char("abc".into())), 3usize);
    let day_hour = Qualifier { start: Field::Day, end: Field::Hour, leading: 2, fraction: 0 };
    for v in
        [Value::Null, Value::Exact(Decimal::new(1, 0)), Value::Approx(1.0), Value::Interval(Interval { units: 1, qualifier: day_hour })]
    {
        assert_eq!(heap_bytes(&v), 0usize, "{v:?}");
    }
    let empty = row_bytes(&vec![Value::Null, Value::Null]);
    assert_eq!(row_bytes(&vec![bits, blob]), empty + 1000usize + held);
}
