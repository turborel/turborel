//! Unit tests of the languages (RULES.md D320 to D329): their names,
//! stemmers and stop words; normalization in each; elisions and the
//! Catalan middle dot; and the character bigrams of the scripts written
//! without spaces.

use super::tokens_in;
use crate::fulltext::text::{Language, Languages, is_single_letter, is_stop, normalize_in, stem, stop_list};

/// The words of `text` in `language`.
fn words_in(text: &str, language: Language) -> Vec<String> {
    tokens_in(text, language).into_iter().map(|t| t.word).collect()
}

/// Each word of `text` in `language`, its offset and length.
fn spans_in(text: &str, language: Language) -> Vec<(String, u32, u32)> {
    tokens_in(text, language).into_iter().map(|t| (t.word, t.offset, t.length)).collect()
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4, §5.1.9] the 43 languages are
/// named once each, in any case; each stems or reads bigrams but NONE; a
/// stemmer's program reads; and 17 have stop words.
#[test]
fn every_language_is_named_and_reads() {
    let all: Vec<Language> = Language::all().collect();
    assert_eq!(all.len(), 43);
    for language in &all {
        assert_eq!(Language::named(&language.name().to_ascii_lowercase()).ok(), Some(*language));
        assert_eq!(language.stems(), language.stemmer().is_some(), "{}", language.name());
        assert!(language.stems() != language.reads_bigrams() || *language == Language::None, "{}", language.name());
        assert_eq!(language.has_stop_words(), !stop_list(*language).is_empty(), "{}", language.name());
    }
    assert_eq!(all.iter().filter(|language| language.stems()).count(), 35);
    assert_eq!(all.iter().filter(|language| language.reads_bigrams()).count(), 7);
    assert_eq!(all.iter().filter(|language| language.has_stop_words()).count(), 17);
    assert!(Language::named("KLINGON").is_err());
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2] a language's stop words are
/// Snowball's list, normalized as its words are, matched in it alone.
#[test]
fn stop_words_are_each_languages() {
    for (language, word) in [
        (Language::French, "les"),
        (Language::German, "und"),
        (Language::Russian, "и"),
        (Language::Spanish, "él"),
        (Language::Finnish, "olla"),
        (Language::Ukrainian, "і"),
    ] {
        assert!(is_stop(language, word), "{word}");
        assert!(!is_stop(Language::English, word) || word == "les", "{word}");
    }
    assert!(!is_stop(Language::Arabic, "في"));
    assert!(!is_stop(Language::Chinese, "的"));
    let finnish = stop_list(Language::Finnish);
    assert!(finnish.windows(2).all(|w| w[0] < w[1]));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.5] NONE and ENGLISH take accents
