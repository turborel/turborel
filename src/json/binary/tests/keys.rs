//! Tests of the comparison keys of the scalars the JSON text cannot
//! write (datetimes, infinities and NaN) and of keys that are damaged.

use std::cmp::Ordering;

use super::*;
use crate::value::Datetime;
use crate::value::datetime::Moment;

/// The datetime `utc` microseconds of `moment`, of no time zone.
fn datetime(moment: Moment, utc: i64) -> Item {
    scalar(Value::Datetime(Datetime { moment, utc, zone: 0, zoned: false, precision: 6 }))
}

/// The comparison key of `item`.
fn key_of(item: &Item) -> Vec<u8> {
    document_key(&encode(item).unwrap()).unwrap()
}

/// The item whose key is the whole of `key`.
fn item_of(key: &[u8]) -> Item {
    let mut rest = key;
    let item = item_of_key(&mut rest).unwrap();
    assert!(rest.is_empty(), "{key:?}");
    item
}

/// The error of reading an item of the key `key`.
fn refusal(key: &[u8]) -> SqlError {
    let mut rest = key;
    item_of_key(&mut rest).unwrap_err()
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR10b viii] datetimes are after
/// truth values; times before dates and timestamps, a date compared as
/// the timestamp of its midnight, each by its instant (IA229); a key
/// decodes to an equivalent item.
#[test]
fn datetimes_order_by_class_then_instant() {
    let day = 86_400_000_000_i64;
    let ascending = [
        scalar(Value::Bool(true)),
        datetime(Moment::Time, 0),
        datetime(Moment::Time, 3_600_000_000),
        datetime(Moment::Timestamp, -day),
        datetime(Moment::Date, 0),
        datetime(Moment::Timestamp, 1),
        datetime(Moment::Date, day),
    ];
    for pair in ascending.windows(2) {
        assert_eq!(key_of(&pair[0]).cmp(&key_of(&pair[1])), Ordering::Less, "{pair:?}");
    }
    assert_eq!(key_of(&datetime(Moment::Date, day)), key_of(&datetime(Moment::Timestamp, day)));
    for item in &ascending {
        let back = item_of(&key_of(item));
        assert!(back.equivalent(item).unwrap(), "{item:?}");
    }
    assert!(decode(&encode(&ascending[2]).unwrap()).unwrap().equivalent(&ascending[2]).unwrap());
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR10b v] an approximate number's
/// infinities order below and above every finite number, NaN above both
/// (IA228); zero is one key whatever its kind; each key decodes back.
#[test]
fn non_finite_numbers_order_at_the_ends() {
    let ascending = [
        scalar(Value::Approx(f64::NEG_INFINITY)),
        scalar(Value::Approx(-1e300)),
        scalar(Value::Approx(0.0)),
        scalar(Value::Approx(1e300)),
        scalar(Value::Approx(f64::INFINITY)),
        scalar(Value::Approx(f64::NAN)),
    ];
    for pair in ascending.windows(2) {
        assert_eq!(key_of(&pair[0]).cmp(&key_of(&pair[1])), Ordering::Less, "{pair:?}");
    }
    assert_eq!(key_of(&scalar(Value::Exact(Decimal::new(0, 3)))), key_of(&ascending[2]));
    for item in &ascending {
        assert_eq!(key_of(&item_of(&key_of(item))), key_of(item), "{item:?}");
    }
    let Item::Scalar(nan) = item_of(&key_of(&ascending[5])) else { panic!("not a scalar") };
    assert!(nan.value().as_f64().is_some_and(f64::is_nan));
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: a key cut short anywhere is
/// refused with SQLCODE -916, never read as a shorter item, since keys
/// are prefix-free.
#[test]
fn every_key_prefix_is_refused() {
    let items = [
        parse("{\"b\":[1,-2.5,\"x\\u0000y\"],\"a\":{\"c\":null,\"d\":false}}", Keys::Any).unwrap(),
        datetime(Moment::Time, 5),
        scalar(Value::Approx(f64::INFINITY)),
    ];
    for item in &items {
        let key = key_of(item);
        for cut in 0..key.len() {
            assert_eq!(refusal(&key[..cut]).sqlcode(), -916_i32, "{item:?} cut at {cut}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: octets that no key holds
/// are refused with SQLCODE -916: an unknown tag, a truth value other
/// than 0 or 1, a string's 0 not escaped nor ending it, a string not
/// UTF-8, a number of no sign or of no digits, an instant cut short.
#[test]
fn damaged_keys_are_refused() {
    for key in [
        &[0_u8][..],
        &[8],
        &[4, 2],
        &[3, b'a', 0, 7],
        &[3, 0xC3, 0, 0],
        &[2, 6],
        &[2, 3, 0, 0, 0, 1, 0],
        &[2, 3, 0, 0, 0, 1, b'x', 0],
        &[5, 0, 1, 2],
    ] {
        assert_eq!(refusal(key).sqlcode(), -916_i32, "{key:?}");
    }
    let Item::Scalar(truth) = item_of(&[4, 0]) else { panic!("not a scalar") };
    assert!(matches!(truth.value(), Value::Bool(false)));
}
