//! Tests of the collations a query compares strings by.

use std::cmp::Ordering;

use super::{CODEPOINT, Collator, SQL_COLLATION};
use crate::xquery::testing::collations;

/// Verifies: [EXTENSION] [XQueryFO §7.3.1] RULES.md D60: the codepoint
/// collation compares by code points; a database's collation is named by
/// `SQL_COLLATION` and its name, and compares by its sort keys; another
/// URI names none.
#[test]
fn collations_by_uri() {
    let all = collations();
    let codepoint = Collator::named(CODEPOINT, &all).unwrap();
    assert_eq!(codepoint.compare("B", "a"), Ordering::Less);
    assert_eq!(codepoint.uri(), CODEPOINT);
    let unicode = Collator::named(&format!("{SQL_COLLATION}UNICODE"), &all).unwrap();
    assert_eq!(unicode.compare("B", "a"), Ordering::Greater);
    assert!(Collator::named("urn:none", &all).is_none());
    assert!(Collator::named(&format!("{SQL_COLLATION}NO_SUCH"), &all).is_none());
    assert!(format!("{codepoint:?}").contains(CODEPOINT));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.3.2,
/// §7.5] RULES.md D60: strings equal by a collation's sort keys of no
/// padding; the collation units of a string its characters, folded by a
/// collation of one character at a time; a collation of no units, as
/// `UNICODE` is, err:FOCH0004.
#[test]
fn equality_and_collation_units() {
    let all = collations();
    let named = |name: &str| Collator::named(&format!("{SQL_COLLATION}{name}"), &all).unwrap();
    assert!(named("CASE_INSENSITIVE").equal("Straße", "STRAßE"));
    assert!(!named("UCS_BASIC").equal("a", "a "));
    assert!(!Collator::codepoint().equal("a", "A"));
    assert_eq!(Collator::codepoint().units("aB").unwrap(), vec!['a', 'B']);
    assert_eq!(named("CASE_INSENSITIVE").units("aB").unwrap(), vec!['a', 'b']);
    assert!(named("UNICODE").units("a").unwrap_err().to_string().contains("err:FOCH0004"));
}
