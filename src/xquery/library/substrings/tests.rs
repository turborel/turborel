//! Tests of the functions based on substring matching.

use super::super::testing::{call, code, empty, s, text};

/// The boolean `fn:local` gives of `args`, as text.
fn truth(local: &str, args: &[crate::xml::Sequence]) -> String {
    text(call(local, args))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5.1.1,
/// §7.5.2.1, §7.5.3.1] the examples of `fn:contains`, `fn:starts-with` and
/// `fn:ends-with` by the code point collation, the empty sequence the
/// zero-length string.
#[test]
fn contains_starts_and_ends() {
    assert_eq!(truth("contains", &[s("tattoo"), s("t")]), "true");
    assert_eq!(truth("contains", &[s("tattoo"), s("ttt")]), "false");
    assert_eq!(truth("contains", &[s(""), empty()]), "true");
    assert_eq!(truth("contains", &[empty(), s("a")]), "false");
    assert_eq!(truth("starts-with", &[s("tattoo"), s("tat")]), "true");
    assert_eq!(truth("starts-with", &[s("tattoo"), s("att")]), "false");
    assert_eq!(truth("starts-with", &[empty(), empty()]), "true");
    assert_eq!(truth("ends-with", &[s("tattoo"), s("tattoo")]), "true");
    assert_eq!(truth("ends-with", &[s("tattoo"), s("atto")]), "false");
    assert_eq!(truth("ends-with", &[empty(), empty()]), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5.4.1,
/// §7.5.5.1] the examples of `fn:substring-before` and
/// `fn:substring-after`.
#[test]
fn before_and_after() {
    assert_eq!(text(call("substring-before", &[s("tattoo"), s("attoo")])), "t");
    assert_eq!(text(call("substring-before", &[s("tattoo"), s("tatto")])), "");
    assert_eq!(text(call("substring-before", &[empty(), empty()])), "");
    assert_eq!(text(call("substring-before", &[s("abc"), s("x")])), "");
    assert_eq!(text(call("substring-after", &[s("tattoo"), s("tat")])), "too");
    assert_eq!(text(call("substring-after", &[s("tattoo"), s("tattoo")])), "");
    assert_eq!(text(call("substring-after", &[empty(), empty()])), "");
    assert_eq!(text(call("substring-after", &[s("Eureka!"), s("")])), "Eureka!");
    assert_eq!(text(call("substring-after", &[s("abc"), s("x")])), "");
    assert_eq!(text(call("substring-after", &[s("äöüäöü"), s("ü")])), "äöü");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5] by a
/// collation of one character at a time, units match by its folds, the
/// characters given back the text's own; a collation of no units
/// err:FOCH0004; an unknown one err:FOCH0002.
#[test]
fn matching_by_collation_units() {
    let ci = s("CASE_INSENSITIVE");
    assert_eq!(truth("contains", &[s("Tattoo"), s("TTO"), ci.clone()]), "true");
    assert_eq!(text(call("substring-before", &[s("HelloWorld"), s("world"), ci.clone()])), "Hello");
    assert_eq!(text(call("substring-after", &[s("HelloWorld"), s("HELLO"), ci.clone()])), "World");
    assert_eq!(truth("ends-with", &[s("abC"), s("c"), ci]), "true");
    assert_eq!(code(call("contains", &[s("a"), s("a"), s("UNICODE")])), "FOCH0004");
    assert_eq!(code(call("starts-with", &[s("a"), s("a"), s("CollationA")])), "FOCH0002");
}
