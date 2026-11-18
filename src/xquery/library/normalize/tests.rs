//! Tests of normalization and case mapping.

use super::super::testing::{call, code, empty, s, text};
use super::*;

/// `text` in each of the four forms, NFC, NFD, NFKC and NFKD.
fn forms(text: &str) -> [String; 4] {
    [Form::Nfc, Form::Nfd, Form::Nfkc, Form::Nfkd].map(|form| normalized(text, form).unwrap())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.1,
/// §7.4.6] \[UAX #15 §1.3\] the example of [XQueryFO §7.4.1]: a combining
/// diaeresis after its base composed by NFC, kept apart by NFD; marks put
/// in canonical order; compatibility mappings only by NFKC and NFKD.
#[test]
fn normalization_forms() {
    let munchen = "I plan to go to Mu\u{308}nchen in September";
    assert_eq!(text(call("normalize-unicode", &[s(munchen)])), "I plan to go to München in September");
    assert_eq!(forms("\u{1E0A}\u{323}"), ["\u{1E0C}\u{307}", "D\u{323}\u{307}", "\u{1E0C}\u{307}", "D\u{323}\u{307}"].map(String::from));
    assert_eq!(forms("\u{FB01}"), ["\u{FB01}", "\u{FB01}", "fi", "fi"].map(String::from));
    assert_eq!(forms("\u{2460}"), ["\u{2460}", "\u{2460}", "1", "1"].map(String::from));
    assert_eq!(forms("\u{1E9B}\u{323}"), ["\u{1E9B}\u{323}", "\u{17F}\u{323}\u{307}", "\u{1E69}", "s\u{323}\u{307}"].map(String::from));
    assert_eq!(forms("a\u{301}\u{316}"), ["\u{E1}\u{316}", "a\u{316}\u{301}", "\u{E1}\u{316}", "a\u{316}\u{301}"].map(String::from));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] \[UAX #15 §1.3,
/// §14.1.4\] a character excluded from composition, and a singleton, are
/// not composed again; a mark blocked by another of its class is not
/// composed; Hangul syllables decompose to their jamo and compose back,
/// a syllable of no trailing consonant with one.
#[test]
fn exclusions_blocking_and_hangul() {
    assert_eq!(normalized("\u{958}", Form::Nfc).unwrap(), "\u{915}\u{93C}");
    assert_eq!(normalized("\u{212B}", Form::Nfc).unwrap(), "\u{C5}");
    assert_eq!(normalized("a\u{301}\u{301}", Form::Nfc).unwrap(), "\u{E1}\u{301}");
    assert_eq!(normalized("a\u{363}\u{301}", Form::Nfc).unwrap(), "a\u{363}\u{301}");
    assert_eq!(normalized("a\u{5B0}\u{301}", Form::Nfc).unwrap(), "\u{E1}\u{5B0}");
    assert_eq!(normalized("\u{AC01}", Form::Nfd).unwrap(), "\u{1100}\u{1161}\u{11A8}");
    assert_eq!(normalized("\u{AC00}", Form::Nfd).unwrap(), "\u{1100}\u{1161}");
    assert_eq!(normalized("\u{1100}\u{1161}\u{11A8}", Form::Nfc).unwrap(), "\u{AC01}");
    assert_eq!(normalized("\u{AC00}\u{11A8}", Form::Nfc).unwrap(), "\u{AC01}");
    assert_eq!(normalized("\u{326E}", Form::Nfkd).unwrap(), "\u{1100}\u{1161}");
    assert_eq!(normalized("\u{D7A3}", Form::Nfc).unwrap(), "\u{D7A3}");
    assert_eq!(normalized("", Form::Nfc).unwrap(), "");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.6]
/// the form named, blanks at either end removed and in upper case; no
/// normalization of the zero-length string; the empty sequence the
/// zero-length string; other forms err:FOCH0003.
#[test]
fn forms_by_name() {
    assert_eq!(text(call("normalize-unicode", &[s("e\u{301}"), s(" nfd ")])), "e\u{301}");
    assert_eq!(text(call("normalize-unicode", &[s("e\u{301}"), s(" fully-normalized ")])), "\u{E9}");
    assert_eq!(text(call("normalize-unicode", &[s("e\u{301}"), s("NFKC")])), "\u{E9}");
    assert_eq!(text(call("normalize-unicode", &[s("\u{FB01}"), s("nfkd")])), "fi");
    assert_eq!(text(call("normalize-unicode", &[s("e\u{301}"), s("")])), "e\u{301}");
    assert_eq!(text(call("normalize-unicode", &[empty()])), "");
    assert_eq!(code(call("normalize-unicode", &[s("a"), s("NFX")])), "FOCH0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.6]
/// [XML 1.1 Appendix B] FULLY-NORMALIZED gives a string's NFC, a string
/// its one relevant construct; one whose NFC begins with a composing
/// character has no fully normalized form, err:FOCH0003. MC/DC of
/// `composing`, each condition alone making a character composing: a
/// class not zero (U+0301), a vowel jamo (U+1161, to U+1175, not U+1176),
/// a trailing consonant jamo (U+11A8, to U+11C2, not U+11A7 nor U+11C3),
/// and second in a composite's pair, of class zero (U+0CD5); none of them
/// (`a`). A string beginning with a character excluded from composition
/// whose decomposition begins with a mark (U+0F73) is not fully
/// normalized, its NFC beginning with the mark; the empty string is.
#[test]
fn fully_normalized_form() {
    let table = Table::read().unwrap();
    let composes = |c: char| composing(&table, c);
    assert!(composes('\u{301}') && composes('\u{1161}') && composes('\u{1175}') && composes('\u{11A8}'));
    assert!(composes('\u{11C2}') && composes('\u{CD5}'));
    assert!(!composes('a') && !composes('\u{1176}') && !composes('\u{11A7}') && !composes('\u{11C3}'));
    let full = |t: &str| call("normalize-unicode", &[s(t), s("FULLY-NORMALIZED")]);
    assert_eq!(text(full("A\u{30A}b")), "\u{C5}b");
    assert_eq!(text(full("\u{1100}\u{1161}")), "\u{AC00}");
    assert_eq!(text(full("")), "");
    assert_eq!(text(call("normalize-unicode", &[empty(), s("FULLY-NORMALIZED")])), "");
    for begins in ["\u{301}a", "\u{1161}", "\u{11A8}", "\u{CD5}", "\u{F73}"] {
        assert_eq!(code(full(begins)), "FOCH0003", "{begins:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §7.4.7.1,
/// §7.4.8.1] the examples of `fn:upper-case` and `fn:lower-case`; a full
/// mapping that lengthens a string; a conditional one not applied.
#[test]
fn case_mappings() {
    assert_eq!(text(call("upper-case", &[s("abCd0")])), "ABCD0");
    assert_eq!(text(call("lower-case", &[s("ABc!D")])), "abc!d");
    assert_eq!(text(call("upper-case", &[s("straße")])), "STRASSE");
    assert_eq!(text(call("lower-case", &[s("\u{130}")])), "i\u{307}");
    assert_eq!(text(call("lower-case", &[s("ΑΣ")])), "ασ");
    assert_eq!(text(call("upper-case", &[s("ı")])), "I");
    assert_eq!(text(call("lower-case", &[empty()])), "");
}
