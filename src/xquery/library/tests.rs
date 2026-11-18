//! Tests of the library's seam: finding a function by its name and arity.

use super::*;
use crate::xml::{XS_NAMESPACE, XS_PREFIX};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.1.5]
/// [XQueryFO §1.2, §1.4] a function is found by its expanded QName and the
/// number of its arguments: one of an optional argument by either count,
/// none of a count it does not take or of another namespace.
#[test]
fn functions_are_found_by_name_and_arity() {
    let round = lookup(FN_NAMESPACE, "round-half-to-even", 1).expect("one argument");
    assert_eq!(round.name(), (FN_NAMESPACE, "round-half-to-even"));
    assert_eq!(round.arity(), 1..=2);
    assert!(lookup(FN_NAMESPACE, "round-half-to-even", 2).is_some());
    assert!(lookup(FN_NAMESPACE, "round-half-to-even", 3).is_none());
    assert!(lookup(XDT_NAMESPACE, "round-half-to-even", 1).is_none());
    assert!(lookup(FN_NAMESPACE, "no-such-function", 0).is_none());
    assert!(lookup(FN_NAMESPACE, "string", 0).is_some());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §5.1] each
/// built-in atomic type but `xs:NOTATION` has a constructor, in the
/// namespace of its type's name; no name is two functions of one arity.
#[test]
fn constructors_and_names_are_each_found_once() {
    for kind in crate::xml::AtomicType::all() {
        let name = kind.name();
        let (prefix, local) = name.split_once(':').unwrap();
        let namespace = if prefix == XS_PREFIX { XS_NAMESPACE } else { XDT_NAMESPACE };
        let found = lookup(namespace, local, 1);
        assert_eq!(found.is_some(), kind != crate::xml::AtomicType::Notation, "{name}");
    }
    let all: Vec<_> = all().collect();
    for (i, f) in all.iter().enumerate() {
        for g in all.iter().skip(i.saturating_add(1)) {
            let overlap = f.arity().start() <= g.arity().end() && g.arity().start() <= f.arity().end();
            assert!(f.name() != g.name() || !overlap, "{:?} twice", f.name());
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1] [XQuery Appendix C.2]
/// a focus starts with no context item, the code point collation, no base
/// URI and the clock's time zone as the implicit one; one at an item keeps
/// the rest.
#[test]
fn a_focus_starts_empty() {
    let now = Now { utc: 7, zone: 90 };
    let focus = Focus::new(now);
    assert!(focus.item.is_none());
    assert_eq!((focus.timezone_minutes, focus.collation.as_str(), focus.base_uri.as_deref()), (90_i32, CODEPOINT_COLLATION, None));
    let item = Item::Atomic(crate::xml::Atomic::new(crate::xml::AtomicType::String, "x"));
    let at = focus.at(item, 2, 3);
    assert_eq!((at.position, at.size, at.now), (2, 3, now));
}
