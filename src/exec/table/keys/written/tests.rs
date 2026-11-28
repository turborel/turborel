//! Unit tests of a table's keys written to a spill: each lane read back
//! as it was, its vector's bits, nulls and unpadded flag (RULES.md D1226).

use super::*;
use crate::exec::table::Key;
use crate::value::Decimal;
use crate::value::datetime::Moment;

/// The exact number of `units` at `scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// The keys of rows of `columns` values each, the `n`th of `value(n, c)`.
fn keys_of(rows: usize, columns: usize, value: impl Fn(usize, usize) -> Value) -> Keys {
    let mut keys = Keys::new(columns);
    for n in 0..rows {
        let row: Vec<Value> = (0..columns).map(|c| value(n, c)).collect();
        keys.push(Key::Values(&row));
    }
    keys
}

/// What a lane holds, every bit of it, as a string.
fn held(lane: &Lane) -> String {
    format!("{} {:?} {:?}", lane.unpadded, lane.nulls, lane.data)
}

/// The value of column `c` of row `n`: of each type a lane holds, a null
/// for every 7th row from the 3rd.
fn value(n: usize, c: usize) -> Value {
    let i = i64::try_from(n).unwrap_or(0);
    if n % 7 == 3 {
        return Value::Null;
    }
    match c {
        0 => exact(i128::from(i).saturating_mul(25), 2),
        1 => Value::Approx(if n.is_multiple_of(5) {
            -0.0
        } else if n.is_multiple_of(11) {
            f64::NAN
        } else {
            f64::from(u32::try_from(n).unwrap_or(0)) / 3.0
        }),
        2 => Value::Datetime(Datetime {
            moment: Moment::Timestamp,
            utc: i.saturating_mul(1_000_003),
            zone: -90,
            zoned: n.is_multiple_of(2),
            precision: 3,
        }),
        3 => Value::Bool(n.is_multiple_of(3)),
        4 => Value::Char(format!("k{n}{}", " ".repeat(n % 3)).into()),
        5 => exact(if n == 0 { i128::MAX } else { i128::from(i) }, 0),
        _ => Value::Null,
    }
}

/// Verifies: [DESIGN] (RULES.md D1226) keys of every type a lane holds,
/// integers of a scale, approximate numbers of -0 and NaN, datetimes in
/// zones, truth values, texts with trailing spaces unpadded, numbers wider
/// than 64 bits, a lane of nulls alone, and nulls in each, written in
/// pieces and read back, are the lanes written, bit for bit.
#[test]
fn every_lane_is_read_back_as_written() {
    let mut keys = keys_of(300, 7, value);
    if let Some(lane) = keys.lanes.get_mut(4) {
        lane.unpadded = true;
    }
    let places: Vec<u32> = (0..300).collect();
    let mut read: Option<Keys> = None;
    for piece in places.chunks(128) {
        let mut out = Vec::new();
        assert!(keys.put_groups(piece, &mut out));
        let mut reader = Reader::new(&out, "keys");
        let back = Keys::read(&mut reader, piece.len()).unwrap();
        reader.finish().unwrap();
        let empty = || Keys { lanes: back.lanes.iter().map(|lane| Lane { unpadded: lane.unpadded, ..Lane::default() }).collect() };
        let whole = read.get_or_insert_with(empty);
        whole.extend(&back, &(0..u32::try_from(piece.len()).unwrap()).collect::<Vec<_>>());
    }
    let read = read.unwrap();
    assert!(matches!(keys.lanes.get(5).map(|l| &l.data), Some(Data::Values { .. })), "a number past 64 bits made values");
    assert!(matches!(keys.lanes.get(6).map(|l| &l.data), Some(Data::Unset)), "a lane of nulls alone");
    for (c, (lane, back)) in keys.lanes.iter().zip(&read.lanes).enumerate() {
        assert_eq!(held(back), held(lane), "lane {c}");
    }
    let mut out = Vec::new();
    assert!(keys.put_groups(&[5, 9], &mut out));
    let back = Keys::read(&mut Reader::new(&out, "keys"), 2).unwrap();
    let shown = |k: &Keys, g: usize| format!("{:?}", k.values(g));
    assert_eq!((shown(&back, 0), shown(&back, 1)), (shown(&keys, 5), shown(&keys, 9)), "the groups at the places, in their order");
}

/// Verifies: [DESIGN] (RULES.md D1226) a lane of values that holds a value
/// that holds others writes nothing, so its groups stay where they are.
#[test]
fn a_value_that_holds_others_is_not_written() {
    let keys = keys_of(3, 1, |n, _| if n == 1 { Value::Row(vec![exact(1, 0)].into()) } else { exact(i128::MAX, 0) });
    assert!(!keys.put_groups(&[0, 1, 2], &mut Vec::new()));
    assert!(keys.put_groups(&[0, 2], &mut Vec::new()));
}
