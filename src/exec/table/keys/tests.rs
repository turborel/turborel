//! Unit tests of the lanes of a table's keys: equal as the bytes `put_key`
//! gives are, of every type a lane holds and across them (RULES.md D356).

use super::*;
use crate::value::Decimal;

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// A datetime of `utc`, displaced by `zone` minutes, with a zone.
fn instant(utc: i64, zone: i16) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc, zone, zoned: true, precision: 0 })
}

/// The values a lane is tried with: of each type a lane holds, the same
/// value held two ways, and nulls.
fn tried() -> Vec<Value> {
    vec![
        exact(1, 0),
        exact(10, 1),
        exact(100, 2),
        exact(2, 0),
        exact(i128::from(i64::MAX).saturating_add(1), 0),
        Value::Approx(0.0),
        Value::Approx(-0.0),
        Value::Approx(1.0),
        Value::Approx(f64::NAN),
        instant(60_000_000, 0),
        instant(60_000_000, 120),
        instant(0, 0),
        Value::Bool(true),
        Value::Bool(false),
        Value::Char("ab".into()),
        Value::Char("ab   ".into()),
        Value::Char("ab c".into()),
        Value::Char(String::new().into()),
        Value::Null,
    ]
}

/// The bytes `put_key` gives of `v`.
fn key_of(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    put_key(Cell::Value(v), &mut out);
    out
}

/// Verifies: [DESIGN] (RULES.md D356) a lane holding any value finds it
/// equal to another exactly as their keys' bytes are equal: 1, 1.0 and
/// 1.00 alike, -0 as 0, two displacements of one instant alike, texts
/// without their trailing spaces, a null equal to a null alone, values of
/// two types never; and gives each group's value as it was first given.
#[test]
fn lanes_equal_as_their_keys_bytes() {
    let values = tried();
    let mut scratch = Vec::new();
    for first in &values {
        let mut lane = Lane::default();
        lane.push(Cell::Value(first));
        assert_eq!(format!("{:?}", lane.value(0)), format!("{first:?}"), "kept as given");
        for other in &values {
            let equal = lane.equal(0, Cell::Value(other), &mut scratch);
            assert_eq!(equal, key_of(first) == key_of(other), "{first:?} against {other:?}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D356) a lane of integers given one of
/// another scale, or a value of another type, turns into values, its
/// groups kept and found as before; a lane of nulls first takes the type
/// of the first value not null.
#[test]
fn a_lane_widens_to_values_once() {
    let mut scratch = Vec::new();
    let mut lane = Lane::default();
    for v in [Value::Null, exact(5, 0), exact(150, 2), Value::Char("x".into())] {
        lane.push(Cell::Value(&v));
    }
    assert!(matches!(lane.data, Data::Values { .. }));
    assert!(lane.equal(0, Cell::Null, &mut scratch));
    assert!(lane.equal(1, Cell::Exact(5, 0), &mut scratch));
    assert!(lane.equal(1, Cell::Exact(500, 2), &mut scratch));
    assert!(lane.equal(2, Cell::Exact(15, 1), &mut scratch));
    assert!(lane.equal(3, Cell::Text("x  "), &mut scratch));
    assert_eq!(format!("{:?}", lane.value(2)), format!("{:?}", exact(150, 2)));
    let mut ints = Lane::default();
    ints.push(Cell::Null);
    ints.push(Cell::Exact(7, 0));
    assert!(matches!(ints.data, Data::Ints { scale: 0, .. }));
    assert!(ints.equal(0, Cell::Null, &mut scratch) && !ints.equal(1, Cell::Null, &mut scratch));
}

/// Verifies: [DESIGN] (RULES.md D356) a lane kept to its first groups
/// gives back the room of the others, texts and values too, and holds the
/// rest as they were.
#[test]
fn a_lane_truncated_keeps_its_first_groups() {
    let mut scratch = Vec::new();
    for values in [vec!["a", "bb", "ccc"], vec!["a", "bb", "ccc"]] {
        let mut texts = Lane::default();
        for t in &values {
            texts.push(Cell::Text(t));
        }
        texts.truncate(2);
        assert!(texts.equal(1, Cell::Text("bb"), &mut scratch));
        texts.push(Cell::Text("d"));
        assert!(texts.equal(2, Cell::Text("d"), &mut scratch));
    }
    let mut held = Lane::default();
    held.push(Cell::Value(&Value::Approx(1.5)));
    held.push(Cell::Value(&exact(1, 0)));
    held.push(Cell::Value(&Value::Char("long text".into())));
    let before = held.bytes();
    held.truncate(1);
    assert!(held.bytes() < before);
    assert!(held.equal(0, Cell::Approx(1.5), &mut scratch));
}
