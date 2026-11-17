//! \[EXTENSION\] TEXT_MATCHES(document, words): whether a document holds
//! every word of a search, the words compared normalized [ISO/IEC
//! 13249-2:2002 (CD, SC32 N0640) §4.5], stop words of the search left out
//! [§4.4.2]. The standard's CONTAINS method of FULLTEXT values takes a
//! text search language; this function takes plain words, and is what a
//! full-text index (`index/fulltext.rs`, RULES.md D86) finds rows for
//! (D34): it is deterministic and never raises for character strings,
//! but for a large object's storage failing as it is read.
//!
//! Both arguments are character strings, large objects too, read a chunk
//! at a time. A null argument gives null; words with no word that is not a
//! stop word give true, as every document holds them all.

use std::collections::HashSet;

use crate::error::{Code, Result, SqlError};
use crate::fulltext::{Language, tokenize};
use crate::functions::ScalarFunction;
use crate::index::fulltext::chars_of;
use crate::params::ParameterType;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};

/// The name of `TextMatches`.
pub const TEXT_MATCHES: &str = "TEXT_MATCHES";

/// \[EXTENSION\] The language TEXT_MATCHES reads its arguments in.
pub const MATCH_LANGUAGE: Language = Language::English;

/// The normalized words of `words` that are not stop words of
/// `MATCH_LANGUAGE`, each once; none if `words` is null.
///
/// # Errors
///
/// SQLCODE -900 if it is not a character string; a large object's read
/// errors.
pub fn search_words(words: &Value) -> Result<Option<Vec<String>>> {
    let Some(chars) = chars_of(words)? else { return Ok(None) };
    let mut out = Vec::new();
    for token in tokenize(chars, MATCH_LANGUAGE) {
        let token = token?;
        if !token.stop {
            out.push(token.word);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(Some(out))
}

/// \[EXTENSION\] The function TEXT_MATCHES.
pub struct TextMatches;

/// The error of a call not of two character strings.
fn signature() -> SqlError {
    SqlError::new(Code::SyntaxRule, "[EXTENSION] TEXT_MATCHES takes two character strings, a document and words")
}

/// Whether the document `document` holds every word of `wanted`; read only
/// until the last is found.
///
/// # Errors
///
/// As reading it.
fn holds(document: &Value, mut wanted: HashSet<String>) -> Result<Option<bool>> {
    let Some(chars) = chars_of(document)? else { return Ok(None) };
    if wanted.is_empty() {
        return Ok(Some(true));
    }
    for token in tokenize(chars, MATCH_LANGUAGE) {
        wanted.remove(&token?.word);
        if wanted.is_empty() {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

impl ScalarFunction for TextMatches {
    fn name(&self) -> &str {
        TEXT_MATCHES
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [document, words] = args else { return Err(signature()) };
        if document.is_character() && words.is_character() { Ok(DataType::Boolean) } else { Err(signature()) }
    }
    /// A parameter is a CHARACTER VARYING of the longest length.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(DataType::varying(MAX_CHARACTER_LENGTH)))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [document, words] = args else { return Err(signature()) };
        if document.is_null() {
            return Ok(Value::Null);
        }
        let Some(wanted) = search_words(words)? else { return Ok(Value::Null) };
        Ok(holds(document, wanted.into_iter().collect())?.map_or(Value::Null, Value::Bool))
    }
    fn index_predicate(&self) -> bool {
        true
    }
    fn deterministic(&self) -> bool {
        true
    }
}

// Unit tests: the function's truth table, of strings and large objects
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::lob::{Lob, LobKind};

    fn text(s: &str) -> Value {
        Value::Char(s.into())
    }

    fn call(document: Value, words: Value) -> Value {
        TextMatches.call(&[document, words]).unwrap()
    }

    /// Verifies: [EXTENSION] TEXT_MATCHES: true iff every word of the
    /// search occurs among the document's, compared normalized, in any
    /// order; false when one is missing; null when either argument is
    /// null, the null document checked first and the null words too.
    #[test]
    fn every_word_must_occur_normalized() {
        let document = text("The quick brown Fox, jumping.");
        assert!(matches!(call(document.clone(), text("fox QUICK")), Value::Bool(true)));
        assert!(matches!(call(document.clone(), text("fox quick")), Value::Bool(true)));
        assert!(matches!(call(document.clone(), text("fox cat")), Value::Bool(false)));
        assert!(matches!(call(document.clone(), text("jump")), Value::Bool(false)), "a prefix is not a word");
        assert!(call(Value::Null, text("fox")).is_null());
        assert!(call(document, Value::Null).is_null());
        assert!(call(Value::Null, Value::Null).is_null());
    }

    /// Verifies: [EXTENSION] TEXT_MATCHES: words with no word in them give
    /// true for every document, an empty one too, and null for a null
    /// document.
    #[test]
    fn no_words_match_every_document() {
        assert!(matches!(call(text("anything"), text("")), Value::Bool(true)));
        assert!(matches!(call(text(""), text("  ,. ")), Value::Bool(true)));
        assert!(matches!(call(text(""), text("word")), Value::Bool(false)));
        assert!(call(Value::Null, text("")).is_null());
    }

    /// Verifies: [EXTENSION] TEXT_MATCHES of character large objects:
    /// either argument may be one, read a chunk at a time, and gives what
    /// the same text as a string gives; a binary large object is refused.
    #[test]
    fn large_objects_match_as_strings_do() {
        let long = format!("{} needle", "hay ".repeat(100_000));
        let lob = Value::Lob(Lob::of_text(&long));
        assert!(matches!(call(lob.clone(), text("needle hay")), Value::Bool(true)));
        assert!(matches!(call(lob.clone(), text("pin")), Value::Bool(false)));
        assert!(matches!(call(text("a needle"), Value::Lob(Lob::of_text("needle"))), Value::Bool(true)));
        assert!(matches!(call(lob, Value::Lob(Lob::empty(LobKind::Character))), Value::Bool(true)));
        let binary = Value::Lob(Lob::of_octets(b"needle"));
        assert_eq!(TextMatches.call(&[binary, text("needle")]).unwrap_err().code, Code::Api);
    }

    /// Verifies: [EXTENSION] TEXT_MATCHES's declaration: two character
    /// strings of any kind give BOOLEAN; another count or type is refused
    /// with SQLCODE -103 (syntax rule); it is an index predicate and
    /// deterministic; a parameter is a character string.
    #[test]
    fn it_takes_two_character_strings() {
        let clob = DataType::Lob { kind: LobKind::Character, length: 1_000, units: crate::value::CharUnits::Characters };
        for (a, b) in [(DataType::varying(10), DataType::character(3)), (clob.clone(), DataType::varying(5))] {
            assert_eq!(TextMatches.return_type(&[a, b]).unwrap(), DataType::Boolean);
        }
        assert_eq!(TextMatches.return_type(&[DataType::Boolean, clob.clone()]).unwrap_err().code, Code::SyntaxRule);
        assert_eq!(TextMatches.return_type(&[clob]).unwrap_err().code, Code::SyntaxRule);
        assert!(TextMatches.index_predicate() && TextMatches.deterministic());
        assert!(matches!(TextMatches.parameter_type(1, &[]), Some(ParameterType::Of(t)) if t.is_character()));
        assert_eq!(TextMatches.call(&[text("a")]).unwrap_err().code, Code::SyntaxRule);
    }
}
