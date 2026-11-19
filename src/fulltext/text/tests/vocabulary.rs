//! The stemmers against Snowball's own vocabularies (RULES.md D320,
//! D326): a sample of each language's words, every k-th of its whole
//! vocabulary, each stemmed as Snowball's stemmer stems it.

use crate::fulltext::text::{Language, stem};

/// Each stemmed language and the sample of its vocabulary, a word and its
/// stem a line, separated by a tab, after `#` comment lines.
const SAMPLES: [(Language, &str); 35] = [
    (Language::Arabic, include_str!("../snowball/vocabulary/arabic.voc")),
    (Language::Armenian, include_str!("../snowball/vocabulary/armenian.voc")),
    (Language::Basque, include_str!("../snowball/vocabulary/basque.voc")),
    (Language::Catalan, include_str!("../snowball/vocabulary/catalan.voc")),
    (Language::Czech, include_str!("../snowball/vocabulary/czech.voc")),
    (Language::Danish, include_str!("../snowball/vocabulary/danish.voc")),
    (Language::Dutch, include_str!("../snowball/vocabulary/dutch.voc")),
    (Language::English, include_str!("../snowball/vocabulary/english.voc")),
    (Language::Esperanto, include_str!("../snowball/vocabulary/esperanto.voc")),
    (Language::Estonian, include_str!("../snowball/vocabulary/estonian.voc")),
    (Language::Finnish, include_str!("../snowball/vocabulary/finnish.voc")),
    (Language::French, include_str!("../snowball/vocabulary/french.voc")),
    (Language::German, include_str!("../snowball/vocabulary/german.voc")),
    (Language::Greek, include_str!("../snowball/vocabulary/greek.voc")),
    (Language::Hindi, include_str!("../snowball/vocabulary/hindi.voc")),
    (Language::Hungarian, include_str!("../snowball/vocabulary/hungarian.voc")),
    (Language::Indonesian, include_str!("../snowball/vocabulary/indonesian.voc")),
    (Language::Irish, include_str!("../snowball/vocabulary/irish.voc")),
    (Language::Italian, include_str!("../snowball/vocabulary/italian.voc")),
    (Language::Lithuanian, include_str!("../snowball/vocabulary/lithuanian.voc")),
    (Language::Nepali, include_str!("../snowball/vocabulary/nepali.voc")),
    (Language::Norwegian, include_str!("../snowball/vocabulary/norwegian.voc")),
    (Language::Persian, include_str!("../snowball/vocabulary/persian.voc")),
    (Language::Polish, include_str!("../snowball/vocabulary/polish.voc")),
    (Language::Portuguese, include_str!("../snowball/vocabulary/portuguese.voc")),
    (Language::Romanian, include_str!("../snowball/vocabulary/romanian.voc")),
    (Language::Russian, include_str!("../snowball/vocabulary/russian.voc")),
    (Language::Serbian, include_str!("../snowball/vocabulary/serbian.voc")),
    (Language::Sesotho, include_str!("../snowball/vocabulary/sesotho.voc")),
    (Language::Spanish, include_str!("../snowball/vocabulary/spanish.voc")),
    (Language::Swedish, include_str!("../snowball/vocabulary/swedish.voc")),
    (Language::Tamil, include_str!("../snowball/vocabulary/tamil.voc")),
    (Language::Turkish, include_str!("../snowball/vocabulary/turkish.voc")),
    (Language::Ukrainian, include_str!("../snowball/vocabulary/ukrainian.voc")),
    (Language::Yiddish, include_str!("../snowball/vocabulary/yiddish.voc")),
];

/// What begins a comment line of a sample.
const COMMENT: char = '#';

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] [EXTENSION] (RULES.md
/// D320) each language stems every word of its sample as Snowball does,
/// and every stemmed language has a sample.
#[test]
fn stems_match_snowballs_vocabularies() {
    let mut checked = 0_usize;
    for (language, sample) in SAMPLES {
        let mut pairs = 0_usize;
        for line in sample.lines().filter(|line| !line.starts_with(COMMENT)) {
            let (word, expected) = line.split_once('\t').unwrap_or_else(|| panic!("{}: {line}", language.name()));
            assert_eq!(stem(language, word), expected, "{}: {word}", language.name());
            pairs += 1;
        }
        assert!(pairs >= 90, "{}: {pairs}", language.name());
        checked += pairs;
    }
    assert_eq!(SAMPLES.len(), Language::all().filter(|language| language.stems()).count());
    assert!(checked > 15_000, "{checked}");
}
