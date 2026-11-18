//! Tests of the operators past the common cases [XQuery §3.4, §3.5,
//! §3.12] [XQuery Appendix B.2]: each kind of number and its errors, each
//! combination of dates, times and durations the operator mapping has or
//! lacks, the comparisons of each type, and the conversions of the type
//! expressions.

use super::super::super::testing::{code_of, run};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §6.2,
/// §6.3] the arithmetic of each kind of number: decimals multiplied, a
/// quotient exact where it ends, of floats and doubles each operator, a
/// unary minus of each; exact numbers compared by sign; idiv of doubles
/// by zero err:FOAR0001, of NaN err:FOAR0002, mod of integers by zero
/// err:FOAR0001, a quotient past 1,024 digits err:FOAR0002.
#[test]
fn numbers_of_each_kind() {
    assert_eq!(
        run("1.5 * 2.5, 0.0000000000000000001 div 1, 0.0000000000000000001 div 0.5"),
        "3.75 0.0000000000000000001 0.0000000000000000002"
    );
    assert_eq!(run("1e0 - 2e0, 2e0 * 3e0, 7e0 mod 2e0, 7e0 idiv 2e0"), "-1 6 1 3");
    assert_eq!(run("-xs:float(1.5), -1.5e0, -1.5"), "-1.5 -1.5 -1.5");
    assert_eq!(run("1 gt -1, -1 lt 1, -1 lt -2, -2 lt -1, 10 gt 9"), "true true false true true");
    assert_eq!(code_of("1 mod 0"), "err:FOAR0001");
    assert_eq!(code_of("1e0 idiv 0e0"), "err:FOAR0001");
    assert_eq!(code_of("xs:double('NaN') idiv 1e0"), "err:FOAR0002");
    let big = "xs:integer(string-join(for $i in 1 to 1024 return '9', ''))";
    assert_eq!(code_of(&format!("{big} div 0.1")), "err:FOAR0002");
    assert_eq!(code_of("-'a'"), "err:XPTY0004");
    assert_eq!(run("-(), +()"), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery Appendix B.2]
/// [XQueryFO §10.6, §10.8] the operator mapping of dates, times and
/// durations: a duration added to a date from either side, but subtracted
/// from one only; a number times a duration; durations of one kind added
/// and subtracted; a date and time moved past its day; months subtracted
/// pinning the day; each combination the mapping lacks err:XPTY0004.
#[test]
fn dates_times_and_durations() {
    assert_eq!(run("xdt:dayTimeDuration('P1D') + xs:date('2026-01-01'), 2 * xdt:dayTimeDuration('PT1H')"), "2026-01-02 PT2H");
    assert_eq!(run("xs:date('2026-03-31') - xdt:yearMonthDuration('P1M')"), "2026-02-28");
    assert_eq!(run("xs:dateTime('2026-01-01T23:00:00') + xdt:dayTimeDuration('PT2H')"), "2026-01-02T01:00:00");
    let sums = "xdt:dayTimeDuration('P1D') + xdt:dayTimeDuration('PT1H'), xdt:dayTimeDuration('P1D') - xdt:dayTimeDuration('PT1H')";
    assert_eq!(run(sums), "P1DT1H PT23H");
    for lacking in [
        "xdt:dayTimeDuration('P1D') - xs:date('2026-01-01')",
        "xs:date('2026-01-01') * xdt:dayTimeDuration('P1D')",
        "xs:time('10:00:00') + xdt:yearMonthDuration('P1M')",
        "xdt:yearMonthDuration('P1Y') + xdt:dayTimeDuration('P1D')",
        "xdt:dayTimeDuration('P1D') * xdt:dayTimeDuration('P1D')",
        "xdt:dayTimeDuration('P1D') + 1",
        "2 div xdt:dayTimeDuration('P1D')",
    ] {
        assert_eq!(code_of(lacking), "err:XPTY0004", "{lacking}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §10.1.1,
/// §10.6.3] the errors of dates and durations: a date moved past the
/// years held err:FODT0001, a duration past 128 bits err:FODT0002, a
/// duration times NaN err:FOCA0005.
#[test]
fn errors_of_dates_and_durations() {
    assert_eq!(code_of("xs:date('9999-12-01') + xdt:yearMonthDuration('P1M')"), "err:FODT0001");
    let huge = "xdt:yearMonthDuration('P10000000000000000000000000000000000000Y')";
    assert_eq!(code_of(&format!("{huge} + {huge}")), "err:FODT0002");
    assert_eq!(code_of(&format!("xdt:yearMonthDuration('-P10000000000000000000000000000000000000Y') - {huge}")), "err:FODT0002");
    assert_eq!(code_of("xdt:dayTimeDuration('P1D') * xs:double('NaN')"), "err:FOCA0005");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.5.1, §3.5.2]
/// [XQueryFO §10.4] comparisons of each kind: durations of the two kinds
/// equal as durations, Gregorian values equal by their instants, each
/// general comparison operator, an untyped value cast to a date beside
/// one.
#[test]
fn comparisons_of_each_kind() {
    assert_eq!(
        run("xdt:yearMonthDuration('P1Y') eq xs:duration('P12M'), xs:duration('P1D') ne xdt:dayTimeDuration('PT24H')"),
        "true false"
    );
    assert_eq!(run("xs:gYear('2026') eq xs:gYear('2026'), xs:gYear('2026') ne xs:gYear('2025')"), "true true");
    assert_eq!(code_of("xs:gYear('2026') lt xs:gYear('2027')"), "err:XPTY0004");
    assert_eq!(run("(1, 5) < 2, (1, 5) <= 0, (1, 5) >= 5, (1, 5) > 5"), "true false true false");
    assert_eq!(run("<a>2026-01-01</a> = xs:date('2026-01-01')"), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.12.3,
/// §3.12.4, §3.12.6, §3.3.1, §4.14] `treat` of a value of its type is the
/// value; `cast` and `castable` of more than one value err:XPTY0004 and
/// false; a range of an empty operand empty, of more than one value
/// err:XPTY0004; a variable whose value is not of its declared type
/// err:XPTY0004.
#[test]
fn type_expressions_and_ranges() {
    assert_eq!(run("1 treat as xs:integer, (1, 2) castable as xs:integer"), "1 false");
    assert_eq!(code_of("(1, 2) cast as xs:integer"), "err:XPTY0004");
    assert_eq!(run("() to 3, 1 to ()"), "");
    assert_eq!(code_of("(1, 2) to 3"), "err:XPTY0004");
    assert_eq!(code_of("declare variable $x as xs:string := 1; $x"), "err:XPTY0004");
}
