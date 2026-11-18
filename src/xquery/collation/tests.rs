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
