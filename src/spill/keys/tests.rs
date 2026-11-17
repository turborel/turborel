use super::*;
use crate::value::datetime::{Datetime, Field, Moment, parse};
use crate::value::interval::{Interval, Qualifier, unit};
use crate::value::lob::Lob;
use crate::value::{Binary, DecFloat, bucket};

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// The date `text`.
fn date(text: &str) -> Value {
    let w = parse(Moment::Date, text).unwrap();
    Value::Datetime(Datetime::of_local(Moment::Date, w.local, 0, false, 0))
}

/// The interval of `n` days.
fn days(n: i128) -> Value {
    Value::Interval(Interval { units: n.saturating_mul(unit(Field::Day)), qualifier: Qualifier::new(Field::Day, Field::Day, 9, 0) })
}

/// A value of each kind a key is made of, no two identical.
fn values() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Char("ab".into()),
        exact(5, 0),
        exact(15, 1),
        exact(i128::from(i64::MAX).saturating_add(1), 0),
        Value::Approx(2.5),
        Value::DecFloat(DecFloat::of_decimal(&Decimal::new(25, 1)).unwrap()),
        Value::Binary(Binary::new(vec![1, 2])),
        Value::Lob(Lob::of_text("held")),
        Value::Bool(true),
        date("2024-02-29"),
        days(3),
        Value::Array(vec![exact(1, 0), Value::Null].into()),
        Value::Row(vec![Value::Char("x".into())].into()),
        Value::Multiset(vec![exact(1, 0), exact(2, 0), exact(1, 0)].into()),
    ]
}

/// Verifies: [DESIGN] spill.rs: the key `norm_row` makes of values is the
/// key `norm_key` makes of their normalized values, of a value of each
/// kind and of all of them in a row; no two values not identical share
/// one.
#[test]
fn a_rows_key_is_the_key_of_its_normalized_values() {
    let all = values();
    for v in &all {
        assert_eq!(norm_row(std::slice::from_ref(v)), norm_key(&[v.norm()]), "{v:?}");
    }
    let normalized: Vec<NormKey> = all.iter().map(Value::norm).collect();
    assert_eq!(norm_row(&all), norm_key(&normalized));
    let keys: std::collections::HashSet<Vec<u8>> = all.iter().map(|v| norm_row(std::slice::from_ref(v))).collect();
    assert_eq!(keys.len(), all.len(), "each value its own key");
}

/// Verifies: [DESIGN] spill.rs [X3.135-1986 §5.11 GR5] values identical as
/// DISTINCT takes them share a key: a character string and it with
/// trailing spaces, an exact number at two scales, 0 and -0, and a
/// multiset's elements in any order; the key of each column part written
/// alone is that of its value.
#[test]
fn identical_values_share_a_key() {
    let key = |v: Value| norm_row(&[v]);
    assert_eq!(key(Value::Char("a  ".into())), key(Value::Char("a".into())));
    assert_eq!(key(exact(150, 2)), key(exact(15, 1)));
    assert_eq!(key(Value::Approx(-0.0)), key(Value::Approx(0.0)));
    assert_eq!(key(Value::Multiset(vec![exact(2, 0), exact(1, 0)].into())), key(Value::Multiset(vec![exact(1, 0), exact(2, 0)].into())));
    assert_ne!(key(Value::Multiset(vec![exact(1, 0)].into())), key(Value::Multiset(vec![exact(1, 0), exact(1, 0)].into())));
    let mut parts = Vec::new();
    put_norm_text("z ", &mut parts);
    put_norm_exact(70, 1, &mut parts);
    put_norm_approx(1.5, &mut parts);
    assert_eq!(parts, norm_row(&[Value::Char("z".into()), exact(7, 0), Value::Approx(1.5)]));
}

/// Verifies: [DESIGN] spill.rs: the key of buckets is the same bytes
/// exactly when the buckets are: of a bucket of each kind, the null one of
/// a key on which a null matches a null too, no two alike, and those of
/// equal values alike.
#[test]
fn bucket_keys_are_the_same_exactly_when_the_buckets_are() {
    let mut buckets: Vec<Bucket> = values().iter().filter_map(bucket).collect();
    buckets.push(Bucket::Null);
    buckets.push(Bucket::Extension(vec![1, 2].into()));
    let distinct: std::collections::HashSet<&Bucket> = buckets.iter().collect();
    let keys: std::collections::HashSet<Vec<u8>> = buckets.iter().map(|b| bucket_key(std::slice::from_ref(b))).collect();
    assert_eq!(keys.len(), distinct.len(), "each bucket its own key");
    let of = |v: Value| bucket_key(&[bucket(&v).unwrap()]);
    assert_eq!(of(exact(15, 1)), of(Value::Approx(1.5)), "a number's bucket");
    assert_eq!(of(Value::Char("a ".into())), of(Value::Char("a".into())));
    assert_eq!(of(Value::Multiset(vec![exact(2, 0), exact(1, 0)].into())), of(Value::Multiset(vec![exact(1, 0), exact(2, 0)].into())));
}
