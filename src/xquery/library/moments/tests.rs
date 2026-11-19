//! Tests of dates and times: comparisons, components and time zones.

use super::super::testing::{atom, call, call_in, code, empty, focus, s, text, typed};
use super::*;

/// The implicit time zone of [XQueryFO §10.4]'s examples, -05:00.
const MINUS_FIVE: i32 = -300;

fn at(kind: AtomicType, text: &str) -> Atomic {
    Atomic::new(kind, text)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.4.8.1]
/// the examples of `op:dateTime-equal`, a value of no time zone in the
/// implicit one, 24:00:00 the next day's start.
#[test]
fn date_time_equality_examples() {
    let dt = |t: &str| at(AtomicType::DateTime, t);
    let equal = |a: &str, b: &str| date_time_equal(&dt(a), &dt(b), MINUS_FIVE).unwrap();
    assert!(equal("2002-04-02T12:00:00-01:00", "2002-04-02T17:00:00+04:00"));
    assert!(equal("2002-04-02T12:00:00", "2002-04-02T23:00:00+06:00"));
    assert!(!equal("2002-04-02T12:00:00", "2002-04-02T17:00:00"));
    assert!(equal("2002-04-02T12:00:00", "2002-04-02T12:00:00"));
    assert!(equal("2002-04-02T23:00:00-04:00", "2002-04-03T02:00:00-01:00"));
    assert!(equal("1999-12-31T24:00:00", "2000-01-01T00:00:00"));
    assert!(!equal("2005-04-04T24:00:00", "2005-04-04T00:00:00"));
    assert!(date_time_less_than(&dt("2002-04-02T12:00:00"), &dt("2002-04-02T12:00:01"), 0).unwrap());
    assert!(date_time_greater_than(&dt("2002-04-02T12:00:00Z"), &dt("2002-04-02T12:00:00+01:00"), 0).unwrap());
    assert!(date_time_equal(&dt("2002-04-02T12:00:00"), &at(AtomicType::Date, "2002-04-02"), 0).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.4.11.1
/// to §10.4.16.1] the examples of the comparisons of dates, by their
/// starting instants, and of times, on one reference date, 24:00:00
/// before 23:59:59.
#[test]
fn date_and_time_examples() {
    let d = |t: &str| at(AtomicType::Date, t);
    let t = |x: &str| at(AtomicType::Time, x);
    assert!(!date_equal(&d("2004-12-25Z"), &d("2004-12-25+07:00"), MINUS_FIVE).unwrap());
    assert!(date_equal(&d("2004-12-25-12:00"), &d("2004-12-26+12:00"), MINUS_FIVE).unwrap());
    assert!(date_less_than(&d("2004-12-25Z"), &d("2004-12-25-05:00"), MINUS_FIVE).unwrap());
    assert!(!date_less_than(&d("2004-12-25-12:00"), &d("2004-12-26+12:00"), MINUS_FIVE).unwrap());
    assert!(date_greater_than(&d("2004-12-25Z"), &d("2004-12-25+07:00"), MINUS_FIVE).unwrap());
    assert!(!date_greater_than(&d("2004-12-25-12:00"), &d("2004-12-26+12:00"), MINUS_FIVE).unwrap());
    assert!(!time_equal(&t("08:00:00+09:00"), &t("17:00:00-06:00"), MINUS_FIVE).unwrap());
    assert!(time_equal(&t("21:30:00+10:30"), &t("06:00:00-05:00"), MINUS_FIVE).unwrap());
    assert!(time_equal(&t("24:00:00+01:00"), &t("00:00:00+01:00"), MINUS_FIVE).unwrap());
    assert!(!time_less_than(&t("12:00:00"), &t("23:00:00+06:00"), MINUS_FIVE).unwrap());
    assert!(time_less_than(&t("11:00:00"), &t("17:00:00Z"), MINUS_FIVE).unwrap());
    assert!(!time_less_than(&t("23:59:59"), &t("24:00:00"), MINUS_FIVE).unwrap());
    assert!(!time_greater_than(&t("08:00:00+09:00"), &t("17:00:00-06:00"), MINUS_FIVE).unwrap());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.4.17.1
/// to §10.4.21.1] the examples of the equality of the Gregorian types, by
/// their starting instants in a template.
#[test]
fn gregorian_examples() {
    let eq = |kind, a: &str, b: &str| gregorian_equal(&at(kind, a), &at(kind, b), kind, MINUS_FIVE).unwrap();
    assert!(!eq(AtomicType::GYearMonth, "1976-02", "1976-03Z"));
    assert!(!eq(AtomicType::GYearMonth, "1976-03", "1976-03Z"));
    assert!(!eq(AtomicType::GYear, "2005-12:00", "2005+12:00"));
    assert!(eq(AtomicType::GYear, "1976-05:00", "1976"));
    assert!(eq(AtomicType::GMonthDay, "--12-25-14:00", "--12-26+10:00"));
    assert!(!eq(AtomicType::GMonthDay, "--12-25", "--12-26Z"));
    assert!(!eq(AtomicType::GMonth, "--12---14:00", "--12--+10:00"));
    assert!(!eq(AtomicType::GMonth, "--12--", "--12--Z"));
    assert!(!eq(AtomicType::GDay, "---25-14:00", "---25+10:00"));
    assert!(!eq(AtomicType::GDay, "---12", "---12Z"));
    assert!(eq(AtomicType::GDay, "---12", "---12-05:00"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.5.7.1
/// to §10.5.21.1] the examples of the component functions of dates and
/// times, each of the local value.
#[test]
fn component_examples() {
    let of = |f: &str, kind, t: &str| text(call(f, &[atom(kind, t)]));
    let dt = AtomicType::DateTime;
    let cases = [
        (of("year-from-dateTime", dt, "1999-05-31T13:20:00-05:00"), "1999"),
        (of("year-from-dateTime", dt, "1999-12-31T24:00:00"), "2000"),
        (of("month-from-dateTime", dt, "1999-12-31T19:20:00-05:00"), "12"),
        (of("day-from-dateTime", dt, "1999-05-31T13:20:00-05:00"), "31"),
        (of("hours-from-dateTime", dt, "1999-12-31T21:20:00-05:00"), "21"),
        (of("hours-from-dateTime", dt, "1999-12-31T24:00:00"), "0"),
        (of("minutes-from-dateTime", dt, "1999-05-31T13:30:00+05:30"), "30"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
    assert_eq!(typed(call("seconds-from-dateTime", &[atom(dt, "1999-05-31T13:20:00-05:00")])), (AtomicType::Decimal, "0".to_owned()));
    assert_eq!(of("timezone-from-dateTime", dt, "1999-05-31T13:20:00-05:00"), "-PT5H");
    assert_eq!(of("timezone-from-dateTime", dt, "2000-06-12T13:20:00Z"), "PT0S");
    assert!(call("timezone-from-dateTime", &[atom(dt, "2004-08-27T00:00:00")]).unwrap().items().is_empty());
    let cases_2 = [
        (of("year-from-date", AtomicType::Date, "2000-01-01+05:00"), "2000"),
        (of("month-from-date", AtomicType::Date, "1999-05-31-05:00"), "5"),
        (of("day-from-date", AtomicType::Date, "2000-01-01+05:00"), "1"),
        (of("timezone-from-date", AtomicType::Date, "1999-05-31-05:00"), "-PT5H"),
        (of("hours-from-time", AtomicType::Time, "01:23:00+05:00"), "1"),
        (of("hours-from-time", AtomicType::Time, "24:00:00"), "0"),
        (of("minutes-from-time", AtomicType::Time, "13:00:00Z"), "0"),
        (of("seconds-from-time", AtomicType::Time, "13:20:10.5"), "10.5"),
        (of("timezone-from-time", AtomicType::Time, "13:20:00-05:00"), "-PT5H"),
    ];
    for (got, want) in cases_2 {
        assert_eq!(got, want);
    }
    assert!(call("year-from-date", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("year-from-date", &[s("2000-01-01")])), "XPTY0004");
    assert_eq!(code(call("hours-from-time", &[atom(dt, "2000-01-01T00:00:00")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.7.1.1,
/// §10.7.2.1, §10.7.3.1] the examples of the time zone adjustments, the
/// implicit time zone -05:00 where none is given.
#[test]
fn adjustment_examples() {
    let f = focus();
    let tz = |t: &str| atom(AtomicType::DayTimeDuration, t);
    let adjust = |name: &str, kind, value: &str, zone: Option<Sequence>| {
        let mut args = vec![atom(kind, value)];
        args.extend(zone);
        text(call_in(&f, name, &args))
    };
    let dt = |v: &str, z| adjust("adjust-dateTime-to-timezone", AtomicType::DateTime, v, z);
    let cases = [
        (dt("2002-03-07T10:00:00", None), "2002-03-07T10:00:00-05:00"),
        (dt("2002-03-07T10:00:00-07:00", None), "2002-03-07T12:00:00-05:00"),
        (dt("2002-03-07T10:00:00", Some(tz("-PT10H"))), "2002-03-07T10:00:00-10:00"),
        (dt("2002-03-07T10:00:00-07:00", Some(tz("-PT10H"))), "2002-03-07T07:00:00-10:00"),
        (dt("2002-03-07T10:00:00-07:00", Some(tz("PT10H"))), "2002-03-08T03:00:00+10:00"),
        (dt("2002-03-07T00:00:00+01:00", Some(tz("-PT8H"))), "2002-03-06T15:00:00-08:00"),
        (dt("2002-03-07T10:00:00", Some(empty())), "2002-03-07T10:00:00"),
        (dt("2002-03-07T10:00:00-07:00", Some(empty())), "2002-03-07T10:00:00"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
    let d = |v: &str, z| adjust("adjust-date-to-timezone", AtomicType::Date, v, z);
    let cases_2 = [
        (d("2002-03-07", None), "2002-03-07-05:00"),
        (d("2002-03-07-07:00", None), "2002-03-07-05:00"),
        (d("2002-03-07", Some(tz("-PT10H"))), "2002-03-07-10:00"),
        (d("2002-03-07-07:00", Some(tz("-PT10H"))), "2002-03-06-10:00"),
        (d("2002-03-07-07:00", Some(empty())), "2002-03-07"),
    ];
    for (got, want) in cases_2 {
        assert_eq!(got, want);
    }
    let t = |v: &str, z| adjust("adjust-time-to-timezone", AtomicType::Time, v, z);
    let cases_3 = [
        (t("10:00:00", None), "10:00:00-05:00"),
        (t("10:00:00-07:00", None), "12:00:00-05:00"),
        (t("10:00:00-07:00", Some(tz("-PT10H"))), "07:00:00-10:00"),
        (t("10:00:00-07:00", Some(empty())), "10:00:00"),
        (t("10:00:00-07:00", Some(tz("PT10H"))), "03:00:00+10:00"),
    ];
    for (got, want) in cases_3 {
        assert_eq!(got, want);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.1.1,
/// §10.7.1] a zone beyond 14 hours or not of whole minutes err:FODT0003; a
/// result before 0001 err:FODT0001; the empty sequence of the empty
/// sequence.
#[test]
fn adjustment_errors() {
    let tz = |t: &str| atom(AtomicType::DayTimeDuration, t);
    let first = atom(AtomicType::DateTime, "2002-03-07T10:00:00");
    assert_eq!(code(call("adjust-dateTime-to-timezone", &[first.clone(), tz("PT14H1M")])), "FODT0003");
    assert_eq!(code(call("adjust-dateTime-to-timezone", &[first.clone(), tz("PT1H0.5S")])), "FODT0003");
    assert_eq!(text(call("adjust-dateTime-to-timezone", &[first, tz("-PT14H")])), "2002-03-07T10:00:00-14:00");
    let early = atom(AtomicType::DateTime, "0001-01-01T00:00:00+01:00");
    assert_eq!(code(call("adjust-dateTime-to-timezone", &[early, tz("PT0S")])), "FODT0001");
    assert!(call("adjust-date-to-timezone", &[empty()]).unwrap().items().is_empty());
}
