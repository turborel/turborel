//! The tokenizer: a document's characters cut into words, each with its
//! place: its ordinal among the document's words, its sentence, its
//! paragraph and its character offset [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §4.1, §5.1.5]. Every rule here is implementation-defined
//! [§4.1, §4.4, §4.5, §5.1.4 DR5]; the choices are these.
//!
//! - Streaming. Characters are read one at a time and only the word being
//!   read is held, its first `MAX_WORD_CHARS` characters at most, so a
//!   document of gigabytes is never held whole. An error reading the
//!   characters is the stream's last item, and the word being read is
//!   dropped.
//! - Words. A word is a maximal run of letters (L*), numbers (N*) and
//!   marks (M*) of Unicode 13.0. One connector (Pc, as `_`) joins two such
//!   runs, and one apostrophe (U+0027 or U+2019) joins two letters, so
//!   `don't` is one word and `'quoted'` is the word `quoted`. Each
//!   ideograph or kana (an Lo character of the Han, Hiragana or Katakana
//!   blocks) is a word by itself, with any marks after it.
//! - Sentences. A sentence ends after `.`, `!` or `?`, with any closing
//!   brackets, closing quotes and further terminators after it, when a
//!   blank (Zs or a tab), a line terminator or the end follows. So `3.14`
//!   and `e.g.x` end none. The ideographic and full-width terminators
//!   (U+3002, U+FF01, U+FF0E, U+FF1F) end a sentence by themselves, as
//!   text in ideographs puts no space after them.
//! - Paragraphs. A paragraph ends at two line terminators in a row (LF,
//!   CR, CRLF counted once, VT, FF, NEL, U+2028), only blanks between,
//!   or at U+2029. A paragraph end also ends the sentence. Sentences and
//!   paragraphs are numbered from 1, and only those holding a word count.
//! - Places. Every word, stop words too, has an ordinal from 1, so
//!   distances stay right [§5.1.5 DR7-DR9]. A word's offset and length are
//!   in characters, as written, and stop at `u32::MAX`.
//! - Normalization [§4.5]. Each character is simply case folded,
//!   canonically decomposed, folded again and its nonspacing marks (Mn)
//!   taken out, and U+2019 becomes U+0027: `Résumé` is `resume`.
//! - Stop words [§4.4.2, §5.1.5 DR6]. English has the Snowball list
//!   (`stop.rs`), NONE none. An index may name its own list or none
//!   (`StopWords`). Stop words are tokens all the same, marked `stop`, so
//!   the index keeps their places.
//! - Stems [§4.2.3.4]. English stems by Porter2 (`porter2.rs`); NONE's
//!   stem of a word is the word.

mod kind;
mod porter2;
mod segment;
mod snowball;
mod stop;
#[cfg(test)]
mod tests;

pub use segment::MAX_WORD_CHARS;
pub use stop::StopWords;

use crate::error::{Code, Result, SqlError, SqlState};
use kind::tables;
use segment::Segmenter;

/// A language the tokenizer knows [§4.4]: what words it stems and which
/// it takes for stop words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// English: Porter2 stems and English stop words.
    English,
    /// No language: no stem but the word, and no stop words.
    None,
}

/// The name of `Language::English`.
const ENGLISH: &str = "ENGLISH";
/// The name of `Language::None`.
const NONE: &str = "NONE";

impl Language {
    /// The language named `name`, its case not counting.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 if the tokenizer knows none of that name
    /// [§5.1.9 GR].
    pub fn named(name: &str) -> Result<Language> {
        match name.to_ascii_uppercase().as_str() {
            ENGLISH => Ok(Language::English),
            NONE => Ok(Language::None),
            other => Err(SqlError::new(Code::Data, format!("[ISO/IEC 13249-2:2002 §5.1.9] no language is named {other}"))
                .with_state(SqlState::INVALID_LANGUAGE_SPECIFICATION)),
        }
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Language::English => ENGLISH,
            Language::None => NONE,
        }
    }
}

/// A word of a document and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The word, normalized (`normalize`).
    pub word: String,
    /// Its ordinal among the document's words, stop words counted, from 1.
    pub ordinal: u32,
    /// The ordinal of its sentence, from 1.
    pub sentence: u32,
    /// The ordinal of its paragraph, from 1.
    pub paragraph: u32,
    /// The offset of its first character in the document, from 0.
    pub offset: u32,
    /// Its characters, as written.
    pub length: u32,
    /// Whether it is a stop word of the language.
    pub stop: bool,
}

/// The tokens of a document's characters, as they are read.
pub struct Tokens<I> {
    chars: I,
    segmenter: Segmenter,
    done: bool,
}

/// The tokens of the characters `chars` of a document in `language`, read
/// one at a time, so a document of any length is never held whole.
pub fn tokenize<I: Iterator<Item = Result<char>>>(chars: I, language: Language) -> Tokens<I> {
    tokenize_with(chars, language, &StopWords::Language)
}

/// The tokens of the characters `chars` of a document in `language`, as
/// `tokenize` reads them, each marked `stop` by the stop words `stop`.
pub fn tokenize_with<I: Iterator<Item = Result<char>>>(chars: I, language: Language, stop: &StopWords) -> Tokens<I> {
    Tokens { chars, segmenter: Segmenter::new(language, stop.clone()), done: false }
}

impl<I: Iterator<Item = Result<char>>> Iterator for Tokens<I> {
    type Item = Result<Token>;

    fn next(&mut self) -> Option<Result<Token>> {
        if self.done {
            return None;
        }
        let tables = match tables() {
            Ok(tables) => tables,
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        for c in self.chars.by_ref() {
            match c {
                Ok(c) => {
                    if let Some(token) = self.segmenter.step(tables, c) {
                        return Some(Ok(token));
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        self.done = true;
        self.segmenter.finish(tables).map(Ok)
    }
}

/// `word` normalized [§4.5]: each character simply case folded,
/// canonically decomposed and folded again, nonspacing marks taken out,
/// U+2019 written U+0027.
pub fn normalize(word: &str) -> String {
    match tables() {
        Ok(tables) => tables.normalize(word),
        // The tables are compiled in and checked by the tests; were one
        // damaged, `tokenize` reports it, and this falls back to lower case.
        Err(_) => word.to_lowercase(),
    }
}

/// The stem of the normalized word `word` in `language` [§4.2.3.4]:
/// Porter2 for English, the word itself for NONE.
pub fn stem(language: Language, word: &str) -> String {
    match language {
        Language::English => porter2::stem(word),
        Language::None => word.to_owned(),
    }
}

/// Whether the normalized word `word` is a stop word of `language`
/// [§4.4.2]: the Snowball list for English, none for NONE.
pub fn is_stop(language: Language, word: &str) -> bool {
    match language {
        Language::English => stop::is_english_stop(word),
        Language::None => false,
    }
}
