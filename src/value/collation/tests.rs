// Unit tests: the contract of `Collation`, for each built-in (RULES.md
// R-SAFE-3, D60).

use std::cmp::Ordering;
use std::sync::Arc;

use super::*;
use crate::value::compare_strings;

fn named(name: &str, pad: Pad) -> CollationRef {
    let collation = builtin().into_iter().find(|c| c.name() == name).unwrap();
    CollationRef { collation, pad }
}

fn key(c: &CollationRef, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    c.collation.sort_key(text, c.pad, &mut out);
    out
}

/// Every pairing of a small alphabet, up to `length`: spaces, a character
/// below a space, one above, and one of more bytes.
fn strings(length: usize) -> Vec<String> {
    let mut all = vec![String::new()];
    let mut last = all.clone();
    for _ in 0..length {
        last = last.iter().flat_map(|s| [' ', '\t', 'a', 'é'].map(|c| format!("{s}{c}"))).collect();
        all.extend(last.iter().cloned());
    }
    all
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D60: a PAD
/// SPACE sort key orders exactly as `compare_strings` does, over every
/// string of a small alphabet, spaces and characters below a space among
/// them, so `UCS_BASIC` as a key is today's order.
#[test]
fn a_pad_space_key_orders_as_comparison_does() {
    let c = named(UCS_BASIC, Pad::Space);
    let all = strings(4);
    for a in &all {
        for b in &all {
            assert_eq!(key(&c, a).cmp(&key(&c, b)), compare_strings(a, b), "{a:?} {b:?}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] under NO PAD a string
/// is less than itself with spaces after it.
#[test]
fn no_pad_counts_trailing_spaces() {
    let c = named(UCS_BASIC, Pad::None);
    assert_eq!(key(&c, "a").cmp(&key(&c, "a ")), Ordering::Less);
    assert_eq!(key(&c, "a"), b"a");
}

/// Verifies: [EXTENSION] RULES.md D60: `CASE_INSENSITIVE` folds simply, so
/// a string's cases compare equal, one to one, and its characters fold
/// alike; sharp s stays one character.
#[test]
fn case_insensitive_folds_simply() {
    let c = named(CASE_INSENSITIVE, Pad::Space);
    assert_eq!(key(&c, "Bob"), key(&c, "bOB"));
    assert_eq!(key(&c, "ÉCOLE"), key(&c, "école"));
    assert_ne!(key(&c, "ß"), key(&c, "ss"));
    assert_ne!(key(&c, "a"), key(&c, "b"));
    assert_eq!(c.collation.fold('Q'), Some('q'));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.6] UNICODE orders by
/// base letter before accent before case, ignores punctuation at the first
/// three levels, and gives canonical equivalents one key.
#[test]
fn unicode_orders_as_uca_does() {
    let c = named(UNICODE, Pad::None);
    let mut words = vec!["Zebra", "apple", "résumé", "resume", "Résumé", "résume", "z"];
    words.sort_by_key(|w| key(&c, w));
    assert_eq!(words, ["apple", "resume", "résume", "résumé", "Résumé", "z", "Zebra"]);
    assert_eq!(key(&c, "\u{e9}"), key(&c, "e\u{301}"), "NFC and NFD");
    assert_eq!(key(&c, "\u{1e69}"), key(&c, "s\u{323}\u{307}"), "marks reordered");
    assert_eq!(key(&c, "\u{ac00}"), key(&c, "\u{1100}\u{1161}"), "a Hangul syllable");
    assert!(key(&c, "\u{439}") < key(&c, "\u{438}\u{306}\u{334}"), "S2.1.1: the mark left after a contraction still counts");
    assert_eq!(key(&c, "de luge").cmp(&key(&c, "delta")), Ordering::Greater, "the space weighs last");
    assert_eq!(c.collation.fold('a'), None);
    assert!(key(&c, "\u{4e00}") < key(&c, "\u{3400}"), "unified before extension");
    assert!(key(&c, "a") < key(&c, "\u{10ffff}"), "unassigned after");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.43 GR5] the pad
/// characteristic is part of a collation; PAD SPACE UNICODE takes trailing
/// spaces away; behaving alike is by behaviour, not name.
#[test]
fn the_pad_characteristic_counts() {
    let pad = named(UNICODE, Pad::Space);
    assert_eq!(key(&pad, "a  "), key(&pad, "a"));
    assert_ne!(key(&named(UNICODE, Pad::None), "a  "), key(&named(UNICODE, Pad::None), "a"));
    assert!(named(SQL_TEXT, Pad::Space).behaves_as(&named(UCS_BASIC, Pad::Space)));
    assert!(!named(UCS_BASIC, Pad::None).behaves_as(&named(UCS_BASIC, Pad::Space)));
    assert!(!pad.behaves_as(&named(CASE_INSENSITIVE, Pad::Space)));
    assert!(named(SQL_TEXT, Pad::Space).is_default() && !named(UCS_BASIC, Pad::None).is_default());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.1] RULES.md D60: a sort
/// key is bits, a whole number of bytes; a null stays null; a value of
/// another type is left as it is.
#[test]
fn sort_keys_are_values() {
    let c = named(CASE_INSENSITIVE, Pad::None);
    assert!(matches!(c.sort_key(&Value::Char("ab".into())), Value::Bits(b) if b.bytes() == b"ab"));
    assert!(matches!(c.sort_key(&Value::Null), Value::Null));
    assert!(matches!(c.sort_key(&Value::Approx(1.0)), Value::Approx(_)));
    let _: Arc<dyn Collation> = Arc::clone(&c.collation);
}

/// Verifies: [DESIGN] RULES.md D3, D60: the table is the one the generator
/// makes from Unicode 13.0: every built-in reads it, and a change to its
/// bytes changes this digest, which fixes what `Collation::version` says.
#[test]
fn the_table_is_the_one_generated() {
    assert_eq!(builtin().len(), 4, "the table read");
    assert_eq!(table::digest(), table::DIGEST);
}
