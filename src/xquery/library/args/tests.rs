//! Tests of the function conversion rules.

use super::super::testing::{atom, code, document, empty, focus, in_context, int, s, seq};
use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.1.5]
/// [XQueryFO §2.4] an argument is atomized: a node its typed value; one
/// value where one is wanted, none where none is allowed, more a type
/// error, err:XPTY0004.
#[test]
fn arguments_are_atomized() {
    let doc = document("<a>12</a>");
    let node = Sequence::new(vec![Item::Node(doc)]);
    assert_eq!(atomized(&node).ok(), Some(vec![Atomic::new(AtomicType::UntypedAtomic, "12")]));
    assert_eq!(optional_atomic(&empty()).unwrap(), None);
    assert_eq!(code(optional_atomic(&seq(&[int(1), int(2)])).map(|_| empty())), "XPTY0004");
    assert_eq!(code(atomic(&empty()).map(|_| empty())), "XPTY0004");
    assert_eq!(integer(&node).unwrap().text(), "12");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.1.5,
/// Appendix B.1] a string parameter takes a string, a type derived from
/// one, an untyped value or a URI; a number is promoted to a double; an
/// untyped value is cast to the parameter's type; anything else is
/// err:XPTY0004, an untyped value of no such form err:FORG0001.
#[test]
fn values_are_converted_to_parameter_types() {
    assert_eq!(string(&atom(AtomicType::Token, "t")).unwrap(), "t");
    assert_eq!(string(&atom(AtomicType::AnyUri, "http://x")).unwrap(), "http://x");
    assert_eq!(string(&atom(AtomicType::UntypedAtomic, "u")).unwrap(), "u");
    assert_eq!(code(string(&int(1)).map(|_| empty())), "XPTY0004");
    assert_eq!(string_or_empty(&empty()).unwrap(), "");
    assert_eq!(strings(&seq(&[s("a"), s("b")])).unwrap(), vec!["a", "b"]);
    assert!((double(&int(3)).unwrap() - 3.0_f64).abs() < f64::EPSILON);
    assert!((double(&atom(AtomicType::UntypedAtomic, "2.5")).unwrap() - 2.5_f64).abs() < f64::EPSILON);
    assert_eq!(code(double(&s("1")).map(|_| empty())), "XPTY0004");
    assert_eq!(code(double(&atom(AtomicType::UntypedAtomic, "x")).map(|_| empty())), "FORG0001");
    assert_eq!(code(integer(&atom(AtomicType::Decimal, "1")).map(|_| empty())), "XPTY0004");
    assert_eq!(one_of(&atom(AtomicType::UntypedAtomic, "2001-01-01"), AtomicType::Date).unwrap().kind, AtomicType::Date);
    assert_eq!(optional_of(&atom(AtomicType::Int, "5"), AtomicType::Integer).unwrap().unwrap().kind, AtomicType::Int);
    assert_eq!(code(one_of(&s("2001-01-01"), AtomicType::Date).map(|_| empty())), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.3,
/// §14.1] a node parameter takes one node or none; the context item is
/// read where an argument is not given, err:FONC0001 if there is none and
/// err:FOTY0011 if a node is wanted and it is no node.
#[test]
fn nodes_and_the_context_item() {
    let doc = Sequence::new(vec![Item::Node(document("<a/>"))]);
    assert!(optional_node(&doc).unwrap().is_some());
    assert!(optional_node(&empty()).unwrap().is_none());
    assert_eq!(code(optional_node(&int(1)).map(|_| empty())), "XPTY0004");
    assert_eq!(code(node(&seq(&[doc.clone(), doc.clone()])).map(|_| empty())), "XPTY0004");
    assert_eq!(code(node(&empty()).map(|_| empty())), "XPTY0004");
    let none = focus();
    assert_eq!(in_context(&none, |c| code(or_context(&[], 0, c))), "FONC0001");
    let atomic_focus = none.at(Item::Atomic(Atomic::new(AtomicType::String, "x")), 1, 1);
    assert_eq!(in_context(&atomic_focus, |c| or_context(&[], 0, c).unwrap().items().len()), 1);
    assert_eq!(in_context(&atomic_focus, |c| code(or_context_node(&[], 0, c))), "FOTY0011");
    assert_eq!(in_context(&atomic_focus, |c| or_context_node(&[int(1)], 0, c).unwrap().items().len()), 1);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] (RULES.md D18, D86) a
/// string or sequence past the library's bounds is SQLCODE -900, one
/// within them is made.
#[test]
fn results_are_bounded() {
    assert!(string_value("x".repeat(MAX_STRING)).is_ok());
    assert_eq!(string_value("x".repeat(MAX_STRING.saturating_add(1))).unwrap_err().sqlcode(), -900_i32);
    assert!(within_items(MAX_ITEMS).is_ok());
    assert_eq!(within_items(MAX_ITEMS.saturating_add(1)).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(boolean(true).items().len(), 1);
    assert_eq!(optional(None).items().len(), 0);
}
