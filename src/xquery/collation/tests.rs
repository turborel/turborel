//! Tests of the collations a query compares strings by.

use std::cmp::Ordering;

use super::{CODEPOINT, Collator, SQL_COLLATION};
use crate::xquery::testing::{UNSPLIT, collations};

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

/// Units as a test reads them: each one's characters and its weights at
/// each level.
type Split = Vec<((usize, usize), Vec<Vec<u8>>)>;

/// The units of `text` by `collator`.
fn split(collator: &Collator, text: &str) -> Split {
    let units = collator.units(text).unwrap();
    (0..units.len()).map(|u| (units.characters(u), (0..units.levels()).map(|l| units.weights(l, u).to_vec()).collect())).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.3.2,
/// §7.5] RULES.md D60: strings equal by a collation's sort keys of no
/// padding; the collation units of a string its characters, weighed by
/// their code points or folded by a collation of one character at a time;
/// a collation that does not split strings into units err:FOCH0004.
#[test]
fn equality_and_collation_units() {
    let all = collations();
    let named = |name: &str| Collator::named(&format!("{SQL_COLLATION}{name}"), &all).unwrap();
    assert!(named("CASE_INSENSITIVE").equal("Straße", "STRAßE"));
    assert!(!named("UCS_BASIC").equal("a", "a "));
    assert!(!Collator::codepoint().equal("a", "A"));
    let one = |at: usize, weight: &str| ((at, at + 1), vec![weight.as_bytes().to_vec()]);
    assert_eq!(split(&Collator::codepoint(), "aB"), vec![one(0, "a"), one(1, "B")]);
    assert_eq!(split(&named("CASE_INSENSITIVE"), "aB"), vec![one(0, "a"), one(1, "b")]);
    assert_eq!(split(&Collator::codepoint(), ""), Split::new());
    assert!(named(UNSPLIT).units("a").unwrap_err().to_string().contains("err:FOCH0004"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.5]
/// RULES.md D60, D106: `UNICODE` splits a string into units each a
/// character its steps begin at and the marks after it, of four levels;
/// a precomposed character and its decomposition one unit of the same
/// weights; a completely ignorable character a unit of no weights, but
/// at the string's start, where marks begin a unit too; a Hangul
/// syllable one unit; a contraction S2.1.1 makes past a mark one unit, the
/// next character beginning its own; a tailoring's contraction one unit,
/// its letters alone units elsewhere.
#[test]
fn units_of_unicode_and_tailorings() {
    let all = collations();
    let named = |name: &str| Collator::named(&format!("{SQL_COLLATION}{name}"), &all).unwrap();
    let unicode = named("UNICODE");
    let bounds = |c: &Collator, text: &str| split(c, text).into_iter().map(|(b, _)| b).collect::<Vec<_>>();
    assert_eq!(bounds(&unicode, "ae\u{301}b"), [(0, 1), (1, 3), (3, 4)]);
    assert_eq!(split(&unicode, "\u{e9}")[0].1, split(&unicode, "e\u{301}")[0].1);
    assert_eq!(split(&unicode, "e\u{301}")[0].1.len(), 4);
    assert_eq!(bounds(&unicode, "\u{301}a\u{0}b"), [(0, 1), (1, 2), (2, 3), (3, 4)]);
    assert!(split(&unicode, "a\u{0}b")[1].1.iter().all(Vec::is_empty));
    assert_eq!(bounds(&unicode, "\u{ac01}x"), [(0, 1), (1, 2)]);
    assert_eq!(bounds(&unicode, "\u{438}\u{334}\u{306}x"), [(0, 3), (3, 4)], "S2.1.1 takes the breve past a mark");
    let czech = named("UNICODE_CS");
    assert_eq!(bounds(&czech, "chata"), [(0, 2), (2, 3), (3, 4), (4, 5)]);
    assert_eq!(bounds(&czech, "abc"), [(0, 1), (1, 2), (2, 3)]);
}
