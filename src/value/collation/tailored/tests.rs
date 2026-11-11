// Unit tests: the collations UNICODE_<LOCALE> order each language as its
// dictionaries do (RULES.md D60).

use std::sync::Arc;

use super::*;
use crate::value::collation::UNICODE;

fn table() -> Arc<Table> {
    Arc::new(Table::read().unwrap())
}

fn tailoring(name: &str) -> Tailored {
    all(&table()).into_iter().find(|t| t.name == name).unwrap()
}

fn key(c: &dyn Collation, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    c.sort_key(text, Pad::None, &mut out);
    out
}

/// That `words` are in strictly ascending order under `c`.
fn ascending(c: &dyn Collation, words: &[&str]) {
    for pair in words.windows(2) {
        assert!(key(c, pair[0]) < key(c, pair[1]), "{}: {:?} before {:?}", c.name(), pair[0], pair[1]);
    }
}

/// Verifies: [EXTENSION] RULES.md D60: every tailoring's rules are read
/// and made, none refused, each of its own name and of a version naming
/// its CLDR release and rules.
#[test]
fn every_tailoring_is_made() {
    let all = all(&table());
    assert_eq!(all.len(), 48);
    for t in &all {
        assert!(t.name.starts_with(super::super::TAILORED), "{}", t.name);
        assert!(t.version.starts_with("UCA 13.0.0 CLDR 38 "), "{}", t.version);
        let built = t.build().unwrap_or_else(|e| panic!("{}: {e}", t.name));
        assert!(!built.map.is_empty() || built.backwards, "{} tailors nothing", t.name);
    }
}

/// Verifies: [EXTENSION] RULES.md D60: a language's letters sort as its
/// dictionaries sort them, where UNICODE sorts them otherwise: Swedish å, ä
/// and ö after z, Spanish ñ after n, Czech ch after h, Turkish dotless ı
/// before i, Vietnamese ă and â as letters of their own.
#[test]
fn letters_sort_as_the_language_has_them() {
    ascending(&tailoring("UNICODE_SV"), &["z", "zz", "å", "ä", "ö"]);
    ascending(&tailoring("UNICODE_ES"), &["n", "nz", "ñ", "o"]);
    ascending(&tailoring("UNICODE_CS"), &["h", "hz", "ch", "i"]);
    ascending(&tailoring("UNICODE_TR"), &["ı", "I", "i", "İ", "j"]);
    ascending(&tailoring("UNICODE_VI"), &["az", "ă", "ăz", "â", "b"]);
    ascending(&tailoring("UNICODE_PL"), &["lz", "ł", "m", "z", "ź", "ż"]);
    let unicode = Unicode::new(table());
    ascending(&unicode, &["ä", "ñ", "o", "z"]);
}

/// Verifies: [EXTENSION] RULES.md D60: a letter of two characters is one
/// (Spanish traditional ch and ll), a letter may sort as two at the first
/// level (German phonebook ä as ae), and a level-two relation leaves a
/// letter equal at the first (Swedish w as v).
#[test]
fn contractions_and_expansions_are_tailored() {
    ascending(&tailoring("UNICODE_ES_TRADITIONAL"), &["cz", "ch", "d", "lz", "ll", "m"]);
    ascending(&tailoring("UNICODE_DE_PHONEBOOK"), &["ad", "ae", "ä", "af"]);
    ascending(&tailoring("UNICODE_SV"), &["va", "wa", "vb"]);
    // Hungarian ccs is cs twice, at the third level after it.
    ascending(&tailoring("UNICODE_HU"), &["cz", "cs", "cscs", "ccs", "csd"]);
    let unicode = Unicode::new(table());
    ascending(&unicode, &["ä", "ad", "ccs", "cscs"]);
}

