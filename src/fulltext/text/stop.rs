//! The English stop words [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.4.2]:
//! implementation-defined, here the Snowball English stop-word list
//! (snowballstem.org, `algorithms/english/stop.txt`), its contractions
//! written with U+0027 as `normalize` writes them. An index may keep its
//! own stop words instead, or none (`StopWords`).

use std::collections::BTreeSet;
use std::sync::Arc;

use super::{Language, normalize};

/// What separates the words of a stop-word list.
const LIST_SEPARATOR: char = ',';

/// The stop words a document's tokens are marked by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopWords {
    /// The language's own list.
    Language,
    /// No stop words.
    None,
    /// Exactly these words, normalized.
    Words(Arc<BTreeSet<String>>),
}

impl StopWords {
    /// The stop words listed in `text`, an option's value: its words split
    /// at commas, each trimmed and normalized, empty ones dropped. A list
    /// with no word is `StopWords::None`.
    pub fn parse(text: &str) -> StopWords {
        let words: BTreeSet<String> = text.split(LIST_SEPARATOR).map(str::trim).filter(|word| !word.is_empty()).map(normalize).collect();
        if words.is_empty() { StopWords::None } else { StopWords::Words(Arc::new(words)) }
    }

    /// Whether the normalized word `word` is a stop word of a document in
    /// `language`.
    pub fn contains(&self, language: Language, word: &str) -> bool {
        match self {
            StopWords::Language => super::is_stop(language, word),
            StopWords::None => false,
            StopWords::Words(words) => words.contains(word),
        }
    }
}

/// The English stop words, normalized, in byte order so a binary search
/// finds them.
pub(super) const ENGLISH: [&str; 174] = [
    "a",
    "about",
    "above",
    "after",
    "again",
    "against",
    "all",
    "am",
    "an",
    "and",
    "any",
    "are",
    "aren't",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can't",
    "cannot",
    "could",
    "couldn't",
    "did",
    "didn't",
    "do",
    "does",
    "doesn't",
    "doing",
    "don't",
    "down",
    "during",
    "each",
    "few",
    "for",
    "from",
    "further",
    "had",
    "hadn't",
    "has",
    "hasn't",
    "have",
    "haven't",
    "having",
    "he",
    "he'd",
    "he'll",
    "he's",
    "her",
    "here",
    "here's",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "how's",
    "i",
    "i'd",
    "i'll",
    "i'm",
    "i've",
    "if",
    "in",
    "into",
    "is",
    "isn't",
    "it",
    "it's",
    "its",
    "itself",
    "let's",
    "me",
    "more",
    "most",
    "mustn't",
    "my",
    "myself",
    "no",
    "nor",
    "not",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "ought",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "same",
    "shan't",
    "she",
    "she'd",
    "she'll",
    "she's",
    "should",
    "shouldn't",
    "so",
    "some",
    "such",
    "than",
    "that",
    "that's",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "there's",
    "these",
    "they",
    "they'd",
    "they'll",
    "they're",
    "they've",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "wasn't",
    "we",
    "we'd",
    "we'll",
    "we're",
    "we've",
    "were",
    "weren't",
    "what",
    "what's",
    "when",
    "when's",
    "where",
    "where's",
    "which",
    "while",
    "who",
    "who's",
    "whom",
    "why",
    "why's",
    "with",
    "won't",
    "would",
    "wouldn't",
    "you",
    "you'd",
    "you'll",
    "you're",
    "you've",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

/// Whether the normalized word `word` is an English stop word.
pub(super) fn is_english_stop(word: &str) -> bool {
    ENGLISH.binary_search(&word).is_ok()
}
