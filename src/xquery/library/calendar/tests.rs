//! Tests of the arithmetic of dates and times.

use super::*;

fn at(kind: AtomicType, text: &str) -> Atomic {
    Atomic::new(kind, text)
}

fn shown(r: Result<Atomic>) -> String {
    r.unwrap().lexical.into_string()
}

fn error(r: Result<Atomic>) -> String {
    let message = r.unwrap_err().to_string();
    let (_, after) = message.split_once("err:").unwrap();
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.8.1.1
/// to §10.8.3.1] the examples of the differences of dates and times, a
/// value of no time zone in the implicit one.
#[test]
fn difference_examples() {
    let dt = |t: &str| at(AtomicType::DateTime, t);
    assert_eq!(shown(subtract_date_times(&dt("2000-10-30T06:12:00"), &dt("1999-11-28T09:00:00Z"), -300)), "P337DT2H12M");
    let d = |t: &str| at(AtomicType::Date, t);
    assert_eq!(shown(subtract_dates(&d("2000-10-30"), &d("1999-11-28"), 0)), "P337D");
    assert_eq!(shown(subtract_dates(&d("2000-10-30"), &d("1999-11-28Z"), 300)), "P336DT19H");
    assert_eq!(shown(subtract_dates(&d("2000-10-15-05:00"), &d("2000-10-10+02:00"), 0)), "P5DT7H");
    let t = |x: &str| at(AtomicType::Time, x);
    assert_eq!(shown(subtract_times(&t("11:12:00Z"), &t("04:00:00"), -300)), "PT2H12M");
    assert_eq!(shown(subtract_times(&t("11:00:00-05:00"), &t("21:30:00+05:30"), -300)), "PT0S");
    assert_eq!(shown(subtract_times(&t("17:00:00-06:00"), &t("08:00:00+09:00"), -300)), "P1D");
    assert_eq!(shown(subtract_times(&t("24:00:00"), &t("23:59:59"), -300)), "-PT23H59M59S");
    assert_eq!(error(subtract_dates(&d("2000-10-30"), &t("04:00:00"), 0)), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.8.4.1
/// to §10.8.13.1] [Schema2 Appendix E] the examples of a duration added
/// to or subtracted from a date and time, a date or a time: months first,
/// a day kept within its month; a time wrapping round its day; the time
/// zone kept.
#[test]
fn addition_examples() {
    let dt = |t: &str| at(AtomicType::DateTime, t);
    let d = |t: &str| at(AtomicType::Date, t);
    let t = |x: &str| at(AtomicType::Time, x);
    let ym = |x: &str| at(AtomicType::YearMonthDuration, x);
    let dtd = |x: &str| at(AtomicType::DayTimeDuration, x);
    assert_eq!(shown(add_year_month_duration_to_date_time(&dt("2000-10-30T11:12:00"), &ym("P1Y2M"))), "2001-12-30T11:12:00");
    assert_eq!(shown(add_day_time_duration_to_date_time(&dt("2000-10-30T11:12:00"), &dtd("P3DT1H15M"))), "2000-11-02T12:27:00");
    assert_eq!(shown(subtract_year_month_duration_from_date_time(&dt("2000-10-30T11:12:00"), &ym("P1Y2M"))), "1999-08-30T11:12:00");
    assert_eq!(shown(subtract_day_time_duration_from_date_time(&dt("2000-10-30T11:12:00"), &dtd("P3DT1H15M"))), "2000-10-27T09:57:00");
    assert_eq!(shown(add_year_month_duration_to_date(&d("2000-10-30"), &ym("P1Y2M"))), "2001-12-30");
    assert_eq!(shown(add_day_time_duration_to_date(&d("2004-10-30Z"), &dtd("P2DT2H30M0S"))), "2004-11-01Z");
    assert_eq!(shown(subtract_year_month_duration_from_date(&d("2000-10-30"), &ym("P1Y2M"))), "1999-08-30");
    assert_eq!(shown(subtract_year_month_duration_from_date(&d("2000-02-29Z"), &ym("P1Y"))), "1999-02-28Z");
    assert_eq!(shown(subtract_year_month_duration_from_date(&d("2000-10-31-05:00"), &ym("P1Y1M"))), "1999-09-30-05:00");
    assert_eq!(shown(subtract_day_time_duration_from_date(&d("2000-10-30"), &dtd("P3DT1H15M"))), "2000-10-26");
    assert_eq!(shown(add_day_time_duration_to_time(&t("11:12:00"), &dtd("P3DT1H15M"))), "12:27:00");
    assert_eq!(shown(add_day_time_duration_to_time(&t("23:12:00+03:00"), &dtd("P1DT3H15M"))), "02:27:00+03:00");
    assert_eq!(shown(subtract_day_time_duration_from_time(&t("11:12:00"), &dtd("P3DT1H15M"))), "09:57:00");
    assert_eq!(shown(subtract_day_time_duration_from_time(&t("08:20:00-05:00"), &dtd("P23DT10H10M"))), "22:10:00-05:00");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.1.1] a
/// result past 9999 or before 0001 err:FODT0001; a duration of the wrong
/// kind err:XPTY0004.
#[test]
fn addition_errors() {
    let dt = at(AtomicType::DateTime, "9999-12-31T23:00:00");
    assert_eq!(error(add_day_time_duration_to_date_time(&dt, &at(AtomicType::DayTimeDuration, "PT1H"))), "FODT0001");
    let early = at(AtomicType::Date, "0001-01-15");
    assert_eq!(error(subtract_year_month_duration_from_date(&early, &at(AtomicType::YearMonthDuration, "P1M"))), "FODT0001");
    assert_eq!(error(add_year_month_duration_to_date(&early, &at(AtomicType::DayTimeDuration, "P1D"))), "XPTY0004");
}
