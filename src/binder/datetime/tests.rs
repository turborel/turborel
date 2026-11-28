//! Unit tests of the bodies over vectors of EXTRACT and of CAST to a
//! datetime type as a statement binds them (RULES.md D363, D1320): each
//! against its call of each row's value, the values compared whole.

use super::*;
use crate::storage::{Datetimes, Mask};
use crate::value::CharUnits;
use crate::value::datetime::parse;

/// The session casts are bound in: a half hour past five east of UTC.
const NOW: Now = Now { utc: 0, zone: 330 };

/// A vector of timestamps of six digits as a store reads them, their
/// microseconds alone and no displacement, the third row null: an
/// afternoon, the first instant, a slot under the null, the last morning.
fn stamps() -> Column {
    let texts = ["2013-07-15 13:47:59.123456", "0001-01-01 00:00:00", "1999-12-31 23:59:59.999999", "9999-12-31 10:00:00"];
    let utc = texts.iter().map(|t| parse(Moment::Timestamp, t).unwrap().local).collect();
    let nulls = Mask::from_bools(&[false, false, true, false]);
    Column::Datetimes(Datetimes::new(utc, None, (Moment::Timestamp, false, 6), Some(nulls)))
}

/// Each value of `column` as it is written whole.
fn written(column: &Column) -> Vec<String> {
    column.values().iter().map(|v| format!("{v:?}")).collect()
}

/// What `f` gives of each value of `column`, called a row at a time,
/// written whole.
fn called(f: &dyn ScalarFunction, column: &Column) -> Vec<String> {
    column.values().iter().map(|v| format!("{:?}", f.call(std::slice::from_ref(v)).unwrap())).collect()
}

/// Verifies: [DESIGN] RULES.md D1320, D363 [ISO/IEC 9075:1992 §6.6 GR3,
/// §6.1 GR5]: EXTRACT of a vector of timestamps without time zone, shown
/// in the session's displacement by the cast the binder wraps it in, is
/// computed over vectors at both steps, the cast giving datetimes that
/// carry the displacement and the field integers of its scale, each row's
/// the value the two calls give of that row, a null of the null; of every
/// field, SECOND at three precisions. A field the type has not, and a
/// vector of values, give no vector.
#[test]
fn extract_of_timestamps_as_shown_is_computed_over_vectors() {
    let column = stamps();
    let shown = CastAt { to: DataType::Timestamp { precision: 6, zoned: false }, now: NOW };
    let cast = shown.eval_batch(&[Arg::Column(&column)], 4).unwrap().unwrap();
    let Column::Datetimes(datetimes) = &cast else { panic!("not datetimes: {cast:?}") };
    assert_eq!(datetimes.zones.as_deref(), Some(&[330_i16; 4][..]));
    assert_eq!(written(&cast), called(&shown, &column));
    let whole = [Field::Year, Field::Month, Field::Day, Field::Hour, Field::Minute].map(|f| (ExtractField::Datetime(f), 0));
    let seconds = [0, 3, 6].map(|scale| (ExtractField::Datetime(Field::Second), scale));
    let zones = [(ExtractField::TimezoneHour, 0), (ExtractField::TimezoneMinute, 0)];
    for (field, scale) in whole.into_iter().chain(seconds).chain(zones) {
        let extract = Extract { field, scale, digits: SMALLINT_PRECISION };
        let made = extract.eval_batch(&[Arg::Column(&cast)], 4).unwrap().unwrap();
        let Column::Ints(ints) = &made else { panic!("not integers: {made:?}") };
        assert_eq!(ints.scale, scale, "{field:?}");
        assert_eq!(written(&made), called(&extract, &cast), "{field:?} at scale {scale}");
    }
    let minute = Extract { field: ExtractField::Datetime(Field::Minute), scale: 0, digits: SMALLINT_PRECISION };
    let Some(Ok(Column::Ints(minutes))) = minute.eval_batch(&[Arg::Column(&cast)], 4) else { panic!("no integers") };
    // 13:47 in UTC is 19:17 a half hour past five east of it.
    assert_eq!(minutes.units.first(), Some(&17));
    let dates = Column::Datetimes(Datetimes::new(vec![0], None, (Moment::Date, false, 0), None));
    assert!(minute.eval_batch(&[Arg::Column(&dates)], 1).is_none(), "a date has no minute");
    let values = Column::Values(cast.values().to_vec());
    assert!(minute.eval_batch(&[Arg::Column(&values)], 4).is_none(), "values are called a row at a time");
}

/// Verifies: [DESIGN] RULES.md D1320, D15 [ISO/IEC 9075:1992 §6.10 GR9,
/// GR11, §6.1 GR6]: CAST of a vector of timestamps to DATE and to a
/// timestamp WITH TIME ZONE of fewer digits gives over vectors each row's
/// value as its call gives it; to a character string no vector, its rows
/// called; and where a row's cast raises, the first instant shown west of
/// UTC in the year before 0001, no vector, the call of that row raising
/// SQLSTATE 22007.
#[test]
fn cast_of_timestamps_is_computed_over_vectors_or_left_to_its_rows() {
    let column = stamps();
    for to in [DataType::Date, DataType::Timestamp { precision: 2, zoned: true }, DataType::Time { precision: 0, zoned: false }] {
        let cast = CastAt { to: to.clone(), now: NOW };
        let made = cast.eval_batch(&[Arg::Column(&column)], 4).unwrap().unwrap();
        assert_eq!(written(&made), called(&cast, &column), "to {to}");
    }
    let text = CastAt { to: DataType::Varying { length: 40, units: CharUnits::Characters }, now: NOW };
    assert!(text.eval_batch(&[Arg::Column(&column)], 4).is_none(), "a string is made a row at a time");
    let west = CastAt { to: DataType::Timestamp { precision: 6, zoned: false }, now: Now { utc: 0, zone: -60 } };
    assert!(west.eval_batch(&[Arg::Column(&column)], 4).is_none(), "the first instant is of no year west of UTC");
    let first = column.values().get(1).cloned().unwrap();
    assert_eq!(west.call(&[first]).unwrap_err().sqlstate(), SqlState::INVALID_DATETIME_FORMAT);
}
