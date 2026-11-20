//! The stop words a document's tokens are marked by [ISO/IEC 13249-2:2002
//! (CD, SC32 N0640) §4.4.2]: implementation-defined, by default its
//! language's, Snowball's list of the language (snowballstem.org,
//! `algorithms/<language>/stop.txt`, kept in `stop/` under Snowball's BSD
//! license, whose notice is `stop/COPYING`) normalized as the language
//! normalizes its words. An index may keep its own stop words instead, or
//! none.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::{Language, normalize_in};

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
    /// at commas, each trimmed and normalized as `language` normalizes its
    /// words, empty ones dropped. A list with no word is `StopWords::None`.
    pub fn parse(text: &str, language: Language) -> StopWords {
        let words: BTreeSet<String> =
            text.split(LIST_SEPARATOR).map(str::trim).filter(|word| !word.is_empty()).map(|word| normalize_in(language, word)).collect();
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
