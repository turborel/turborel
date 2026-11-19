//! Tests of the error and trace functions.

use super::super::testing::{call, code, empty, int, s, seq, texts};
use super::*;
use crate::error::SqlState;
use crate::xml::{Item, Sequence};

fn qname(uri: Option<&str>, lexical: &str) -> Sequence {
    Sequence::new(vec![Item::Atomic(Atomic::qname(uri, lexical))])
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §3.1] the
/// examples of `fn:error`: of no argument err:FOER0000; of a QName the
/// error it names, as `NS#LP` with its description; of the empty
/// sequence and a description err:FOER0000; of the errors' own namespace
/// the code itself.
#[test]
fn errors_are_raised() {
    assert_eq!(code(call("error", &[])), "FOER0000");
    let named =
        call("error", &[qname(Some("http://www.example.com/HR"), "myerr:toohighsal"), s("Does not apply because salary is too high")]);
    let error = named.unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR);
    let message = error.to_string();
    assert!(message.contains("http://www.example.com/HR#toohighsal"), "{message}");
    assert!(message.contains("Does not apply because salary is too high"), "{message}");
    assert_eq!(code(call("error", &[empty(), s("why")])), "FOER0000");
    assert_eq!(code(call("error", &[qname(Some(ERR_NAMESPACE), "err:FOAR0001")])), "FOAR0001");
    let object = call("error", &[qname(None, "local"), s("d"), seq(&[int(1), int(2)])]).unwrap_err();
    assert!(object.to_string().contains("#local: d"), "{object}");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §3] an
/// argument that is no QName, or a QName that must be given and is not,
/// is err:XPTY0004.
#[test]
fn error_arguments_are_typed() {
    assert_eq!(code(call("error", &[s("x")])), "XPTY0004");
    assert_eq!(code(call("error", &[empty()])), "XPTY0004");
    assert_eq!(code(call("error", &[qname(None, "x"), int(1)])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §4.1]
/// `fn:trace` returns its value unchanged; its label must be a string.
#[test]
fn trace_returns_its_value() {
    let value = seq(&[int(1), s("a")]);
    assert_eq!(texts(&call("trace", &[value, s("the value of $v is:")]).unwrap()), vec!["1", "a"]);
    assert!(call("trace", &[empty(), s("")]).unwrap().items().is_empty());
    assert_eq!(code(call("trace", &[int(1), int(2)])), "XPTY0004");
}
