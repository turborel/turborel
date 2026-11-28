//! Unit tests of the bodies over vectors of the functions of datetimes
//! (`datetimes.rs`, RULES.md D1320): each field and each cast of a vector
//! against `Datetime::field` and `cast_at` of each of its values, over
//! the edges of the types and a spread of a fixed seed. No datetime holds
//! a leap second (`value/datetime.rs`), so none is among them.

use super::*;
use crate::storage::Mask;
use crate::value::datetime::{Datetime, LAST_DATE, MAX_DISPLACEMENT, MIN_DISPLACEMENT, days_from_civil};
use crate::value::types::datetime::cast_at;
use crate::value::{DataType, Value};

const DAY: i64 = MICROS_PER_DAY;
const HOUR: i64 = MICROS_PER_HOUR;

/// The displacements vectors carry: none, a minute either side, hours,
/// half hours, the least and the greatest a session takes, and the least
/// and the greatest 16 bits hold.
const ZONES: [i16; 11] = [0, 1, -1, 60, -61, 330, -210, MIN_DISPLACEMENT, MAX_DISPLACEMENT, i16::MIN, i16::MAX];

/// The datetime types vectors are of and are cast to: fields, precision,
/// and whether WITH TIME ZONE.
const SHAPES: [(Moment, u32, bool); 7] = [
    (Moment::Date, 0, false),
    (Moment::Time, 0, false),
    (Moment::Time, 6, true),
    (Moment::Timestamp, 6, false),
    (Moment::Timestamp, 0, false),
    (Moment::Timestamp, 3, true),
    (Moment::Timestamp, 6, true),
];

/// The midnight of a day of the calendar, in microseconds.
fn midnight(year: i64, month: i64, day: i64) -> i64 {
    days_from_civil(year, month, day).saturating_mul(DAY)
}

/// `micros` moved by `by`.
fn moved(micros: i64, by: i64) -> i64 {
    micros.checked_add(by).unwrap()
}

/// Microseconds at the edges of the words and of the types: the first
/// instant and the one before it, the ends of a minute, an hour and a day
/// either side of 0, the last date and instant of 9999 and the ones past
/// them, the least and the greatest 64 bits hold.
const EDGES: [i64; 26] = [
    0,
    1,
    -1,
    59_999_999,
    60_000_000,
    HOUR - 1,
    HOUR,
    DAY - 1,
    DAY,
    DAY + 1,
    -DAY,
    -DAY - 1,
    -HOUR,
    14 * HOUR,
    13 * HOUR - 1,
    LAST_DATE - 1,
    LAST_DATE,
    LAST_INSTANT - 14 * HOUR,
    LAST_INSTANT - 1,
    LAST_INSTANT,
    LAST_INSTANT + 1,
    LAST_INSTANT + 13 * HOUR,
    i64::MIN,
    i64::MIN + 1,
    i64::MAX - 1,
    i64::MAX,
];

/// An afternoon's time of day, a microsecond before its minute ends.
const AFTERNOON: i64 = 13 * HOUR + 47 * MICROS_PER_MINUTE + 59_999_999;

/// The edges; then the microsecond before, the midnight, the microsecond
/// after and an afternoon of the epoch of 1970, the leap day of 2000 and
/// the day after it, the day 1900 has no leap day before, a century's
/// first day, and the first days of the years 0002 and 9999; then a
/// spread from two days before the first instant to two days past the
/// last.
fn instants() -> Vec<i64> {
    let days = [(1970, 1, 1), (2000, 2, 29), (2000, 3, 1), (1900, 3, 1), (2000, 1, 1), (2, 1, 1), (9999, 1, 1)];
    let mut all = EDGES.to_vec();
    for at in days.map(|(year, month, day)| midnight(year, month, day)) {
        all.extend([moved(at, -1), at, moved(at, 1), moved(at, AFTERNOON)]);
    }
    let span = u64::try_from(LAST_INSTANT + 4 * DAY).unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..240_u32 {
        state ^= state << 13_u32;
        state ^= state >> 7_u32;
        state ^= state << 17_u32;
        all.push(moved(i64::try_from(state.checked_rem(span).unwrap()).unwrap(), -2 * DAY));
    }
    all
}

