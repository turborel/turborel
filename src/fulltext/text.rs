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
//! - Languages [§4.4]. Each language has its own normalization, stems,
//!   stop words and cutting into words (`language.rs`, RULES.md D321): a
//!   pattern's word in its language, a document's word in its.
//! - Normalization [§4.5]. In NONE and ENGLISH each character is simply
//!   case folded, canonically decomposed, folded again and its nonspacing
//!   marks (Mn) taken out, and U+2019 becomes U+0027: `Résumé` is
//!   `resume`. In a stemmed language a word is case folded and composed
//!   (NFC), its accents kept for its stemmer; in one read as bigrams it is
//!   composed for compatibility (NFKC), so full-width forms are one.
//! - Elisions. In French, Italian, Catalan and Irish, an article or
//!   pronoun elided before an apostrophe is taken off its word.
//! - Bigrams. In Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese,
//!   a run of the letters of the scripts written without spaces is read
//!   as its overlapping bigrams, each a word, one letter alone a word of
//!   one (D323).
//! - Stop words [§4.4.2, §5.1.5 DR6]. A language has Snowball's list if
//!   Snowball has one (`stop.rs`), NONE none. An index may name its own
//!   list or none (`StopWords`). Stop words are tokens all the same,
//!   marked `stop`, so the index keeps their places.
//! - Stems [§4.2.3.4]. A language stems by its Snowball program
//!   (`snowball.rs`, D320); NONE's stem of a word, and that of a language
//!   read as bigrams, is the word.
//! - Served languages. A language a plug-in serves (`served.rs`, RULES.md
//!   D328) has a document read whole and cut by one call, its words'
//!   sentences and paragraphs those the rules above give, and its words
//!   stemmed a batch a call.

mod kind;
mod language;
mod segment;
pub mod served;
mod snowball;
mod stop;
#[cfg(test)]
mod tests;

pub use language::{Language, Languages};
pub use segment::MAX_WORD_CHARS;
pub use stop::StopWords;

use crate::error::Result;
use kind::tables;
use segment::Segmenter;

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
    /// Of a language a plug-in cuts into words: the language, its stop
    /// words, and the tokens of the document once it is read whole.
    served: Option<(Language, StopWords, Option<std::vec::IntoIter<Token>>)>,
}

/// The tokens of the characters `chars` of a document in `language`, read
/// one at a time, so a document of any length is never held whole.
pub fn tokenize<I: Iterator<Item = Result<char>>>(chars: I, language: Language) -> Tokens<I> {
    tokenize_with(chars, language, &StopWords::Language)
}

/// The tokens of the characters `chars` of a document in `language`, as
/// `tokenize` reads them, each marked `stop` by the stop words `stop`.
pub fn tokenize_with<I: Iterator<Item = Result<char>>>(chars: I, language: Language, stop: &StopWords) -> Tokens<I> {
    let served = language.served_segments().then(|| (language, stop.clone(), None));
    Tokens { chars, segmenter: Segmenter::new(language, stop.clone()), done: false, served }
}

impl<I: Iterator<Item = Result<char>>> Tokens<I> {
    /// The next token of a document a plug-in cuts into words: the
    /// document read whole and cut at the first.
    fn next_served(&mut self) -> Option<Result<Token>> {
        let (language, stop, read) = self.served.as_mut()?;
        if read.is_none() {
            let text: Result<String> = self.chars.by_ref().collect();
            match text.and_then(|text| served::tokens(&text, *language, stop)) {
                Ok(tokens) => *read = Some(tokens.into_iter()),
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        read.as_mut()?.next().map(Ok)
    }
}

impl<I: Iterator<Item = Result<char>>> Iterator for Tokens<I> {
    type Item = Result<Token>;

    fn next(&mut self) -> Option<Result<Token>> {
        if self.done {
            return None;
        }
        if self.served.is_some() {
            return self.next_served();
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

/// `word` normalized [§4.5] as NONE and ENGLISH normalize: each
/// character simply case folded, canonically decomposed and folded again,
/// nonspacing marks taken out, U+2019 written U+0027.
pub fn normalize(word: &str) -> String {
    normalize_in(Language::None, word)
}

/// `word` normalized [§4.5] as `language` normalizes its words.
pub fn normalize_in(language: Language, word: &str) -> String {
    match tables() {
        Ok(tables) => tables.normalize_as(language.profile().fold, word),
        // The tables are compiled in and checked by the tests; were one
        // damaged, `tokenize` reports it, and this falls back to lower case.
        Err(_) => word.to_lowercase(),
    }
}

/// The stem of the word `word`, normalized in `language`, in that
/// language [§4.2.3.4]: by its Snowball stemmer, the word itself in NONE
/// and in a language read as bigrams; by its plug-in, if one stems it.
///
/// # Errors
///
/// A served language's call's (`served.rs`).
pub fn stem(language: Language, word: &str) -> Result<String> {
    let mut got = stems(language, &[word])?;
    Ok(got.pop().unwrap_or_else(|| word.to_owned()))
}

/// The stems of the words `words`, as `stem` gives each: of a language a
/// plug-in stems, one call for all of them that it has not stemmed yet.
///
/// # Errors
///
/// As `stem`.
pub fn stems(language: Language, words: &[&str]) -> Result<Vec<String>> {
    if let Language::Served(slot) = language
        && language.served_stems()
    {
        return served::stems(slot, words);
    }
    Ok(match language.stemmer() {
        Some(stemmer) => words.iter().map(|word| stemmer.stem(word)).collect(),
        None => words.iter().map(|word| (*word).to_owned()).collect(),
    })
}

/// Whether the word `word`, normalized in `language`, is a stop word of
/// that language [§4.4.2]: Snowball's list of it, none if Snowball has
/// none.
pub fn is_stop(language: Language, word: &str) -> bool {
    if let Some(served) = language.serving()
        && let Some(list) = served.stop_words()
    {
        return list.binary_search_by(|listed| listed.as_str().cmp(word)).is_ok();
    }
    stop_list(language).binary_search_by(|listed| listed.as_str().cmp(word)).is_ok()
}

/// The stop words of `language`, normalized in it, sorted, each once.
fn stop_list(language: Language) -> &'static [String] {
    language.stop_list(|listed| normalize_in(language, listed))
}

/// Whether the normalized word `word` is a single letter, with its marks,
/// of a script `language` reads as bigrams: one a document holds alone or
/// inside a bigram (D323).
pub fn is_single_letter(language: Language, word: &str) -> bool {
    if !language.reads_bigrams() {
        return false;
    }
    let Ok(tables) = tables() else { return false };
    let mut letters = word.chars().filter(|c| !tables.is_mark(*c));
    let first = letters.next();
    letters.next().is_none() && first.is_some_and(|c| tables.is_unit(c))
}
