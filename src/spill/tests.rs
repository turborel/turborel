//! Unit tests of normalized keys (`spill.rs`).

use super::{bucket_key, norm_key, put_norm};
use crate::value::datetime::{Field, Moment};
use crate::value::interval::{Interval, Qualifier};
use crate::value::types::unregistered;
use crate::value::{Bits, Datetime, Decimal, Value, bucket};

/// Verifies: [DESIGN] RULES.md D38, normalized keys: a value's key made in
/// place (`put_norm`) is the bytes of the key of its normalized value
/// (`norm_key`), for nulls, texts with and without trailing spaces, exact
/// numbers with and without trailing zeros, approximate zeros of either
/// sign and other numbers, and bit strings; and values the comparison
/// rules find equal have one key.
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
