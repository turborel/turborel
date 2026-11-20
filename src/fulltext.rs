//! Full text [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4]: a document's
//! text cut into words, sentences and paragraphs, each word normalized,
//! so the inverted index (`index/fulltext.rs`, RULES.md D86) keeps where
//! each word is and a search finds it.
//!
//! The tokenizer (`fulltext/text.rs`) is implementation-defined in every
//! part the standard leaves open [§4.1, §4.4, §4.5]: what a word is, where
//! sentences and paragraphs end, how a word is normalized, its stem and
//! which words are stop words, by language.

pub mod matches;
pub mod pattern;
pub mod rank;
pub mod text;
pub mod thesaurus;
pub mod types;
pub mod value;

pub use text::{
    Language, Languages, StopWords, Token, Tokens, is_single_letter, is_stop, normalize, normalize_in, stem, stems, tokenize, tokenize_with,
};
