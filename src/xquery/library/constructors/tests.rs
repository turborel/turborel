//! Tests of `fn:dateTime`.

use super::super::testing::{atom, call, code, empty, s, typed};
use super::*;

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