/// Verifies: [EXTENSION] RULES.md D60: `[caseFirst upper]` puts upper case
/// first (Danish, aa after å), and `[backwards 2]` compares accents from
/// the end (Canadian French).
#[test]
fn options_change_the_third_and_second_levels() {
    ascending(&tailoring("UNICODE_DA"), &["A", "a", "z", "ø", "Å", "å", "aa"]);
    ascending(&tailoring("UNICODE_FR_CA"), &["cote", "côte", "coté", "côté"]);
    let unicode = Unicode::new(table());
    ascending(&unicode, &["a", "A", "cote", "coté", "côte", "côté"]);
}

/// Verifies: [EXTENSION] RULES.md D60: a string a tailoring does not name
/// orders as UNICODE orders it, canonically equivalent strings stay equal,
/// and a mark after a tailored letter is found though another comes
/// between [UTS #10 S2.1.1].
#[test]
fn what_is_not_tailored_orders_as_unicode() {
    let (sv, unicode) = (tailoring("UNICODE_SV"), Unicode::new(table()));
    let words = ["", " a", "a-b", "ab", "Apple", "apple", "banana", "resume", "résumé", "日本", "\u{1F600}", "zebra"];
    for a in words {
        for b in words {
            assert_eq!(key(&sv, a).cmp(&key(&sv, b)), key(&unicode, a).cmp(&key(&unicode, b)), "{a:?} {b:?}");
        }
    }
    assert_eq!(key(&sv, "å"), key(&sv, "a\u{30A}"));
    // A dot below, of a lower class, comes between a and its ring.
    let parted = key(&sv, "a\u{323}\u{30A}");
    assert!(key(&sv, "z") < parted && parted < key(&sv, "ä"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D60: under
/// PAD SPACE trailing spaces are taken away first; under NO PAD they count.
#[test]
fn a_tailorings_pad_is_kept() {
    let sv = tailoring("UNICODE_SV");
    let padded = |text: &str| {
        let mut out = Vec::new();
        sv.sort_key(text, Pad::Space, &mut out);
        out
    };
    assert_eq!(padded("å  "), padded("å"));
    assert_ne!(key(&sv, "å  "), key(&sv, "å"));
    assert_eq!(super::super::default_pad("UNICODE_SV"), Pad::None);
    assert_eq!(super::super::default_pad(UNICODE), Pad::None);
}

/// Verifies: [EXTENSION] RULES.md D60 [UTS #35 Part 5 §3]: a relation puts
/// its string right after its reset, before what an earlier rule put
/// there, at the first level and the third alike.
#[test]
fn a_later_relation_goes_nearer_its_reset() {
    for (rules, order) in
        [("&a<c&a<b", ["a", "b", "c"]), ("&a<<<c&a<<<b", ["a", "b", "c"]), ("&[before 1]b<y&[before 1]b<x", ["y", "x", "b"])]
    {
        let t = Tailored { name: "T", version: String::new(), rules, unicode: Unicode::new(table()), made: OnceLock::new() };
        ascending(&t, &order);
    }
}

/// Verifies: [DESIGN] RULES.md D3, D60: the rules are those `cargo xtask
/// cldr` makes from CLDR 38, and a tailored key is the one it has always
/// been, so a change to either, which would reorder an index, changes this
/// test and so `Tailored::version` with it.
#[test]
fn the_rules_and_keys_are_fixed() {
    assert_eq!(super::super::table::fnv(RULES.as_bytes()), RULES_DIGEST);
    let sv = tailoring("UNICODE_SV");
    // å is put just before ǀ, 0x22FE, at the first level: 0x22FD, then
    // 0x8001 in the lower 16 bits; common weights; not variable.
    let expected = [0x22, 0xFD, 0x01, 0x80, 0x01, 0, 0, 0x00, 0x20, 0x00, 0, 0, 0x00, 0x02, 0x00, 0, 0, 0xFF, 0xFF, 0x01, 0xFF, 0xFF];
    assert_eq!(key(&sv, "å"), expected);
}
