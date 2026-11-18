//! Tests of the aggregate functions.

use super::super::testing::{atom, call, call_in, code, dbl, dec, empty, focus, int, s, seq, text, typed};
use crate::xml::AtomicType;

fn ym(text: &str) -> crate::xml::Sequence {
    atom(AtomicType::YearMonthDuration, text)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.4.2.1]
/// the examples of `fn:avg`: of integers a decimal, of year-month
/// durations one; of the empty sequence the empty sequence; NaN of
/// infinities of either sign and of NaN; mixed types err:FORG0006.
#[test]
fn avg_examples() {
    let seq3 = || seq(&[int(3), int(4), int(5)]);
    assert_eq!(typed(call("avg", &[seq3()])), (AtomicType::Decimal, "4".to_owned()));
    assert_eq!(text(call("avg", &[seq(&[ym("P20Y"), ym("P10M")])])), "P10Y5M");
    assert_eq!(code(call("avg", &[seq(&[ym("P20Y"), seq3()])])), "FORG0006");
    assert!(call("avg", &[empty()]).unwrap().items().is_empty());
    let float = |t: &str| atom(AtomicType::Float, t);
    assert_eq!(text(call("avg", &[seq(&[float("INF"), float("-INF")])])), "NaN");
    assert_eq!(text(call("avg", &[seq(&[seq3(), float("NaN")])])), "NaN");
    assert_eq!(typed(call("avg", &[seq(&[atom(AtomicType::UntypedAtomic, "1"), int(2)])])), (AtomicType::Double, "1.5".to_owned()));
    assert_eq!(
        text(call("avg", &[seq(&[atom(AtomicType::DayTimeDuration, "PT1H"), atom(AtomicType::DayTimeDuration, "PT2H")])])),
        "PT1H30M"
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.4.3.1,
/// §15.4.4.1] the examples of `fn:max` and `fn:min`: the numbers promoted
/// to one type; strings by the collation; dates in the implicit time zone;
/// NaN if one is; mixed or unordered types err:FORG0006.
#[test]
fn max_and_min_examples() {
    assert_eq!(text(call("max", &[seq(&[int(3), int(4), int(5)])])), "5");
    assert_eq!(typed(call("max", &[seq(&[int(5), dbl("5.0e0")])])), (AtomicType::Double, "5".to_owned()));
    assert_eq!(code(call("max", &[seq(&[int(3), int(4), s("Zero")])])), "FORG0006");
    assert_eq!(text(call("max", &[seq(&[s("a"), s("b"), s("c")])])), "c");
    assert_eq!(text(call("min", &[seq(&[int(3), int(4), int(5)])])), "3");
    assert_eq!(typed(call("min", &[seq(&[int(5), dbl("5.0e0")])])), (AtomicType::Double, "5".to_owned()));
    assert_eq!(text(call("min", &[seq(&[s("a"), s("b"), s("c")])])), "a");
    assert_eq!(text(call("min", &[seq(&[s("b"), s("A")]), s("urn:turborel:collation:CASE_INSENSITIVE")])), "A");
    assert_eq!(text(call("min", &[seq(&[dec("1.5"), dbl("NaN"), int(0)])])), "NaN");
    let dates = seq(&[atom(AtomicType::Date, "2001-01-01"), atom(AtomicType::Date, "2000-12-31Z")]);
    assert_eq!(text(call_in(&focus(), "min", &[dates])), "2000-12-31Z");
    assert_eq!(code(call("max", &[seq(&[atom(AtomicType::GYear, "2000"), atom(AtomicType::GYear, "2001")])])), "FORG0006");
    assert_eq!(code(call("max", &[atom(AtomicType::QName, "x")])), "FORG0006");
    assert!(call("min", &[empty()]).unwrap().items().is_empty());
    assert_eq!(text(call("max", &[atom(AtomicType::UntypedAtomic, "7")])), "7");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.4.5.1]
/// the examples of `fn:sum`: of durations a duration; of the empty
/// sequence the integer 0 or the zero given; mixed types err:FORG0006.
#[test]
fn sum_examples() {
    assert_eq!(text(call("sum", &[seq(&[ym("P20Y"), ym("P10M")])])), "P20Y10M");
    assert_eq!(text(call("sum", &[empty(), ym("P0M")])), "P0M");
    assert_eq!(typed(call("sum", &[seq(&[int(3), int(4), int(5)])])), (AtomicType::Integer, "12".to_owned()));
    assert_eq!(typed(call("sum", &[empty()])), (AtomicType::Integer, "0".to_owned()));
    assert!(call("sum", &[empty(), empty()]).unwrap().items().is_empty());
    assert_eq!(text(call("sum", &[empty(), int(0)])), "0");
    assert_eq!(code(call("sum", &[seq(&[ym("P20Y"), dbl("9E1")])])), "FORG0006");
    assert_eq!(typed(call("sum", &[seq(&[atom(AtomicType::Short, "1"), dec("0.5")])])), (AtomicType::Decimal, "1.5".to_owned()));
    assert_eq!(typed(call("sum", &[atom(AtomicType::Byte, "7")])), (AtomicType::Integer, "7".to_owned()));
    assert_eq!(code(call("sum", &[s("1")])), "FORG0006");
}
