//! Tests of the library's tables: each function found by its name and
//! arity among the built-in functions.

use super::super::functions::{FN_NAMESPACE, Library};
use super::*;
use crate::xml::{QName, Sequence};

/// The name `local` of the namespace `uri`.
fn name(uri: &str, local: &str) -> QName {
    QName { prefix: None, uri: Some(uri.into()), local: local.into() }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.1.5]
/// [XQueryFO §1.2, §1.4] a function is found by its expanded QName and the
/// number of its arguments: one of an optional argument by either count,
/// fn:concat by two or more, none of a count it does not take or of
/// another namespace; the constructor functions are no functions of the
/// library, as a call of one is a cast.
#[test]
fn functions_are_found_by_name_and_arity() {
    let library = Library::builtin();
    let round = name(FN_NAMESPACE, "round-half-to-even");
    assert!(library.find(&round, 1).is_some() && library.find(&round, 2).is_some());
    assert!(library.find(&round, 3).is_none() && library.find(&round, 0).is_none());
    assert!(library.find(&name("urn:other", "round-half-to-even"), 1).is_none());
    assert!(library.find(&name(FN_NAMESPACE, "no-such-function"), 0).is_none());
    assert!(library.find(&name(FN_NAMESPACE, "concat"), 9).is_some() && library.find(&name(FN_NAMESPACE, "concat"), 1).is_none());
    assert!(library.find(&name(crate::xml::XS_NAMESPACE, "integer"), 1).is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §1.4] no
/// name and arity is two functions of the tables, so each is found alone.
#[test]
fn no_name_and_arity_is_two_functions() {
    let all: Vec<&Builtin> = all().collect();
    for (i, f) in all.iter().enumerate() {
        assert!(f.least <= f.most, "{}", f.local);
        for g in all.iter().skip(i.saturating_add(1)) {
            let overlap = f.least <= g.most && g.least <= f.most;
            assert!(f.local != g.local || !overlap, "fn:{} twice", f.local);
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §1.4] a row
/// of a section's table holds its local name, the fewest and most
/// arguments it takes, and its body.
#[test]
fn a_row_holds_its_signature() {
    let row = Builtin::function("f", 1, 2, |_, _| Ok(Sequence::empty()));
    assert_eq!((row.local, row.least, row.most), ("f", 1, 2));
    assert!(testing::in_context(&testing::focus(), |c| (row.body)(c, &[])).unwrap().items().is_empty());
}
