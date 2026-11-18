//! Tests of the collations the library compares strings by.

use std::cmp::Ordering;

use super::super::testing::focus;
use super::*;

/// The error code of `result`.
fn code<T: std::fmt::Debug>(result: Result<T>) -> String {
    let message = result.unwrap_err().to_string();
    let (_, after) = message.split_once("err:").unwrap();
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.3.1]
/// the code point collation and turborel's by their names; the default
/// the focus's, a collation named the one given; another err:FOCH0002.
#[test]
fn collations_are_named() {
    assert!(matches!(Collator::named(CODEPOINT_COLLATION).unwrap(), Collator::Codepoint));
    assert!(matches!(Collator::named("UNICODE").unwrap(), Collator::Named(_)));
    assert_eq!(format!("{:?}", Collator::named("CASE_INSENSITIVE").unwrap()), "CASE_INSENSITIVE");
    assert_eq!(format!("{:?}", Collator::of(&focus(), None).unwrap()), CODEPOINT_COLLATION);
    assert_eq!(code(Collator::named("http://www.example.com/collations/French")), "FOCH0002");
    let unknown = Focus { collation: "nowhere".to_owned(), ..focus() };
    assert_eq!(code(Collator::of(&unknown, None)), "FOCH0002");
    assert!(Collator::of(&unknown, Some(CODEPOINT_COLLATION)).is_ok());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.3.2]
/// strings ordered by code point, or by a collation's sort keys with no
/// padding.
#[test]
fn strings_are_ordered_by_collation() {
    let codepoint = Collator::Codepoint;
    assert_eq!(codepoint.compare("B", "a"), Ordering::Less);
    assert_eq!(codepoint.compare("a", "a "), Ordering::Less);
    assert_eq!(codepoint.compare("\u{FFFF}", "😀"), Ordering::Less);
    let unicode = Collator::named("UNICODE").unwrap();
    assert_eq!(unicode.compare("B", "a"), Ordering::Greater);
    assert!(Collator::named("CASE_INSENSITIVE").unwrap().equal("Straße", "STRAßE"));
    assert!(!Collator::named("UCS_BASIC").unwrap().equal("a", "a "));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5] the
/// collation units of a string: its characters, folded by a collation of
/// one character at a time; a collation of no units err:FOCH0004.
#[test]
fn collation_units() {
    assert_eq!(Collator::Codepoint.units("aB").unwrap(), vec!['a', 'B']);
    assert_eq!(Collator::named("CASE_INSENSITIVE").unwrap().units("aB").unwrap(), vec!['a', 'b']);
    assert_eq!(code(Collator::named("UNICODE").unwrap().units("a")), "FOCH0004");
}