/// Vectors of `shape` of the instants, `rows` of them each: with no
/// displacements, and with each of them in turn; without nulls, and with
/// every third row null.
fn vectors((moment, precision, in_zone): (Moment, u32, bool), rows: usize) -> Vec<Datetimes> {
    let mut made = Vec::new();
    for (at, chunk) in instants().chunks(rows).enumerate() {
        let zones: Vec<i16> = (0..chunk.len()).map(|n| ZONES[at.saturating_add(n).checked_rem(ZONES.len()).unwrap()]).collect();
        let nulls = Mask::from_bools(&(0..chunk.len()).map(|n| at.saturating_add(n).checked_rem(3) == Some(0)).collect::<Vec<_>>());
        let shape = (moment, in_zone, u8::try_from(precision).unwrap());
        made.push(Datetimes::new(chunk.to_vec(), None, shape, None));
        made.push(Datetimes::new(chunk.to_vec(), Some(zones.clone()), shape, None));
        made.push(Datetimes::new(chunk.to_vec(), Some(zones), shape, Some(nulls)));
    }
    made
}

/// The field `field` of `d`, SECOND in units of `unit`, as EXTRACT's call
/// reads it.
fn field_of(d: &Datetime, field: ExtractField, unit: i64) -> Option<i64> {
    match field {
        ExtractField::TimezoneHour => d.zone.checked_div(60).map(i64::from),
        ExtractField::TimezoneMinute => d.zone.checked_rem(60).map(i64::from),
        ExtractField::Datetime(Field::Second) => d.field(Field::Second).and_then(|n| n.checked_div(unit)),
        ExtractField::Datetime(f) => d.field(f),
    }
}

/// How many rows of `vector` hold `field`, SECOND in units of `unit`, each
/// asserted the field `Datetime::field` gives of its value: none if the
/// body gives no vector, asserted of a field no value of it has.
fn compared(vector: &Datetimes, field: ExtractField, unit: i64) -> Option<usize> {
    let Some(made) = fields(vector, field, unit) else {
        assert!((0..vector.len()).all(|n| vector.get(n).is_none_or(|d| field_of(&d, field, unit).is_none())));
        return None;
    };
    assert_eq!(made.len(), vector.len());
    let mut held = 0_usize;
    for (n, got) in made.iter().enumerate() {
        let Some(d) = vector.get(n) else { continue };
        assert_eq!(Some(*got), field_of(&d, field, unit), "{field:?} of {d:?} in units of {unit}");
        held = held.saturating_add(1);
    }
    Some(held)
}

/// Verifies: [DESIGN] RULES.md D1320 [ISO/IEC 9075:1992 §6.6 GR3a]: every
/// field of a vector of dates, times or timestamps, with displacements or
/// none and with nulls, is of each row not null the field
/// `Datetime::field` gives of its value, SECOND at each precision, the
/// fields of the displacement its hours and minutes of its sign; at the
/// first instant, the instants before it, the ends of a minute, an hour
/// and a day, leap days, the last instant of 9999 and the ones past it,
/// and the least and greatest words. A field the type has not gives no
/// vector, as does a vector with fewer displacements than rows.
#[test]
fn every_field_of_a_vector_is_the_field_of_each_value() {
    let parts = [Field::Year, Field::Month, Field::Day, Field::Hour, Field::Minute, Field::Second].map(ExtractField::Datetime);
    let asked: Vec<(ExtractField, i64)> = parts
        .into_iter()
        .chain([ExtractField::TimezoneHour, ExtractField::TimezoneMinute])
        .flat_map(|field| [1, 1000, 1_000_000].map(|unit| (field, unit)))
        .collect();
    let (mut held, mut refused) = (0_usize, 0_usize);
    for vector in SHAPES.into_iter().flat_map(|shape| vectors(shape, 16)) {
        for &(field, unit) in &asked {
            match compared(&vector, field, unit) {
                Some(rows) => held = held.saturating_add(rows),
                None => refused = refused.saturating_add(1),
            }
        }
    }
    assert!(held > 50_000 && refused > 100, "{held} fields compared, {refused} vectors refused");
    let short = Datetimes::new(vec![0, DAY], Some(vec![60]), (Moment::Timestamp, true, 6), None);
    assert!(fields(&short, ExtractField::Datetime(Field::Minute), 1).is_none(), "a displacement is missing");
    assert!(cast(&short, (Moment::Timestamp, 6, true), Now::default()).is_none(), "a displacement is missing");
}

/// The type of `shape`.
fn type_of((moment, precision, zoned): (Moment, u32, bool)) -> DataType {
    match moment {
        Moment::Date => DataType::Date,
        Moment::Time => DataType::Time { precision, zoned },
        Moment::Timestamp => DataType::Timestamp { precision, zoned },
    }
}

