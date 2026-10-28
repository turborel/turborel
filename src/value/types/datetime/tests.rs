//! Unit tests of the datetime kinds: CAST in a session of a displacement
//! and a date, and their bytes, literals and descriptors. Split from
//! datetime.rs (RULES.md R-SIZE-2).

use super::*;
use crate::value::Spelled;
use crate::value::datetime::{Written, parse};

/// The value `text` writes, of `moment`, of its own displacement, or of
/// `zone` WITH TIME ZONE if `zoned`, else implied.
fn of(moment: Moment, text: &str) -> Value {
    let Written { local, zone, precision, .. } = parse(moment, text).unwrap();
    Value::Datetime(Datetime::of_local(moment, local, zone.unwrap_or(0), zone.is_some(), precision))
}

/// A session at 2024-02-29 23:30:00 UTC, of +01:00, so its date is
/// 2024-03-01.
fn session() -> Now {
    let Written { local, .. } = parse(Moment::Timestamp, "2024-02-29 23:30:00").unwrap();
    Now { utc: local, zone: 60 }
}

/// `v` CAST to `t` in `session()`, as its string.
fn cast(v: Value, t: DataType) -> String {
    cast_at(v, &t, session()).unwrap().to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR9-GR11] in a session:
/// a time or timestamp WITH TIME ZONE keeps its displacement, and one
/// without takes the session's; a time becomes a timestamp on the
/// session's date; a timestamp a date or time as shown.
#[test]
fn casts_in_a_session() {
    let zoned = of(Moment::Timestamp, "2024-01-02 03:04:05.678+05:00");
    assert_eq!(cast(zoned.clone(), DataType::Timestamp { precision: 1, zoned: true }), "2024-01-02 03:04:05.6+05:00");
    assert_eq!(cast(zoned.clone(), DataType::Timestamp { precision: 0, zoned: false }), "2024-01-01 23:04:05");
    assert_eq!(cast(zoned.clone(), DataType::Time { precision: 0, zoned: true }), "03:04:05+05:00");
    assert_eq!(cast(zoned.clone(), DataType::Time { precision: 0, zoned: false }), "23:04:05");
    assert_eq!(cast(zoned.clone(), DataType::Date), "2024-01-02", "as shown");
    let time = of(Moment::Time, "22:00:00+02:00");
    assert_eq!(cast(time.clone(), DataType::Timestamp { precision: 0, zoned: true }), "2024-03-01 22:00:00+02:00", "the session's date");
    assert_eq!(cast(time, DataType::Time { precision: 0, zoned: false }), "21:00:00");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR5e, GR6e, GR9-GR11,
/// SR4; §6.1 SR6] in a session: a date is a timestamp at its midnight in
/// the session's displacement; a string is the datetime it writes, of the
/// instant its displacement gives; a
/// datetime CAST to a string its literal's string; a date to a time, a
/// number or a bit string to a datetime, is no value (SQLSTATE 22005); a
/// year out of 0001 to 9999 is refused (22007).
#[test]
fn more_casts_in_a_session() {
    let date = of(Moment::Date, "2024-05-06");
    assert_eq!(cast(date.clone(), DataType::Timestamp { precision: 0, zoned: true }), "2024-05-06 00:00:00+01:00");
    assert_eq!(cast(date.clone(), DataType::Date), "2024-05-06");
    assert_eq!(cast(Value::Char("10:00:00-03:00".into()), DataType::Time { precision: 0, zoned: false }), "14:00:00", "its instant");
    assert_eq!(cast(Value::Char("10:00:00".into()), DataType::Time { precision: 0, zoned: true }), "10:00:00+01:00");
    assert_eq!(cast(Value::Null, DataType::Date), "NULL");
    assert_eq!(cast(date.clone(), DataType::Varying { length: 10 }), "2024-05-06");
    assert_eq!(
        cast(Value::Exact(crate::value::Decimal::new(1, 0)), DataType::Character { length: 3 }),
        "1  ",
        "not a datetime: as the type casts"
    );
    for (v, t) in [
        (date, DataType::Time { precision: 0, zoned: false }),
        (of(Moment::Time, "01:00:00"), DataType::Date),
        (Value::Exact(crate::value::Decimal::new(1, 0)), DataType::Date),
        (Value::Bits(crate::value::Bits::default()), DataType::Date),
    ] {
        assert_eq!(cast_at(v, &t, session()).unwrap_err().code, Code::Data, "{t}");
    }
    let late = of(Moment::Timestamp, "9999-12-31 23:00:00-01:00");
    assert_eq!(
        cast_at(late, &DataType::Timestamp { precision: 0, zoned: true }, session()).unwrap_err().code,
        Code::Data,
        "22007: 10000 in UTC"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1] each datetime kind is
/// named and written as SQL writes it.
#[test]
fn kinds_are_named_as_written() {
    let names = [&DATE, &TIME, &TIME_ZONED, &TIMESTAMP, &TIMESTAMP_ZONED].map(<Chronon as SqlType>::name);
    assert_eq!(names, ["DATE", "TIME", "TIME WITH TIME ZONE", "TIMESTAMP", "TIMESTAMP WITH TIME ZONE"]);
    assert_eq!(TIME_ZONED.written(&DataType::Time { precision: 2, zoned: true }), "TIME(2) WITH TIME ZONE");
    assert_eq!(TIMESTAMP.written(&DataType::Timestamp { precision: 0, zoned: false }), "TIMESTAMP(0)");
    assert_eq!(TIMESTAMP_ZONED.written(&DataType::Timestamp { precision: 3, zoned: true }), "TIMESTAMP(3) WITH TIME ZONE");
    assert_eq!(DATE.written(&DataType::Date), "DATE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3, §21.3.5] a datetime
/// kind's literal, text, position and descriptor; its value's bytes read
/// back, a displacement out of range corrupt, a record of the wrong bytes
/// none; SqlType::cast is CAST in UTC; CURRENT_TIME is the instant's time.
#[test]
fn kinds_keep_their_values() {
    let t = of(Moment::Time, "01:02:03.5+04:00");
    assert_eq!(
        (TIME_ZONED.literal(&t), TIME_ZONED.literal(&Value::Null), TIME_ZONED.text(&t)),
        ("TIME '01:02:03.5+04:00'".to_owned(), "NULL".to_owned(), "01:02:03.5+04:00".to_owned())
    );
    assert!(TIME_ZONED.position(&t).is_some_and(|p| p > 0.0_f64) && TIME_ZONED.position(&Value::Null).is_none());
    assert_eq!(TIMESTAMP.descriptor(&DataType::Timestamp { precision: 3, zoned: false }).datetime_precision, Some(3));
    assert_eq!(DATE.descriptor(&DataType::Date).datetime_precision, Some(0));
    let mut out = Vec::new();
    TIME_ZONED.put_value(&Value::Null, &mut out);
    assert_eq!(TIME_ZONED.value(&mut Reader::new(&out, "value")).unwrap().to_string(), "00:00:00+00:00", "of another kind, its zero");
    let mut bad = vec![1, 0];
    bad.put_u32(70_000);
    bad.put_u64(0);
    assert_eq!(TIME_ZONED.value(&mut Reader::new(&bad, "value")).unwrap_err().code, Code::Corrupt);
    let zoned = DataType::Time { precision: 1, zoned: true };
    assert!(TIME_ZONED.decode_record(&[0; 3], &zoned).is_none() && TIME_ZONED.decode_record(&[0; 9], &zoned).is_none());
    assert_eq!(TIME_ZONED.record_width(&zoned), Some(10));
    assert_eq!(
        TIME.cast(Value::Char("10:00:00".into()), &DataType::Time { precision: 0, zoned: true }).unwrap().to_string(),
        "10:00:00+00:00"
    );
    assert!(
        !DATE.comparable(&DataType::Time { precision: 0, zoned: false })
            && !DATE.comparable(&DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Numeric })
    );
    let now = Now { utc: 3_600_000_000, zone: 90 };
    assert_eq!(current(Moment::Time, 0, now).to_string(), "02:30:00+01:30");
    assert_eq!(current(Moment::Date, 0, now).to_string(), "0001-01-01");
    assert_eq!(kind_of(&Datetime::of_local(Moment::Date, 0, 0, false, 0)).name(), "DATE");
}
