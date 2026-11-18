//! Unit tests of search patterns: reading them, expanding them and
//! evaluating them over small documents.

mod errors;
mod eval;
mod expand;
mod parse;

use super::*;
use crate::error::SqlError;

/// `text` read, English by default.
fn p(text: &str) -> Pattern {
    parse(text, Language::English).unwrap_or_else(|e| panic!("{text}: {e:?}"))
}

/// The error of reading `text`.
fn err(text: &str) -> SqlError {
    parse(text, Language::English).expect_err(text)
}

/// The mask of `word`: `%` and `_` its wild cards.
fn mask(word: &str) -> Mask {
    Mask(
        word.chars()
            .map(|c| match c {
                '%' => MaskChar::Any,
                '_' => MaskChar::One,
                c => MaskChar::Literal(c),
            })
            .collect(),
    )
}

/// The English literal of `words`, `%` alone an optional word.
fn lit(words: &str) -> Literal {
    let parts = words.split(' ').map(|w| if w == "%" { Part::Optional } else { Part::Word(mask(w)) }).collect();
    Literal { language: Language::English, stemmed: false, parts }
}

/// `lit(words)`, stemmed.
fn stemmed(words: &str) -> Literal {
    Literal { stemmed: true, ..lit(words) }
}

/// Whether the English document `document` matches `pattern`.
fn holds(pattern: &str, document: &str) -> bool {
    contains(&p(pattern), document.chars().map(Ok), Language::English).unwrap()
}