/// The sessions casts are made in: UTC at the first instant; a day of
/// 2026 east of UTC by a half hour; the last instant at the least
/// displacement; the first day at the greatest.
fn sessions() -> [Now; 4] {
    [
        Now::default(),
        Now { utc: moved(midnight(2026, 11, 27), 5 * HOUR), zone: 330 },
        Now { utc: LAST_INSTANT, zone: MIN_DISPLACEMENT },
        Now { utc: 10 * HOUR, zone: MAX_DISPLACEMENT },
    ]
}

/// Verifies: [DESIGN] RULES.md D1320 [ISO/IEC 9075:1992 §6.10 GR9, GR10,
/// GR11, §6.1 GR5, GR6]: a vector of dates, times or timestamps, with a
/// time zone or without, cast to each datetime type in sessions of four
/// displacements and dates, is of each row the datetime `cast_at` gives
/// of its value, its instant, its displacement, its precision and whether
/// it is WITH TIME ZONE, a null of a null; a vector with a row not null
/// whose cast raises, its year before 0001 or past 9999 in UTC or as
/// shown, gives none, so the rows raise it; a date to a time and a time
/// to a date give none. Vectors of one row say so of every edge alone.
#[test]
fn a_cast_of_a_vector_is_the_cast_of_each_value() {
    let (mut compared, mut raised) = (0_usize, 0_usize);
    for (from, to, now) in SHAPES.into_iter().flat_map(|f| SHAPES.into_iter().flat_map(move |t| sessions().map(|s| (f, t, s)))) {
        let untaken = matches!((from.0, to.0), (Moment::Date, Moment::Time) | (Moment::Time, Moment::Date));
        for vector in vectors(from, 1).into_iter().chain(vectors(from, 5)) {
            let wanted: Vec<_> =
                (0..vector.len()).map(|n| cast_at(vector.get(n).map_or(Value::Null, Value::Datetime), &type_of(to), now)).collect();
            let made = cast(&vector, to, now);
            if untaken || wanted.iter().any(Result::is_err) {
                assert!(made.is_none(), "{from:?} to {to:?} at {now:?}: {vector:?}");
                raised = raised.saturating_add(1);
                continue;
            }
            let made = made.unwrap_or_else(|| panic!("{from:?} to {to:?} at {now:?}: {vector:?}"));
            assert_eq!(made.len(), vector.len());
            for (n, want) in wanted.into_iter().enumerate() {
                let got = made.get(n).map_or(Value::Null, Value::Datetime);
                let (Value::Datetime(g), Ok(Value::Datetime(w))) = (&got, &want) else {
                    assert!(got.is_null() && want.is_ok_and(|v| v.is_null()), "{from:?} to {to:?} at {now:?}, row {n} of {vector:?}");
                    continue;
                };
                assert_eq!(g, w, "{from:?} to {to:?} at {now:?}, row {n} of {vector:?}");
                compared = compared.saturating_add(1);
            }
        }
    }
    assert!(compared > 50_000 && raised > 5_000, "{compared} casts compared, {raised} vectors that raise");
}

/// Verifies: [DESIGN] RULES.md D1320, D1289 [ISO/IEC 9075:1992 §6.1
/// GR6]: a null's slot holds what the store left there, so a slot past
/// the year 9999 under a null sends no vector to the rows, and the cast
/// gives the null; the same slot not null gives none.
#[test]
fn a_slot_past_the_years_under_a_null_is_not_judged() {
    let (shape, to) = ((Moment::Timestamp, false, 6), (Moment::Timestamp, 6, false));
    let nulls = Mask::from_bools(&[false, true, false]);
    let nulled = Datetimes::new(vec![DAY, i64::MAX, 2 * DAY], None, shape, Some(nulls));
    let made = cast(&nulled, to, Now { utc: 0, zone: 60 }).unwrap();
    assert_eq!((made.get(0).map(|d| (d.utc, d.zone)), made.get(1), made.get(2).map(|d| d.utc)), (Some((DAY, 60)), None, Some(2 * DAY)));
    let held = Datetimes::new(vec![DAY, i64::MAX, 2 * DAY], None, shape, None);
    assert!(cast(&held, to, Now { utc: 0, zone: 60 }).is_none());
    // The first hour of 0001 is shown in the year before it west of UTC.
    let first = Datetimes::new(vec![HOUR - 1], None, shape, None);
    assert!(cast(&first, to, Now { utc: 0, zone: -60 }).is_none());
    assert!(cast(&first, to, Now::default()).is_some());
}
