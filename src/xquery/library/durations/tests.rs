//! Tests of durations.

use super::super::testing::{atom, call, code, empty, s, text, typed};
use super::*;

fn ym(text: &str) -> Atomic {
    Atomic::new(AtomicType::YearMonthDuration, text)
}

fn dt(text: &str) -> Atomic {
    Atomic::new(AtomicType::DayTimeDuration, text)
}

fn dbl(text: &str) -> Atomic {
    Atomic::new(AtomicType::Double, text)
}

/// The lexical form of `r`'s value.
fn shown(r: Result<Atomic>) -> String {
    r.unwrap().lexical.into_string()
}

/// The error code of `r`.
fn error(r: Result<Atomic>) -> String {
    let message = r.unwrap_err().to_string();
    let (_, after) = message.split_once("err:").unwrap();
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.4.1 to
/// §10.4.7.1] the comparisons of the two ordered durations, and the
/// examples of `op:duration-equal`; an operand of another type
/// err:XPTY0004.
#[test]
fn comparisons() {
    assert!(year_month_duration_equal(&ym("P1Y"), &ym("P12M")).unwrap());
    assert!(year_month_duration_less_than(&ym("-P1M"), &ym("P0M")).unwrap());
    assert!(year_month_duration_greater_than(&ym("P2Y"), &ym("P23M")).unwrap());
    assert!(day_time_duration_equal(&dt("PT24H"), &dt("P1D")).unwrap());
    assert!(day_time_duration_less_than(&dt("PT0.5S"), &dt("PT1S")).unwrap());
    assert!(day_time_duration_greater_than(&dt("P1D"), &dt("PT23H")).unwrap());
    let d = |t: &str| Atomic::new(AtomicType::Duration, t);
    assert!(duration_equal(&d("P1Y"), &d("P12M")).unwrap());
    assert!(duration_equal(&d("PT24H"), &d("P1D")).unwrap());
    assert!(!duration_equal(&d("P1Y"), &d("P365D")).unwrap());
    assert!(duration_equal(&ym("P1Y"), &d("P12M")).unwrap());
    assert_eq!(duration_order(&dt("PT1S"), &dt("PT2S")).unwrap(), Ordering::Less);
    assert_eq!(duration_order(&ym("P1Y"), &ym("P1M")).unwrap(), Ordering::Greater);
    assert_eq!(error(year_month_duration_equal(&dt("P1D"), &ym("P1M")).map(|_| ym("P0M"))), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.6.1.1
/// to §10.6.10.1] the examples of the arithmetic of durations; a NaN
/// factor err:FOCA0005; an infinite one, or a zero divisor, err:FODT0002;
/// an infinite divisor a zero duration.
#[test]
fn arithmetic_examples() {
    let cases = [
        (shown(add_year_month_durations(&ym("P2Y11M"), &ym("P3Y3M"))), "P6Y2M"),
        (shown(subtract_year_month_durations(&ym("P2Y11M"), &ym("P3Y3M"))), "-P4M"),
        (shown(multiply_year_month_duration(&ym("P2Y11M"), &dbl("2.3"))), "P6Y9M"),
        (shown(divide_year_month_duration(&ym("P2Y11M"), &dbl("1.5"))), "P1Y11M"),
        (shown(divide_year_month_duration_by_year_month_duration(&ym("P3Y4M"), &ym("-P1Y4M"))), "-2.5"),
        (shown(add_day_time_durations(&dt("P2DT12H5M"), &dt("P5DT12H"))), "P8DT5M"),
        (shown(subtract_day_time_durations(&dt("P2DT12H"), &dt("P1DT10H30M"))), "P1DT1H30M"),
        (shown(multiply_day_time_duration(&dt("PT2H10M"), &dbl("2.1"))), "PT4H33M"),
        (shown(divide_day_time_duration(&dt("P1DT2H30M10.5S"), &dbl("1.5"))), "PT17H40M7S"),
        (shown(divide_day_time_duration_by_day_time_duration(&dt("P2DT53M11S"), &dt("P1DT10H"))), "1.437834967320261437"),
        (shown(multiply_year_month_duration(&ym("P1M"), &dbl("0.5"))), "P1M"),
        (shown(multiply_year_month_duration(&ym("P1M"), &dbl("0.49"))), "P0M"),
        (shown(multiply_day_time_duration(&dt("P1D"), &dbl("-0"))), "PT0S"),
        (shown(divide_day_time_duration(&dt("P1D"), &dbl("INF"))), "PT0S"),
        (error(multiply_day_time_duration(&dt("P1D"), &dbl("NaN"))), "FOCA0005"),
        (error(multiply_day_time_duration(&dt("P1D"), &dbl("INF"))), "FODT0002"),
        (error(divide_year_month_duration(&ym("P1M"), &dbl("0"))), "FODT0002"),
        (error(divide_day_time_duration_by_day_time_duration(&dt("P1D"), &dt("PT0S"))), "FOAR0001"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
    let most = ym(&format!("P{}M", i128::MAX));
    assert_eq!(error(add_year_month_durations(&most, &ym("P1M"))), "FODT0002");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.5.1.1
/// to §10.5.6.1] the examples of the component functions of durations,
/// each of the canonical form and of its sign; the empty sequence of the
/// empty sequence; an untyped value cast; a duration of the other kind
/// err:XPTY0004.
#[test]
fn component_examples() {
    let of = |f: &str, d: Atomic| text(call(f, &[Sequence::new(vec![crate::xml::Item::Atomic(d)])]));
    let cases = [
        (of("years-from-duration", ym("P20Y15M")), "21"),
        (of("years-from-duration", ym("-P15M")), "-1"),
        (of("months-from-duration", ym("P20Y15M")), "3"),
        (of("months-from-duration", ym("-P20Y18M")), "-6"),
        (of("days-from-duration", dt("P3DT10H")), "3"),
        (of("days-from-duration", dt("P3DT55H")), "5"),
        (of("hours-from-duration", dt("P3DT10H")), "10"),
        (of("hours-from-duration", dt("P3DT12H32M12S")), "12"),
        (of("hours-from-duration", dt("PT123H")), "3"),
        (of("hours-from-duration", dt("-P3DT10H")), "-10"),
        (of("minutes-from-duration", dt("P3DT10H")), "0"),
        (of("minutes-from-duration", dt("-P5DT12H30M")), "-30"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
    assert_eq!(
        typed(call("seconds-from-duration", &[atom(AtomicType::DayTimeDuration, "P3DT10H12.5S")])),
        (AtomicType::Decimal, "12.5".to_owned())
    );
    assert_eq!(of("seconds-from-duration", dt("-PT256S")), "-16");
    assert_eq!(text(call("years-from-duration", &[atom(AtomicType::UntypedAtomic, "P2Y")])), "2");
    assert!(call("days-from-duration", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("days-from-duration", &[s("P1D")])), "XPTY0004");
    assert_eq!(code(call("years-from-duration", &[atom(AtomicType::DayTimeDuration, "P1D")])), "XPTY0004");
}
