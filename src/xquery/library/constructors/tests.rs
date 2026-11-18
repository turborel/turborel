//! Tests of the constructor functions.

use super::super::lookup;
use super::super::testing::{atom, call, code, empty, focus, int, s, seq, typed};
use super::*;

/// The value of the constructor of `kind` of `value`.
fn construct_of(kind: AtomicType, value: &Sequence) -> Result<Sequence> {
    let (namespace, local) = Constructor::of(kind).name();
    lookup(namespace, local, 1).unwrap().call(&focus(), std::slice::from_ref(value))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §5.1] a
/// constructor is a cast: `xs:unsignedInt(12)` and `xs:unsignedInt("12")`
/// the value 12; a value its type refuses err:FORG0001; the empty sequence
/// of the empty sequence; more than one value err:XPTY0004.
#[test]
fn constructors_cast() {
    assert_eq!(typed(construct_of(AtomicType::UnsignedInt, &int(12))), (AtomicType::UnsignedInt, "12".to_owned()));
    assert_eq!(typed(construct_of(AtomicType::UnsignedInt, &s("12"))), (AtomicType::UnsignedInt, "12".to_owned()));
    assert_eq!(code(construct_of(AtomicType::UnsignedInt, &int(-1))), "FORG0001");
    assert_eq!(typed(construct_of(AtomicType::DayTimeDuration, &s("PT36H"))).1, "P1DT12H");
    assert_eq!(typed(construct_of(AtomicType::UntypedAtomic, &int(7))), (AtomicType::UntypedAtomic, "7".to_owned()));
    assert!(construct_of(AtomicType::Date, &empty()).unwrap().items().is_empty());
    assert_eq!(code(construct_of(AtomicType::Integer, &seq(&[int(1), int(2)]))), "XPTY0004");
    assert_eq!(code(construct_of(AtomicType::QName, &s("a:b"))), "XPST0083");
    assert_eq!(construct(AtomicType::Boolean, &int(0)).unwrap().items().len(), 1);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §5.2.1]
/// the examples of `fn:dateTime`, 24:00:00 the start of the day given; a
/// time zone either argument has; different time zones err:FORG0008; an
/// argument of another type err:XPTY0004.
#[test]
fn date_times_of_dates_and_times() {
    let date = |t: &str| atom(AtomicType::Date, t);
    let time = |t: &str| atom(AtomicType::Time, t);
    assert_eq!(typed(call("dateTime", &[date("1999-12-31"), time("12:00:00")])), (AtomicType::DateTime, "1999-12-31T12:00:00".to_owned()));
    assert_eq!(typed(call("dateTime", &[date("1999-12-31"), time("24:00:00")])).1, "1999-12-31T00:00:00");
    assert_eq!(typed(call("dateTime", &[date("1999-12-31Z"), time("12:00:00")])).1, "1999-12-31T12:00:00Z");
    assert_eq!(typed(call("dateTime", &[date("1999-12-31"), time("12:00:00-05:00")])).1, "1999-12-31T12:00:00-05:00");
    assert_eq!(typed(call("dateTime", &[date("1999-12-31+01:00"), time("12:00:00+01:00")])).1, "1999-12-31T12:00:00+01:00");
    assert_eq!(code(call("dateTime", &[date("1999-12-31Z"), time("12:00:00+01:00")])), "FORG0008");
    assert_eq!(code(call("dateTime", &[s("1999-12-31"), time("12:00:00")])), "XPTY0004");
    assert_eq!(code(call("dateTime", &[empty(), time("12:00:00")])), "XPTY0004");
    let untyped = atom(AtomicType::UntypedAtomic, "2000-01-01");
    assert_eq!(typed(call("dateTime", &[untyped, time("01:02:03.5")])).1, "2000-01-01T01:02:03.5");
}
