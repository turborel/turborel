//! The contract of a data type's kind, `SqlType` (RULES.md R-SAFE-3, D53),
//! checked for every built-in kind; a plug-in type can be checked the same
//! way.

use std::cmp::Ordering;
use std::sync::Arc;

use turborel::bytes::Reader;
use turborel::value::Spelled;
use turborel::value::datetime::{Datetime, Field, Moment, parse};
use turborel::value::interval::{self, Interval, Qualifier};
use turborel::value::types::Field as RowField;
use turborel::value::{Binary, DataType, DecFloat, Decimal, SqlType, Value, assign, bucket, compare};

/// Checks the contract of `t`'s kind on `t` and `values`, which are
/// assigned to `t` first.
pub(crate) fn type_contract(t: &DataType, values: &[Value]) {
    let kind: &dyn SqlType = t.kind();
    let values = codec_contract(t, values);
    for a in &values {
        let mut key = Vec::new();
        kind.encode_key(a, t, &mut key).unwrap();
        let mut rest = key.as_slice();
        let read = kind.decode_key(&mut rest, t).unwrap();
        assert_eq!(compare(&read, a), Some(Ordering::Equal), "{a}'s key reads back a value equal to it");
        assert!(rest.is_empty(), "{a}'s key is read whole");
        let (anchor, exact) = kind.anchor(a, t).unwrap();
        assert!(exact && compare(&anchor, a) == Some(Ordering::Equal) && anchor.to_literal() == a.to_literal(), "{a} is its own anchor");
        for b in &values {
            let mut other = Vec::new();
            kind.encode_key(b, t, &mut other).unwrap();
            assert_eq!(Some(key.cmp(&other)), compare(a, b), "the keys of {a} and {b} order as they compare");
            if compare(a, b) == Some(Ordering::Equal) {
                assert_eq!(bucket(a), bucket(b), "{a} and {b} share a bucket");
            }
        }
    }
}

/// Checks the contract of `t`'s kind on `t` and `values` but for keys and
/// anchors: its parameters, assignment, and value and record bytes. The
/// values, assigned to `t` first, are returned. Two values are the same
/// if they are written alike and compare equal, or unknown, as a row of a
/// null field does.
fn codec_contract(t: &DataType, values: &[Value]) -> Vec<Value> {
    let kind: &dyn SqlType = t.kind();
    assert!(kind.comparable(t), "{t} is comparable with itself");
    let mut params = Vec::new();
    kind.put_params(t, &mut params);
    assert_eq!(kind.params(&mut Reader::new(&params, "parameters")).unwrap(), t.clone(), "{t}'s parameters read back");
    let values: Vec<Value> = values.iter().map(|v| assign(v.clone(), t, "contract").unwrap()).collect();
    let same = |a: &Value, b: &Value| matches!(compare(a, b), Some(Ordering::Equal) | None) && a.to_literal() == b.to_literal();
    for a in &values {
        let again = assign(a.clone(), t, "contract").unwrap();
        assert!(same(&again, a), "assigning {a} to {t} again gives it");
        let mut bytes = Vec::new();
        kind.put_value(a, &mut bytes);
        assert!(same(&kind.value(&mut Reader::new(&bytes, "value")).unwrap(), a), "{a}'s bytes read back");
        let record = kind.encode_record(a, t).unwrap();
        if let Some(width) = kind.record_width(t) {
            assert_eq!(record.len(), width, "{a}'s record bytes are {t}'s width");
        }
        assert!(same(&kind.decode_record(&record, t).unwrap(), a), "{a}'s record bytes read back");
    }
    values
}

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: CHARACTER keeps the
/// `SqlType` contract: on strings empty, short, of trailing spaces, of
/// characters below the space and beyond ASCII.
#[test]
fn character_keeps_the_type_contract() {
    let strings = ["", "a", "a  ", "ab", "\t", "z", "é", "Ω€"];
    type_contract(&DataType::character(5), &strings.map(|s| Value::Char(s.into())));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR3a]
/// CHARACTER VARYING keeps the `SqlType` contract: on strings empty, of
/// spaces, of trailing spaces, of characters below and above the space,
/// beyond ASCII, longer than a key's chunk.
#[test]
fn varying_keeps_the_type_contract() {
    let strings = ["", "   ", "a", "a  ", "a\t", "a!", "ab", "\u{0}", "é", "Ω€", "abcdefgh", "abcdefghi", "abcdefgh  x", "abcdefgh\t"];
    type_contract(&DataType::varying(12), &strings.map(|s| Value::Char(s.into())));
}

