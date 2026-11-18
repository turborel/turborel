//! Tests of the context functions.

use super::super::testing::{call, call_in, code, focus, text, typed};
use super::*;
use crate::xml::Item;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §16.1,
/// §16.2] the context position and size; err:FONC0001 with no context
/// item.
#[test]
fn position_and_last() {
    let at = focus().at(Item::Atomic(Atomic::new(AtomicType::String, "x")), 2, 5);
    assert_eq!(typed(call_in(&at, "position", &[])), (AtomicType::Integer, "2".to_owned()));
    assert_eq!(text(call_in(&at, "last", &[])), "5");
    assert_eq!(code(call("position", &[])), "FONC0001");
    assert_eq!(code(call("last", &[])), "FONC0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §16.3.1 to
/// §16.6] the current date and time, its date and its time, of the
/// statement's instant in the implicit time zone, the same at every call;
/// the implicit time zone as a duration.
#[test]
fn current_date_and_time() {
    let f = focus();
    assert_eq!(typed(call_in(&f, "current-dateTime", &[])), (AtomicType::DateTime, "2004-05-12T13:17:15.125-05:00".to_owned()));
    assert_eq!(text(call_in(&f, "current-dateTime", &[])), text(call_in(&f, "current-dateTime", &[])));
    assert_eq!(typed(call_in(&f, "current-date", &[])), (AtomicType::Date, "2004-05-12-05:00".to_owned()));
    assert_eq!(typed(call_in(&f, "current-time", &[])), (AtomicType::Time, "13:17:15.125-05:00".to_owned()));
    assert_eq!(typed(call_in(&f, "implicit-timezone", &[])), (AtomicType::DayTimeDuration, "-PT5H".to_owned()));
    let utc = Focus { timezone_minutes: 0, ..focus() };
    assert_eq!(text(call_in(&utc, "current-dateTime", &[])), "2004-05-12T18:17:15.125Z");
    let east = Focus { timezone_minutes: 600, ..focus() };
    assert_eq!(text(call_in(&east, "current-date", &[])), "2004-05-13+10:00");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §16.7,
/// §16.8] the default collation, the code point collation by default, and
/// the static base URI, the empty sequence if there is none.
#[test]
fn collation_and_base_uri() {
    assert_eq!(text(call("default-collation", &[])), super::super::CODEPOINT_COLLATION);
    assert!(call("static-base-uri", &[]).unwrap().items().is_empty());
    let based = Focus { base_uri: Some("http://e.org/".to_owned()), collation: "UNICODE".to_owned(), ..focus() };
    assert_eq!(typed(call_in(&based, "static-base-uri", &[])), (AtomicType::AnyUri, "http://e.org/".to_owned()));
    assert_eq!(text(call_in(&based, "default-collation", &[])), "UNICODE");
}
