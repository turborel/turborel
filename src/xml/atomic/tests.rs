//! Tests of casting atomic values.

use super::*;

fn untyped(text: &str) -> Atomic {
    Atomic { kind: AtomicType::UntypedAtomic, lexical: text.into() }
}

/// `text`, untyped, cast as `to`: its canonical form, or the error code.
fn cast_text(text: &str, to: AtomicType) -> String {
    match cast(&untyped(text), to) {
        Ok(a) => {
            assert_eq!(a.kind, to);
            a.lexical.into_string()
        }
        Err(e) => {
            assert_eq!(e.sqlstate(), SqlState::XQUERY_ERROR, "{e}");
            let message = e.to_string();
            message.split_whitespace().find(|w| w.starts_with("err:")).unwrap_or("err:?").to_owned()
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi] [XQueryFO §17.1.1,
/// §17.1.2] [Schema2 §3.2.2, §3.2.3, §3.3.13] numbers and truth values
/// read by their lexical forms, whitespace collapsed, and written as
/// casting them to a string writes them; anything else err:FORG0001.
#[test]
fn numbers_and_truth_values() {
    let cases = [
        (" +012.50 ", AtomicType::Decimal, "12.5"),
        ("-0.0", AtomicType::Decimal, "0"),
        (".5", AtomicType::Decimal, "0.5"),
        ("100.", AtomicType::Decimal, "100"),
        ("1.2.3", AtomicType::Decimal, "err:FORG0001"),
        ("0042", AtomicType::Integer, "42"),
        ("-0", AtomicType::Integer, "0"),
        ("4.0", AtomicType::Integer, "err:FORG0001"),
        ("1", AtomicType::Boolean, "true"),
        ("false", AtomicType::Boolean, "false"),
        ("TRUE", AtomicType::Boolean, "err:FORG0001"),
        ("1.5E0", AtomicType::Double, "1.5"),
        ("1e6", AtomicType::Double, "1.0E6"),
        ("0.000001", AtomicType::Double, "0.000001"),
        ("1.25e-7", AtomicType::Double, "1.25E-7"),
        ("-0", AtomicType::Double, "-0"),
        ("INF", AtomicType::Double, "INF"),
        ("NaN", AtomicType::Float, "NaN"),
        ("0.1", AtomicType::Float, "0.1"),
        ("1E", AtomicType::Double, "err:FORG0001"),
        ("+INF", AtomicType::Double, "err:FORG0001"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1.3,
/// §17.1.6] casts among numbers and truth
/// values: a decimal's fraction dropped as an integer; a float or double
/// its shortest decimal; NaN and infinity no decimal, err:FOCA0002; zero
/// and NaN false.
#[test]
fn casts_among_numbers() {
    let of = |kind, lexical: &str| Atomic { kind, lexical: lexical.into() };
    let cases = [
        (of(AtomicType::Decimal, "-3.9"), AtomicType::Integer, "-3"),
        (of(AtomicType::Decimal, "-0.5"), AtomicType::Integer, "0"),
        (of(AtomicType::Double, "2.5"), AtomicType::Decimal, "2.5"),
        (of(AtomicType::Double, "1.0E20"), AtomicType::Integer, "100000000000000000000"),
        (of(AtomicType::Integer, "7"), AtomicType::Double, "7"),
        (of(AtomicType::Double, "0.1"), AtomicType::Float, "0.1"),
        (of(AtomicType::Boolean, "true"), AtomicType::Decimal, "1"),
        (of(AtomicType::Decimal, "0"), AtomicType::Boolean, "false"),
        (of(AtomicType::Double, "NaN"), AtomicType::Boolean, "false"),
        (of(AtomicType::Float, "-2"), AtomicType::Boolean, "true"),
        (of(AtomicType::Integer, "5"), AtomicType::String, "5"),
    ];
    for (value, to, expected) in cases {
        assert_eq!(&*cast(&value, to).unwrap().lexical, expected, "{value:?} as {}", to.name());
    }
    let error = cast(&of(AtomicType::Double, "INF"), AtomicType::Decimal).unwrap_err();
    assert!(error.to_string().contains("FOCA0002"), "{error}");
    let refused = cast(&of(AtomicType::Boolean, "true"), AtomicType::Date).unwrap_err();
    assert!(refused.to_string().contains("XPTY0004"), "{refused}");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §17.1.1,
/// §17.1.2, §17.1.5, §10.2] [Schema2
/// §3.2.7.1, §3.2.8.1, §3.2.9.1] dates and times: their local values
/// kept, a fraction's zeros dropped, a zero time zone Z, 24:00:00 the next
/// day's start; invalid dates err:FORG0001; years past the limits
/// err:FODT0001; a date and time cast as its date or time.
#[test]
fn dates_and_times() {
    let cases = [
        ("2020-02-29", AtomicType::Date, "2020-02-29"),
        ("2021-02-29", AtomicType::Date, "err:FORG0001"),
        ("2020-01-02+05:30", AtomicType::Date, "2020-01-02+05:30"),
        ("2020-01-02-00:00", AtomicType::Date, "2020-01-02Z"),
        ("0000-01-01", AtomicType::Date, "err:FORG0001"),
        ("10000-01-01", AtomicType::Date, "err:FODT0001"),
        ("-0001-01-01", AtomicType::Date, "err:FODT0001"),
        ("12:30:05.500", AtomicType::Time, "12:30:05.5"),
        ("24:00:00", AtomicType::Time, "00:00:00"),
        ("24:00:01", AtomicType::Time, "err:FORG0001"),
        ("12:30:05+14:01", AtomicType::Time, "err:FORG0001"),
        ("12:30", AtomicType::Time, "err:FORG0001"),
        ("1999-12-31T24:00:00", AtomicType::DateTime, "2000-01-01T00:00:00"),
        ("2020-01-02T03:04:05.1234567Z", AtomicType::DateTime, "2020-01-02T03:04:05.123456Z"),
        ("2020-01-02 03:04:05", AtomicType::DateTime, "err:FORG0001"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
    let instant = Atomic { kind: AtomicType::DateTime, lexical: "2020-01-02T03:04:05-08:00".into() };
    assert_eq!(&*cast(&instant, AtomicType::Date).unwrap().lexical, "2020-01-02-08:00");
    assert_eq!(&*cast(&instant, AtomicType::Time).unwrap().lexical, "03:04:05-08:00");
    let day = Atomic { kind: AtomicType::Date, lexical: "2020-01-02".into() };
    assert_eq!(&*cast(&day, AtomicType::DateTime).unwrap().lexical, "2020-01-02T00:00:00");
    assert!(cast(&day, AtomicType::Time).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §10.7] a
/// date, time or date and time adjusted to UTC and its time zone removed:
/// one with none as it is; a date the date of its start in UTC.
#[test]
fn adjusted_to_utc() {
    let utc = |text: &str, shape| Moment::parse(text, shape).unwrap().unwrap().in_utc().text();
    assert_eq!(utc("2020-01-02T03:04:05+05:00", Shape::DateTime), "2020-01-01T22:04:05");
    assert_eq!(utc("2020-01-02T03:04:05", Shape::DateTime), "2020-01-02T03:04:05");
    assert_eq!(utc("2020-01-02+05:00", Shape::Date), "2020-01-01");
    assert_eq!(utc("2020-01-02-05:00", Shape::Date), "2020-01-02");
    assert_eq!(utc("01:00:00+02:00", Shape::Time), "23:00:00");
    let m = Moment::parse("2020-01-02T03:04:05.5-08:00", Shape::DateTime).unwrap().unwrap();
    assert_eq!(m.sql_literal(), "2020-01-02 03:04:05.5-08:00");
    assert_eq!(m.year(), 2020);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [XQueryFO §10.3,
/// §17.1.4] durations: years and months, or
/// days to seconds, in canonical form, P0M and PT0S of zero; the other
/// components, or none, err:FORG0001; one cast as the other zero.
#[test]
fn durations() {
    let cases = [
        ("P14M", AtomicType::YearMonthDuration, "P1Y2M"),
        ("-P1Y0M", AtomicType::YearMonthDuration, "-P1Y"),
        ("P0Y", AtomicType::YearMonthDuration, "P0M"),
        ("P1D", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("P", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("P1M1Y", AtomicType::YearMonthDuration, "err:FORG0001"),
        ("PT36H", AtomicType::DayTimeDuration, "P1DT12H"),
        ("P1DT0.50S", AtomicType::DayTimeDuration, "P1DT0.5S"),
        ("-PT90M", AtomicType::DayTimeDuration, "-PT1H30M"),
        ("PT0S", AtomicType::DayTimeDuration, "PT0S"),
        ("P1DT", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("PT1.5M", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("P1Y", AtomicType::DayTimeDuration, "err:FORG0001"),
        ("P99999999999999999999999999999999999999Y", AtomicType::YearMonthDuration, "err:FODT0002"),
    ];
    for (text, to, expected) in cases {
        assert_eq!(cast_text(text, to), expected, "{text} as {}", to.name());
    }
    let months = Atomic { kind: AtomicType::YearMonthDuration, lexical: "P1Y".into() };
    assert_eq!(&*cast(&months, AtomicType::DayTimeDuration).unwrap().lexical, "PT0S");
    assert_eq!(duration_units("-P1DT1S", false).unwrap(), -86_401_000_000);
    assert!(duration_units("x", true).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j] [Schema2 §3.2.15]
/// [XQueryFO §17.1.7] hex binary: two digits
/// an octet, written in upper case; an odd count or no digit is no
/// lexical form.
#[test]
fn hex_binary() {
    assert_eq!(cast_text("0fB7", AtomicType::HexBinary), "0FB7");
    assert_eq!(cast_text("", AtomicType::HexBinary), "");
    assert_eq!(cast_text("ABC", AtomicType::HexBinary), "err:FORG0001");
    assert_eq!(cast_text("0G", AtomicType::HexBinary), "err:FORG0001");
    assert_eq!(hex("00ff"), Some(vec![0, 255]));
    assert_eq!(hex_text(&[1, 171]), "01AB");
}