/// Binary strings of the hexits of `hexits`.
fn octets<const N: usize>(hexits: [&str; N]) -> Vec<Value> {
    hexits.iter().map(|h| Value::Binary(Binary::of_hex(h).unwrap())).collect()
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075-2:2023 §8.2
/// GR5] BINARY keeps the `SqlType` contract: on binary strings of its
/// length, all X'00', all X'FF', X'00' octets amid others.
#[test]
fn binary_keeps_the_type_contract() {
    type_contract(&DataType::Binary { length: 3 }, &octets(["000000", "FFFFFF", "A50000", "000001", "00FF00"]));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075-2:2023 §8.2
/// GR5] BINARY VARYING keeps the `SqlType` contract: on binary strings
/// empty, of one octet, of X'00' octets trailing and leading, each
/// beginning others.
#[test]
fn binary_varying_keeps_the_type_contract() {
    let strings = ["", "00", "01", "0000", "0001", "FF", "FF00", "FF0000", "FFFF", "00FF", "A5A5A5A5"];
    type_contract(&DataType::VarBinary { length: 12 }, &octets(strings));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075-2:2023 §4.5.2,
/// §8.2 GR3] DECFLOAT keeps the `SqlType` contract: on 0, either sign, of
/// 34 digits, at the least and greatest exponents, and an exact number
/// assigned, which compares equal to the DECFLOAT value it gives.
#[test]
fn decfloat_keeps_the_type_contract() {
    let mut values: Vec<Value> = [(0, 0), (15, -1), (-15, -1), (1, -6143), (-1, -6143), (1, 6144), (i128::from(i64::MAX), 3)]
        .iter()
        .map(|&(c, e)| Value::DecFloat(DecFloat::new(c, e, 34).unwrap()))
        .collect();
    values.push(Value::DecFloat(DecFloat::new(9_999_999_999_999_999_999_999_999_999_999_999, 6111, 34).unwrap()));
    values.push(Value::Exact(Decimal::new(-12_345, 2)));
    type_contract(&DataType::DecFloat { precision: 34 }, &values);
}

/// The datetimes `texts` write, of `moment`, each of its own displacement
/// or of UTC.
fn datetimes<const N: usize>(moment: Moment, texts: [&str; N]) -> Vec<Value> {
    texts
        .iter()
        .map(|t| {
            let w = parse(moment, t).unwrap();
            Value::Datetime(Datetime::of_local(moment, w.local, w.zone.unwrap_or(0), w.zone.is_some(), w.precision))
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR5]
/// DATE keeps the `SqlType` contract: on the first and last days, a leap
/// day, days either side of 1970.
#[test]
fn date_keeps_the_type_contract() {
    type_contract(&DataType::Date, &datetimes(Moment::Date, ["0001-01-01", "1969-12-31", "1970-01-01", "2024-02-29", "9999-12-31"]));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR5]
/// TIME and TIMESTAMP, WITH TIME ZONE or not, keep the `SqlType`
/// contract: on midnight, fractions, displacements east and west, one
/// instant in two displacements.
#[test]
fn times_and_timestamps_keep_the_type_contract() {
    let times = ["00:00:00", "12:00:00.5", "23:59:59.999999"];
    type_contract(&DataType::Time { precision: 6, zoned: false }, &datetimes(Moment::Time, times));
    let zoned = ["00:00:00+00:00", "05:30:00+05:30", "12:00:00.25-12:59", "23:00:00+13:00"];
    type_contract(&DataType::Time { precision: 6, zoned: true }, &datetimes(Moment::Time, zoned));
    let stamps = ["0001-01-01 00:00:00", "1969-12-31 23:59:59.999999", "2024-02-29 12:34:56.789", "9999-12-31 23:59:59"];
    type_contract(&DataType::Timestamp { precision: 6, zoned: false }, &datetimes(Moment::Timestamp, stamps));
    let zoned_stamps = ["2024-01-01 05:30:00+05:30", "2024-01-01 00:00:00+00:00", "2023-12-31 23:00:00-02:00", "1970-01-01 00:00:00+13:00"];
    type_contract(&DataType::Timestamp { precision: 3, zoned: true }, &datetimes(Moment::Timestamp, zoned_stamps));
}

/// The intervals `texts` write, of qualifier `q`.
fn intervals<const N: usize>(q: Qualifier, texts: [&str; N]) -> Vec<Value> {
    texts.iter().map(|t| Value::Interval(Interval { units: interval::parse(&q, t).unwrap(), qualifier: q })).collect()
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR6]
/// INTERVAL keeps the `SqlType` contract, of each class: on intervals
/// negative, zero, positive, and at the leading field's ends.
#[test]
fn intervals_keep_the_type_contract() {
    let ym = Qualifier::new(Field::Year, Field::Month, 9, 0);
    type_contract(&DataType::Interval(ym), &intervals(ym, ["-999999999-11", "-1-00", "0-00", "0-01", "1-00", "999999999-11"]));
    let dt = Qualifier::new(Field::Day, Field::Second, 9, 6);
    let days = ["-999999999 23:59:59.999999", "-0 00:00:00.000001", "0 00:00:00", "1 12:00:00", "999999999 23:59:59.999999"];
    type_contract(&DataType::Interval(dt), &intervals(dt, days));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: NUMERIC keeps the `SqlType`
/// contract: on numbers negative, zero, positive, of fewer fraction digits
/// than the scale, at the precision's ends.
#[test]
fn exact_numbers_keep_the_type_contract() {
    let numbers = [exact(-999_999_999, 2), exact(-5, 1), exact(0, 0), exact(1, 2), exact(42, 0), exact(999_999_999, 2)];
    type_contract(&DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric }, &numbers);
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: FLOAT keeps the `SqlType`
/// contract, at double and single precision: on numbers of either sign,
/// both zeros, and the largest.
#[test]
fn approximate_numbers_keep_the_type_contract() {
    let numbers = [-f64::MAX, -1.5, -0.0, 0.0, 2.5e-300, 1.0e300, f64::MAX].map(Value::Approx);
    type_contract(&DataType::Approximate { precision: 53, spelled: Spelled::Float }, &numbers);
    type_contract(&DataType::Approximate { precision: 24, spelled: Spelled::Float }, &[-3.5, 0.0, 0.1, 1.0e30].map(Value::Approx));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: a type beyond the standard,
/// a plug-in's, keeps the `SqlType` contract: the tests' IDENT.
#[test]
fn a_plug_in_type_keeps_the_type_contract() {
    let ids = [[0_u8; 16], [1; 16], [0xff; 16], [0x7f; 16]].map(crate::ident::ident);
    type_contract(&crate::ident::ident_type(), &ids);
}

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

/// The array of `elements`.
fn array<const N: usize>(elements: [Value; N]) -> Value {
    Value::Array(Arc::from(elements))
}

/// The row of `fields`.
fn row<const N: usize>(fields: [Value; N]) -> Value {
    Value::Row(Arc::from(fields))
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53, D68: an array type keeps
/// the `SqlType` contract but for keys, as no array is in one [ISO/IEC
/// 9075-2:2023 §9.12 SR3]: on arrays empty, of one element, of a null
/// element, and nested, an array of arrays of character strings.
#[test]
fn arrays_keep_the_type_contract() {
    let t = DataType::array(integer(), 3);
    type_contract_of_arrays(&t, &[array([]), array([exact(1, 0)]), array([Value::Null, exact(2, 0), exact(-3, 0)])]);
    let nested = DataType::array(DataType::array(DataType::character(2), 2), 2);
    let strings = array([Value::Char("a".into()), Value::Null]);
    type_contract_of_arrays(&nested, &[array([strings, Value::Null]), array([array([])]), array([])]);
}

/// `codec_contract` of the array type `t` on `values`, none of which has a
/// key.
fn type_contract_of_arrays(t: &DataType, values: &[Value]) {
    for v in codec_contract(t, values) {
        assert!(t.kind().encode_key(&v, t, &mut Vec::new()).is_err(), "{v} has no key");
    }
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53, D68: a row type keeps the
/// `SqlType` contract, its keys ordered field by field [ISO/IEC
/// 9075-2:2023 §8.2 GR1b i]: on rows of fields equal in their first and
/// differing in their second, of a negative field, and nested; and but
/// for keys and anchors on rows of a null field, which compare with no
/// key.
#[test]
fn rows_keep_the_type_contract() {
    let field = |name: &str, data_type: DataType| RowField { name: name.into(), data_type, unnamed: false };
    let t = DataType::row(vec![field("X", integer()), field("Y", DataType::varying(5))]);
    let text = |s: &str| Value::Char(s.into());
    let values =
        [row([exact(1, 0), text("")]), row([exact(1, 0), text("a")]), row([exact(2, 0), text("a")]), row([exact(-5, 0), text("zz")])];
    type_contract(&t, &values);
    codec_contract(&t, &[row([Value::Null, text("a")]), row([exact(1, 0), Value::Null]), row([Value::Null, Value::Null])]);
    let nested = DataType::row(vec![field("A", DataType::row(vec![field("B", integer())])), field("C", integer())]);
    type_contract(&nested, &[row([row([exact(1, 0)]), exact(2, 0)]), row([row([exact(0, 0)]), exact(9, 0)])]);
}
