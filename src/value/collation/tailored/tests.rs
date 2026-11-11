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

/// A tailoring of `rules`.
fn made(rules: &'static str) -> Tailored {
    Tailored { name: "T", version: String::new(), rules, unicode: Unicode::new(table()), made: OnceLock::new() }
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
    assert_eq!(all.len(), 123);
    for t in &all {
        assert!(t.name.starts_with(super::super::TAILORED), "{}", t.name);
        assert!(t.version.starts_with("UCA 13.0.0 CLDR 38 "), "{}", t.version);
        let built = t.build().unwrap_or_else(|e| panic!("{}: {e}", t.name));
        assert!(!built.map.is_empty() || built.backwards || built.base.reorder.is_some(), "{} tailors nothing", t.name);
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
    ascending(&tailoring("UNICODE_SV_STANDARD"), &["va", "wa", "vb"]);
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
        ascending(&made(rules), &order);
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
    // 0x8000_0001 in the lower 32 bits; common weights; not variable.
    let expected = [0x22, 0xFD, 0x01, 0x80, 0x00, 0x00, 0x01, 0, 0, 0x00, 0x20, 0x00, 0, 0, 0x00, 0x02, 0x00, 0, 0, 0xFF, 0xFF, 0x00];
    assert_eq!(key(&sv, "å"), expected);
}

/// Verifies: [EXTENSION] RULES.md D60 [UTS #35 Part 5 §3.13]: a language
/// whose rules reorder scripts puts its own first, the special groups not
/// named staying before it (Greek, Russian); digits may go after letters
/// (Czech); within a group the order stays.
#[test]
fn scripts_reorder_as_the_language_has_them() {
    let unicode = Unicode::new(table());
    ascending(&unicode, &["a", "ω", "я", "一"]);
    ascending(&tailoring("UNICODE_EL"), &[" ", "!", "0", "α", "ω", "a", "я"]);
    ascending(&tailoring("UNICODE_RU"), &["0", "а", "я", "a"]);
    ascending(&tailoring("UNICODE_CS_DIGITS_AFTER"), &["$", "a", "č", "ω", "0", "9"]);
    ascending(&tailoring("UNICODE_CS"), &["0", "a", "č"]);
}

/// Verifies: [EXTENSION] RULES.md D60: Chinese orders ideographs by pinyin,
/// by strokes, or by radical and strokes, each before Latin, where
/// UNICODE orders them by code point after it.
#[test]
fn ideographs_sort_by_reading_or_strokes() {
    let unicode = Unicode::new(table());
    ascending(&unicode, &["a", "三", "中", "二", "阿", "\u{3400}"]);
    ascending(&tailoring("UNICODE_ZH"), &["阿", "二", "三", "中", "a"]);
    ascending(&tailoring("UNICODE_ZH_STROKE"), &["二", "三", "中", "阿", "a"]);
    ascending(&tailoring("UNICODE_ZH_HANT"), &["二", "三", "中"]);
    // 㐀 is of radical 1, as 三 is; 中 of radical 2.
    ascending(&tailoring("UNICODE_ZH_UNIHAN"), &["三", "\u{3400}", "中", "二", "a"]);
}

/// Verifies: [EXTENSION] RULES.md D60: Japanese compares three levels, so
/// hiragana and katakana, and punctuation, make no difference; a long
/// vowel mark after a kana weighs as that kana's vowel [UTS #35 Part 5
/// §3.9]; and a variable character no longer weighs. MC/DC of weighing,
/// a first-level weight and either the fourth level compared or not
/// variable: Swedish `a` against an acute (first level), Swedish `-`
/// against Japanese (fourth level), Japanese `-` against `a` (variable).
#[test]
fn japanese_compares_three_levels_and_prefixes() {
    let (ja, unicode) = (tailoring("UNICODE_JA"), Unicode::new(table()));
    assert_eq!(key(&ja, "あ"), key(&ja, "ア"));
    assert_ne!(key(&unicode, "あ"), key(&unicode, "ア"));
    assert_eq!(key(&ja, "a-b"), key(&ja, "ab"));
    ascending(&ja, &["かー", "かぁ", "かあ", "かーい", "かい"]);
    ascending(&unicode, &["かーい", "かあ"]);
    assert!(!ja.weighs('-') && ja.weighs('a') && unicode.weighs('-'));
    let sv = tailoring("UNICODE_SV");
    assert!(sv.weighs('a') && !sv.weighs('\u{301}') && sv.weighs('-'));
}

/// Verifies: [EXTENSION] RULES.md D60: Serbian suppresses UNICODE's
/// contraction of и and a breve, so й weighs as и with an accent; emoji
/// go before the currency symbols, no longer variable, a skin tone at the
/// second level; and the POSIX locale orders ASCII by code.
#[test]
fn contractions_are_suppressed_and_groups_reset() {
    let unicode = Unicode::new(table());
    ascending(&unicode, &["иб", "йа"]);
    ascending(&tailoring("UNICODE_SR"), &["йа", "иб"]);
    let emoji = tailoring("UNICODE_EMOJI");
    ascending(&emoji, &["😀", "😃", "😄", "👍", "👍🏻", "👍🏿", "👎", "¤", "$"]);
    ascending(&emoji, &["😀b", "😃a"]);
    ascending(&unicode, &["😃a", "😀b"]);
    let posix = tailoring("UNICODE_EN_US_POSIX");
    ascending(&posix, &[" ", "!", "/", "0", "9", "@", "A", "B", "Z", "[", "`", "a", "z", "{", "~"]);
}

/// Verifies: [EXTENSION] RULES.md D60 [UTS #35 Part 5 §3.11]: a reset to
/// `[last tertiary ignorable]` makes what follows ignorable, or weigh at
/// the third level alone (Urdu); one to `[last regular]` puts
/// what follows in the group of the ideographs, before them; one to
/// U+FDD1 and a character puts what follows before that character's
/// group.
#[test]
fn resets_name_positions_and_groups() {
    let ignored = made("&[last tertiary ignorable]=x");
    assert_eq!(key(&ignored, "axb"), key(&ignored, "ab"));
    let urdu = tailoring("UNICODE_UR");
    assert!(key(&urdu, "\u{0610}").starts_with(&[0, 0, 0, 0, 0]));
    ascending(&urdu, &["ب", "ب\u{0610}", "بب"]);
    ascending(&made("[reorder Hani]&[last regular]<x"), &["0", "x", "一", "a"]);
    ascending(&made("&[before 1]\u{FDD1}€<x"), &["!", "x", "¤", "$", "0"]);
    assert_ne!(key(&made("&[before 1]\u{FDD1}€<x"), "x").get(..2), Some([0, 0].as_slice()));
}

/// Verifies: [EXTENSION] RULES.md D60 [UTS #35 Part 5 §3]: a relation
/// after a reset of more than one element follows the last that differs
/// at its level; a prefix tailors a string only after it; a list puts its
/// characters in turn; `<<<<` is equal under `[strength 3]` and refused
/// otherwise; an import too deep is refused. MC/DC of dropping the last
/// element, more than one left and it ignorable at the relation's level:
/// `a\u{301}` against `[last tertiary ignorable]` alone, which Urdu
/// tailors (more than one), and against `ab` (ignorable). What follows a
/// weight of zero weighs above it in a key, not as the zero bytes between
/// levels, which it once did; Urdu's marks then sorted before nothing.
/// MC/DC of that, a weight of zero and not before it: `<<<x` after
/// `[last tertiary ignorable]` against after `ab` (zero), and against
/// `[before 3]` (not before), which is refused.
#[test]
fn relations_follow_the_reset_as_cldr_has_them() {
    ascending(&made("&a\u{301}<x"), &["a", "á", "x", "b"]);
    ascending(&made("&ab<x"), &["ab", "abz", "x", "ac"]);
    ascending(&made("&[last tertiary ignorable]<<<x"), &["a", "ax", "b"]);
    assert!(made("&[before 3][last tertiary ignorable]<<<x").build().is_err());
    assert!(resolved("[import root private-unihan]", MOST_IMPORTS).is_err());
    assert!(resolved("[import root private-unihan]", MOST_IMPORTS.saturating_sub(1)).is_ok());
    let prefixed = made("&a<<<c|b");
    ascending(&prefixed, &["ca", "cb", "cc", "xa", "xb"]);
    ascending(&prefixed, &["a", "b"]);
    ascending(&made("&a<*dcb"), &["a", "d", "c", "b"]);
    let quaternary = made("[strength 3]&a<<<<b");
    assert_eq!(key(&quaternary, "a"), key(&quaternary, "b"));
    assert!(made("&a<<<<b").build().is_err());
    assert!(made("[import xx standard]").build().is_err());
    assert!(made("[reorder Xxxx]").build().is_err());
}

/// Verifies: [EXTENSION] RULES.md D60 [UTS #35 Part 5 §3.11]: `[last
/// primary ignorable]` is UNICODE's greatest element ignorable at the
/// first level, the second of Runic z's, and `[last secondary ignorable]`
/// is ignorable at every level, as UNICODE has no element ignorable at the
/// first two alone. MC/DC of a candidate replacing the greatest, ignorable
/// and greater: Runic z's first element against its second (ignorable),
/// and an acute's against Runic z's second (greater).
#[test]
fn the_last_ignorables_are_unicodes_greatest() {
    assert_eq!(Unicode::new(table()).last_ignorables(), [[0, 0x011C, 0x0004], [0, 0, 0]]);
}
