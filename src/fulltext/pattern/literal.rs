//! A `<word>` or `<phrase>` as a pattern writes it, read into a `Literal`
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1 Description 3a, 3b]:
//! its escapes taken out, its wild cards marked, its words normalized.
//!
//! The choices this standard leaves to the implementation are these.
//!
//! - A `<word separator>` is any white space [§5.3.1 Description 3b], so
//!   `"International Standard"` is a phrase of two words.
//! - Each word is normalized as the tokenizer normalizes a document's
//!   [§4.5, §6.4.5], its wild cards kept: each run of characters between
//!   wild cards is normalized by itself. A word with no wild card is cut
//!   as the tokenizer cuts a document, so one it reads as several words
//!   is a phrase of them.
//! - A part of a phrase that is one or more `%` alone is an optional word
//!   [§5.3.1 NOTE 13]; one written with the escape character is the
//!   character `%` [§5.3.1 NOTE 11].

use crate::error::Result;
use crate::fulltext::pattern::lex::invalid;
use crate::fulltext::pattern::{Literal, Mask, MaskChar, Part};
use crate::fulltext::{Language, is_stop, normalize, tokenize};

/// The wild card for any characters, none too [§5.3.1 NOTE 12].
const ANY: char = '%';
/// The wild card for one character [§5.3.1 NOTE 12].
const ONE: char = '_';

/// A word or phrase as written: its characters inside the quotes, where
/// they begin, and its escape character, if it has one.
#[derive(Debug, Clone, Copy)]
pub(super) struct Written<'t> {
    /// The `<word representation>` or `<phrase representation>`.
    pub text: &'t str,
    /// The position of its opening quote, from 1.
    pub at: usize,
    /// The `<escape representation character>`.
    pub escape: Option<char>,
}

/// A character of a written word or phrase, its escapes read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// A character of a word.
    Char(MaskChar),
    /// A `<word separator>`.
    Separator,
}

/// The literal `written` writes, of `language`, stemmed if `stemmed`.
///
/// # Errors
///
/// SQLSTATE 2FF01 if it is empty, a word is wild cards only, an escape
/// character stands before a character it cannot escape, or a phrase has
/// optional words only [§5.3.1 Description 3a, 3b].
pub(super) fn literal(written: Written<'_>, language: Language, stemmed: bool) -> Result<Literal> {
    let pieces = pieces(written)?;
    let groups: Vec<&[Piece]> = pieces.split(|p| *p == Piece::Separator).filter(|g| !g.is_empty()).collect();
    let phrase = groups.len() > 1;
    let mut parts = Vec::with_capacity(groups.len());
    for group in groups {
        let chars: Vec<MaskChar> = group
            .iter()
            .filter_map(|p| match p {
                Piece::Char(c) => Some(*c),
                Piece::Separator => None,
            })
            .collect();
        let optional = chars.iter().all(|c| *c == MaskChar::Any);
        if phrase && optional {
            parts.push(Part::Optional);
        } else if chars.iter().any(|c| matches!(c, MaskChar::Literal(_))) {
            parts.push(Part::Word(normalized(&chars)));
        } else {
            return Err(invalid(
                written.at,
                "[ISO/IEC 13249-2:2002 §5.3.1 Description 3a] a word must hold a character that is not a wild card",
            ));
        }
    }
    if !parts.iter().any(|p| matches!(p, Part::Word(_))) {
        return Err(invalid(written.at, "[ISO/IEC 13249-2:2002 §5.3.1 Description 3b] a phrase must hold a word"));
    }
    let mut split = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            Part::Word(mask) => split.extend(words(mask, language)?),
            Part::Optional => split.push(Part::Optional),
        }
    }
    Ok(Literal { language, stemmed, parts: split })
}

/// The word `mask` as the tokenizer would read it in a document: the words
/// it cuts it into, its characters between them dropped, so `"e-mail"` is
/// the phrase `e mail` and `"Standard."` the word `standard` [§4.5]. A
/// word with a wild card, or one the tokenizer finds no word in, is kept.
fn words(mask: Mask, language: Language) -> Result<Vec<Part>> {
    let Some(text) = mask.text() else { return Ok(vec![Part::Word(mask)]) };
    let mut out = Vec::new();
    for token in tokenize(text.chars().map(Ok), language) {
        out.push(Part::Word(Mask(token?.word.chars().map(MaskChar::Literal).collect())));
    }
    if out.is_empty() {
        out.push(Part::Word(mask));
    }
    Ok(out)
}

/// The characters of `written`, its escapes read [§5.3.1 Description 3a].
fn pieces(written: Written<'_>) -> Result<Vec<Piece>> {
    let mut out = Vec::new();
    let mut chars = written.text.chars();
    while let Some(c) = chars.next() {
        let piece = if Some(c) == written.escape {
            let Some(next) = chars.next().filter(|next| *next == c || *next == ANY || *next == ONE) else {
                return Err(invalid(
                    written.at,
                    "[ISO/IEC 13249-2:2002 §5.3.1 Description 3a] an escape character must be followed by itself, % or _",
                ));
            };
            Piece::Char(MaskChar::Literal(next))
        } else {
            piece(c)
        };
        out.push(piece);
    }
    Ok(out)
}

/// The piece the unescaped character `c` is.
fn piece(c: char) -> Piece {
    match c {
        ANY => Piece::Char(MaskChar::Any),
        ONE => Piece::Char(MaskChar::One),
        c => {
            if c.is_whitespace() {
                Piece::Separator
            } else {
                Piece::Char(MaskChar::Literal(c))
            }
        }
    }
}

/// `chars` normalized: each run of characters between wild cards
/// normalized as a document's words are [§4.5].
fn normalized(chars: &[MaskChar]) -> Mask {
    let mut out = Vec::with_capacity(chars.len());
    let mut run = String::new();
    for c in chars {
        match c {
            MaskChar::Literal(c) => run.push(*c),
            MaskChar::One | MaskChar::Any => {
                out.extend(normalize(&run).chars().map(MaskChar::Literal));
                run.clear();
                out.push(*c);
            }
        }
    }
    out.extend(normalize(&run).chars().map(MaskChar::Literal));
    Mask(out)
}

/// Whether `mask` is a stop word of `language`: a word with no wild card
/// that the language takes for one [§4.4.2].
pub(super) fn is_stop_word(mask: &Mask, language: Language) -> bool {
    mask.text().is_some_and(|text| is_stop(language, &text))
}

/// The literal of the thesaurus term `term` in `language`: its words, as
/// the tokenizer cuts and normalizes them, each matched as written, so a
/// `%` or `_` in a term is no wild card. None if it has no word.
///
/// # Errors
///
/// As the tokenizer's.
pub(super) fn term_literal(term: &str, language: Language) -> Result<Option<Literal>> {
    let mut parts = Vec::new();
    for token in tokenize(term.chars().map(Ok), language) {
        parts.push(Part::Word(Mask(token?.word.chars().map(MaskChar::Literal).collect())));
    }
    Ok((!parts.is_empty()).then_some(Literal { language, stemmed: false, parts }))
}