/// off; a stemmed language keeps them, composed; Turkish folds `I` to
/// `ı` and `İ` to `i`; a language read as bigrams makes full-width and
/// half-width forms one.
#[test]
fn each_language_normalizes_its_words() {
    assert_eq!(normalize_in(Language::English, "Élève"), "eleve");
    assert_eq!(normalize_in(Language::French, "Élève"), "élève");
    assert_eq!(normalize_in(Language::French, "E\u{301}le\u{300}ve"), "élève");
    assert_eq!(normalize_in(Language::German, "STRASSE"), "strasse");
    assert_eq!(normalize_in(Language::Russian, "ЙОД"), "йод");
    assert_eq!(normalize_in(Language::Turkish, "IŞIK"), "ışık");
    assert_eq!(normalize_in(Language::Turkish, "İstanbul"), "istanbul");
    assert_eq!(normalize_in(Language::Turkish, "I\u{307}stanbul"), "istanbul");
    assert_eq!(normalize_in(Language::English, "IŞIK"), "isik");
    assert_eq!(normalize_in(Language::Japanese, "ＴＯＫＹＯ"), "tokyo");
    assert_eq!(normalize_in(Language::Japanese, "ｶﾀｶﾅ"), "カタカナ");
    assert_eq!(normalize_in(Language::French, "l\u{2019}homme"), "l'homme");
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] [EXTENSION] (RULES.md D322)
/// French, Italian, Catalan and Irish take an elided article or pronoun
/// off its word, its place moved past it; English keeps it; Catalan joins
/// the letters around a middle dot.
#[test]
fn elisions_are_taken_off() {
    assert_eq!(spans_in("L'homme d\u{2019}affaires", Language::French), [("homme".into(), 2, 5), ("affaires".into(), 10, 8)]);
    assert_eq!(words_in("aujourd'hui qu'il", Language::French), ["aujourd'hui", "il"]);
    assert_eq!(words_in("dell'arte l'amica", Language::Italian), ["arte", "amica"]);
    assert_eq!(words_in("l'àvia col·lecció", Language::Catalan), ["àvia", "col·lecció"]);
    assert_eq!(words_in("col·lecció", Language::Spanish), ["col", "lecció"]);
    assert_eq!(words_in("d'fhág", Language::Irish), ["fhág"]);
    assert_eq!(words_in("l'homme", Language::English), ["l'homme"]);
    assert_eq!(stem(Language::French, &words_in("l'école", Language::French)[0]), "écol");
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] [EXTENSION] (RULES.md D323) a
/// run of ideographs, kana, Hangul or Thai letters in a language read as
/// bigrams is its overlapping pairs, each at its first letter, a letter's
/// marks with it; one letter alone is a word of one; other words are cut
/// as elsewhere; another language reads each ideograph alone, and a Thai
/// run as one word, its marks taken off in ENGLISH.
#[test]
fn runs_are_read_as_bigrams() {
    assert_eq!(spans_in("東京都庁", Language::Chinese), [("東京".into(), 0, 2), ("京都".into(), 1, 2), ("都庁".into(), 2, 2)]);
    assert_eq!(words_in("東", Language::Chinese), ["東"]);
    assert_eq!(words_in("Tokyo東京2020", Language::Japanese), ["tokyo", "東京", "2020"]);
    assert_eq!(words_in("コーヒー", Language::Japanese), ["コー", "ーヒ", "ヒー"]);
    assert_eq!(words_in("한국어 사전", Language::Korean), ["한국", "국어", "사전"]);
    assert_eq!(spans_in("สวัสดี", Language::Thai), [("สวั".into(), 0, 3), ("วัส".into(), 1, 3), ("สดี".into(), 3, 3)]);
    assert_eq!(words_in("ສະບາຍດີ", Language::Lao).len(), 5);
    assert_eq!(words_in("東京都庁", Language::English), ["東", "京", "都", "庁"]);
    assert_eq!(words_in("สวัสดี", Language::English), ["สวสด"], "ENGLISH takes the marks off");
    let ordinals: Vec<u32> = tokens_in("東京。大阪", Language::Japanese).iter().map(|t| t.ordinal * 10 + t.sentence).collect();
    assert_eq!(ordinals, [11, 22]);
}

/// Verifies: [EXTENSION] (RULES.md D323) a single letter is one of a
/// script read as bigrams, its marks with it, in such a language alone.
#[test]
fn single_letters_are_known() {
    assert!(is_single_letter(Language::Chinese, "京"));
    assert!(is_single_letter(Language::Thai, "วั"));
    assert!(!is_single_letter(Language::Chinese, "京都"));
    assert!(!is_single_letter(Language::Chinese, "a"));
    assert!(!is_single_letter(Language::English, "京"));
}

/// Verifies: [EXTENSION] (RULES.md D324) a set of languages holds those
/// added, listed in the order of their names.
#[test]
fn language_sets_hold_their_languages() {
    let set = Languages::of(Language::French).with(Language::English).with(Language::Burmese);
    assert!(set.contains(Language::Burmese) && !set.contains(Language::German));
    assert_eq!(set.iter().collect::<Vec<_>>(), [Language::English, Language::French, Language::Burmese]);
    assert_eq!(Languages::from_bits(set.bits()), set);
}
