//! Tests of the functions based on substring matching.

use super::super::testing::{call, code, empty, s, text};
use crate::xml::Sequence;

/// The boolean `fn:local` gives of `args`, as text.
fn truth(local: &str, args: &[Sequence]) -> String {
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
    let ci = s("urn:turborel:collation:CASE_INSENSITIVE");
    assert_eq!(truth("contains", &[s("Tattoo"), s("TTO"), ci.clone()]), "true");
    assert_eq!(text(call("substring-before", &[s("HelloWorld"), s("world"), ci.clone()])), "Hello");
    assert_eq!(text(call("substring-after", &[s("HelloWorld"), s("HELLO"), ci.clone()])), "World");
    assert_eq!(truth("ends-with", &[s("abC"), s("c"), ci]), "true");
    assert_eq!(code(call("contains", &[s("a"), s("a"), s("urn:turborel:collation:UNSPLIT")])), "FOCH0004");
    assert_eq!(code(call("starts-with", &[s("a"), s("a"), s("CollationA")])), "FOCH0002");
}

/// `fn:local` of `a` and `b` by CollationA, as text.
fn by_a(local: &str, a: Sequence, b: &str) -> String {
    text(call(local, &[a, s(b), s("urn:turborel:collation:COLLATION_A")]))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5.1.1,
/// §7.5.2.1, §7.5.3.1] the examples of `fn:contains`, `fn:starts-with` and
/// `fn:ends-with` by CollationA, whose `-` and `*` are ignorable: a minimal
/// match skips ignorable units, within and around it; a string of
/// ignorable units alone is the zero-length string.
#[test]
fn ignorable_units_in_contains_starts_and_ends() {
    let cases = [
        ("contains", "abcdefghi", "-d-e-f-", "true"),
        ("contains", "a*b*c*d*e*f*g*h*i*", "d-ef-", "true"),
        ("contains", "abcd***e---f*--*ghi", "def", "true"),
        ("contains", "abc", "bd", "false"),
        ("starts-with", "abcdefghi", "-a-b-c-", "true"),
        ("starts-with", "a*b*c*d*e*f*g*h*i*", "a-bc-", "true"),
        ("starts-with", "abcd***e---f*--*ghi", "abcdef", "true"),
        ("starts-with", "--abc", "ab", "true"),
        ("starts-with", "abc", "bc", "false"),
        ("starts-with", "---", "a", "false"),
        ("ends-with", "abcdefghi", "-g-h-i-", "true"),
        ("ends-with", "abcd***e---f*--*ghi", "defghi", "true"),
        ("ends-with", "abc--", "bc", "true"),
        ("ends-with", "abcab", "ab", "true"),
        ("ends-with", "abc", "ab", "false"),
    ];
    for (local, a, b, expected) in cases {
        assert_eq!(by_a(local, s(a), b), expected, "{local}({a:?}, {b:?})");
    }
    for local in ["contains", "starts-with", "ends-with"] {
        assert_eq!(by_a(local, empty(), "--***-*---"), "true", "{local}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5.4.1,
/// §7.5.5.1] the examples of `fn:substring-before` and
/// `fn:substring-after` by CollationA: the characters before and after
/// the first minimal match, the ignorable units around it outside it; a
/// string of ignorable units alone the zero-length string.
#[test]
fn ignorable_units_before_and_after() {
    assert_eq!(by_a("substring-before", s("abcdefghi"), "--d-e-"), "abc");
    assert_eq!(by_a("substring-before", s("abc--d-e-fghi"), "--d-e-"), "abc--");
    assert_eq!(by_a("substring-before", s("a*b*c*d*e*f*g*h*i*"), "***cde"), "a*b*");
    assert_eq!(by_a("substring-before", s("Eureka!"), "--***-*---"), "");
    assert_eq!(by_a("substring-after", s("abcdefghi"), "--d-e-"), "fghi");
    assert_eq!(by_a("substring-after", s("abc--d-e-fghi"), "--d-e-"), "-fghi");
    assert_eq!(by_a("substring-after", s("a*b*c*d*e*f*g*h*i*"), "***cde***"), "*f*g*h*i*");
    assert_eq!(by_a("substring-after", s("Eureka!"), "--***-*---"), "Eureka!");
    assert_eq!(by_a("substring-after", s("--*"), ""), "");
    assert_eq!(by_a("substring-after", s("abc"), "x"), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5]
/// RULES.md D106: by `UNICODE`, units of more than one character: a
/// precomposed character matches its decomposition, a character's marks
/// are never split from it, a completely ignorable character is left out
/// of a minimal match; by the Czech tailoring the contraction `ch` is one
/// unit, so `c` is not found in it, but is before another letter.
#[test]
fn units_of_more_than_one_character() {
    let unicode = s("urn:turborel:collation:UNICODE");
    let by = |local: &str, a: &str, b: &str, c: &Sequence| text(call(local, &[s(a), s(b), c.clone()]));
    assert_eq!(by("contains", "re\u{301}sume\u{301}", "\u{e9}s", &unicode), "true");
    assert_eq!(by("contains", "e\u{301}", "e", &unicode), "false");
    assert_eq!(by("starts-with", "\u{e9}t\u{e9}", "e\u{301}", &unicode), "true");
    assert_eq!(by("ends-with", "caf\u{e9}", "e\u{301}", &unicode), "true");
    assert_eq!(by("substring-before", "ae\u{301}b", "\u{e9}", &unicode), "a");
    assert_eq!(by("substring-after", "ae\u{301}b", "\u{e9}", &unicode), "b");
    assert_eq!(by("substring-after", "xa\u{0}b", "a", &unicode), "\u{0}b");
    assert_eq!(by("substring-before", "x\u{0}ab", "ab", &unicode), "x\u{0}");
    assert_eq!(by("contains", "Pen", "pen", &unicode), "false");
    let czech = s("urn:turborel:collation:UNICODE_CS");
    assert_eq!(by("contains", "chata", "c", &czech), "false");
    assert_eq!(by("contains", "chata", "ch", &czech), "true");
    assert_eq!(by("contains", "abc", "c", &czech), "true");
    assert_eq!(by("substring-after", "ochota", "ch", &czech), "ota");
}
