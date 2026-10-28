//! Unit tests of datetime values (RULES.md R-TEST-1, R-TEST-2): the
//! calendar, literals read and written, displacements, fields and
//! precision. Split from datetime.rs (RULES.md R-SIZE-2).

use super::*;

/// The value `text` writes, of `moment`, in its own displacement or UTC.
fn of(moment: Moment, text: &str) -> Datetime {
    let w = parse(moment, text).unwrap();
    Datetime::of_local(moment, w.local, w.zone.unwrap_or(0), w.zone.is_some(), w.precision)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1 Table 10, §4.5.1] every
/// day from 0001-01-01 to 9999-12-31 of the Gregorian calendar is the next
/// after the one before, its year, month and day read back.
#[test]
fn every_day_of_the_calendar() {
    let mut expected = (1_i64, 1_i64, 1_i64);
    for days in 0..=LAST_DAY {
        assert_eq!(civil_from_days(days), expected, "day {days}");
        assert_eq!(days_from_civil(expected.0, expected.1, expected.2), days);
        let (y, m, d) = expected;
        expected = if d < days_in(y, m) {
            (y, m, d + 1)
        } else if m < 12 {
            (y, m + 1, 1)
        } else {
            (y + 1, 1, 1)
        };
    }
    assert_eq!(expected, (10_000, 1, 1));
    assert!(leap(2000) && leap(2024) && !leap(1900) && !leap(2023));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR16, SR17, SR21, SR22,
/// SR25] a datetime literal's string is of four digits of year and two of
/// each other field, the fraction's digits its precision, dates and times
/// valid in the calendar, seconds to 59, a displacement from -12:59 to
/// +13:00; it is shown as written.
#[test]
fn literals_are_read_and_written() {
    for (moment, text) in [
        (Moment::Date, "2024-02-29"),
        (Moment::Date, "0001-01-01"),
        (Moment::Date, "9999-12-31"),
        (Moment::Time, "23:59:59"),
        (Moment::Time, "00:00:00.5"),
        (Moment::Time, "12:30:00+05:30"),
        (Moment::Time, "01:00:00.123456-12:59"),
        (Moment::Timestamp, "2024-03-01 00:00:00"),
        (Moment::Timestamp, "1999-12-31 23:59:59.999+13:00"),
    ] {
        assert_eq!(of(moment, text).to_string(), text);
    }
    for (moment, text) in [
        (Moment::Date, "2023-02-29"),
        (Moment::Date, "2024-13-01"),
        (Moment::Date, "0000-01-01"),
        (Moment::Date, "24-01-01"),
        (Moment::Date, "2024-1-01"),
        (Moment::Date, "2024-01-01 "),
        (Moment::Time, "24:00:00"),
        (Moment::Time, "12:60:00"),
        (Moment::Time, "12:00:60"),
        (Moment::Time, "1:00:00"),
        (Moment::Time, "12:00:00+13:01"),
        (Moment::Time, "12:00:00-13:00"),
        (Moment::Time, "12:00:00+5:00"),
        (Moment::Time, "12:00:00 +05:00"),
        (Moment::Time, "12:00:00+05:60"),
        (Moment::Time, "12:00:00*05:00"),
        (Moment::Timestamp, "2024-01-01T00:00:00"),
        (Moment::Timestamp, "2024-01-01"),
    ] {
        assert!(parse(moment, text).is_none(), "{text}");
    }
    let w = parse(Moment::Time, "00:00:01.1234567").unwrap();
    assert_eq!((w.local, w.precision), (1_123_456, 7), "digits past the microsecond dropped");
    assert_eq!(parse_displacement("-00:30"), Some(-30));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.5.1, §8.2 GR5, §6.6 GR3a i]
/// a time or timestamp is kept in UTC: values of one instant in different
/// displacements compare equal, each shown and its fields read in its
/// own; a time wraps about midnight; a timestamp's year in UTC or as shown
/// must be from 0001 to 9999.
#[test]
fn instants_fields_and_ranges() {
    let east = of(Moment::Timestamp, "2024-01-01 05:30:00+05:30");
    let west = of(Moment::Timestamp, "2023-12-31 19:00:00-05:00");
    assert_eq!(east.compare(&west), Some(Ordering::Equal));
    assert_eq!(east.compare(&of(Moment::Time, "00:00:00")), None, "not of the same fields");
    assert_eq!(
        [Field::Year, Field::Month, Field::Day, Field::Hour, Field::Minute].map(|f| west.field(f)),
        [Some(2023), Some(12), Some(31), Some(19), Some(0)]
    );
    assert_eq!(of(Moment::Time, "00:30:15.25+01:00").to_string(), "00:30:15.25+01:00");
    assert_eq!(of(Moment::Time, "00:30:15.25+01:00").utc, (23 * 60 + 30) * MICROS_PER_MINUTE + 15_250_000, "wraps");
    assert_eq!(of(Moment::Time, "10:00:07.5").field(Field::Second), Some(7_500_000));
    assert_eq!(of(Moment::Date, "2024-05-06").field(Field::Hour), None);
    assert_eq!(of(Moment::Time, "10:00:00").field(Field::Year), None);
    assert!(of(Moment::Timestamp, "9999-12-31 23:00:00").in_range());
    assert!(!of(Moment::Timestamp, "9999-12-31 23:00:00-01:00").in_range(), "10000 in UTC");
    assert!(!of(Moment::Timestamp, "0001-01-01 00:00:00+01:00").in_range(), "year 0 in UTC");
    assert!(of(Moment::Time, "00:00:00+13:00").in_range());
    assert_eq!(east.key_word(), "TIMESTAMP");
    assert_eq!(Datetime::displacement_text(-779), "-12:59");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.2 GR3l, §4.6] a value
/// truncated to fewer digits of the second's fraction drops the rest and
/// shows that many; the date and time of day it falls on are as shown.
#[test]
fn precision_and_parts() {
    let t = of(Moment::Timestamp, "2024-06-30 23:59:59.987654");
    assert_eq!(t.truncated(2).to_string(), "2024-06-30 23:59:59.98");
    assert_eq!(t.truncated(0).to_string(), "2024-06-30 23:59:59");
    assert_eq!(t.truncated(9).to_string(), "2024-06-30 23:59:59.987654");
    assert_eq!(of(Moment::Date, "2024-06-30").days(), days_from_civil(2024, 6, 30));
    assert_eq!(t.time_of_day(), MICROS_PER_DAY - 12_346);
    assert_eq!(utc_of_unix(0), UNIX_EPOCH_DAYS * MICROS_PER_DAY);
    assert_eq!(days_from_civil(1970, 1, 1), UNIX_EPOCH_DAYS);
    assert_eq!(days_from_civil(9999, 12, 31), LAST_DAY);
}
