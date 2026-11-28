//! Unit tests of the lanes of a table's keys: equal as the bytes `put_key`
//! gives are, of every type a lane holds and across them (RULES.md D356).

use super::*;
use crate::storage::mask::mask_of;
use crate::storage::{Column, Ints};
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

/// Keys of one lane, a group each of `units` at `scale`, a null of none.
fn ints_of(units: &[Option<i64>], scale: u32) -> Keys {
    let mut keys = Keys::new(1);
    for unit in units {
        keys.push(Key::Values(&[unit.map_or(Value::Null, |u| exact(i128::from(u), scale))]));
    }
    keys
}

/// Verifies: [DESIGN] (RULES.md D1274, D356) a lane of integers finds a
/// key equal by its words exactly as by its cells. Every group of a lane
/// of integers of scale 0 and of scale 2, a null among them, is compared
/// with every row of a batch's column of integers and every group of
/// another table's lane, of its scale and of the other, and each compare
/// agrees with that of the same value given as a value, which gives no
/// word, and with the keys' bytes. So an integer equals one of another
/// scale that is the same number, and a null equals a null alone, never
/// the zero that stands in its place. A lane whose first null is its
/// last group tells that group's null.
#[test]
fn integers_are_found_equal_by_their_words_as_by_their_cells() {
    let units = [Some(0), None, Some(100), Some(1), Some(10_000), Some(-7)];
    let mut scratch = Vec::new();
    for (mine, other) in [(0, 0), (2, 2), (0, 2), (2, 0)] {
        let (keys, theirs) = (ints_of(&units, mine), ints_of(&units, other));
        let nulls: Vec<bool> = units.iter().map(Option::is_none).collect();
        let column = Ints::new(units.iter().map(|u| u.unwrap_or(0)).collect(), other, mask_of(&nulls));
        let batch = Batch { columns: vec![Column::Ints(column)], rows: units.len() };
        for (g, r) in (0..units.len()).flat_map(|g| (0..units.len()).map(move |r| (g, r))) {
            let (held, given) = (keys.values(g), theirs.values(r));
            let expected = key_of(&held[0]) == key_of(&given[0]);
            assert_eq!(keys.equal(g, Key::Values(&given), &mut scratch), expected, "{held:?} against the value {given:?}");
            assert_eq!(keys.equal(g, Key::Row(&batch, &[0], r), &mut scratch), expected, "{held:?} against the row {given:?}");
            assert_eq!(keys.equal(g, Key::Group(&theirs, r), &mut scratch), expected, "{held:?} against the group {given:?}");
        }
    }
    let last = ints_of(&[Some(5), Some(0), None], 0);
    let zero = ints_of(&[Some(0), None], 0);
    assert!(!last.equal(2, Key::Group(&zero, 0), &mut scratch) && last.equal(2, Key::Group(&zero, 1), &mut scratch));
    assert!(last.equal(1, Key::Group(&zero, 0), &mut scratch) && !last.equal(1, Key::Group(&zero, 1), &mut scratch));
}

/// Verifies: [DESIGN] (RULES.md D1323, D1113) [X3.135-1986 §5.11 GR4]
/// the hash of a group's key of some of its lanes is the hash of those
/// values, whichever way a lane reads its own: an integer of no fraction
/// by its word, a null among them and the least integer of 64 bits; a
/// number of two fractional digits, a text and a null by their cells.
/// So a group goes to the partition its values hash to, and an integer
/// hashes as the same number held with fractional digits.
#[test]
fn a_groups_hash_of_its_lanes_is_that_of_their_values() {
    let seed = Seed::fixed(0x5EED);
    let rows = [
        vec![exact(7, 0), exact(150, 2), Value::Char("ab ".into())],
        vec![Value::Null, exact(-3, 2), Value::Null],
        vec![exact(i128::from(i64::MIN), 0), Value::Null, Value::Char(String::new().into())],
        vec![exact(70, 0), exact(700, 2), Value::Char("ab".into())],
    ];
    let mut keys = Keys::new(3);
    for row in &rows {
        keys.push(Key::Values(row));
    }
    for (g, row) in rows.iter().enumerate() {
        for lanes in [&[0_usize][..], &[1], &[0, 1], &[2, 0], &[0, 1, 2]] {
            let values: Vec<Value> = lanes.iter().map(|&at| row[at].clone()).collect();
            assert_eq!(keys.hash_of(g, lanes, &seed), crate::exec::table::hash_values(&seed, &values), "group {g} by {lanes:?}");
        }
    }
    assert_eq!(keys.hash_of(0, &[0], &seed), keys.hash_of(3, &[1], &seed));
    assert_ne!(keys.hash_of(0, &[0], &seed), keys.hash_of(3, &[0], &seed));
}
