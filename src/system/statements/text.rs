//! \[DESIGN\] A statement's text, and its normalized form (RULES.md D521):
//! its tokens [X3.135-1986 §5.3] one space apart, each literal a `?`, so
//! that runs differing in their literals alone are tallied as one, with
//! the dynamic parameters a prepared statement writes. Key words are as
//! the lexer folds them, a regular identifier upper case, a delimited one
//! quoted as written. Its key is the normalized text's XXH3-64
//! (`hash/xxhash.rs`). A text the lexer refuses is its words one space
//! apart. It is made once for a prepared statement, and once for each
//! text a session runs directly while it keeps it (`Activity::text_of`),
//! never at each run.

use crate::hash::xxhash::xxh3_64;
use crate::lexer::{Tok, delim, lex};

/// What a literal is written as in a normalized text.
const PLACEHOLDER: &str = "?";

/// A statement's text, and its normalized form and key.
#[derive(Debug)]
pub struct Text {
    raw: String,
    normalized: String,
    key: u64,
}

impl Text {
    /// The text `sql`, normalized.
    pub fn of(sql: &str) -> Text {
        let normalized = normalized(sql);
        let key = xxh3_64(normalized.as_bytes());
        Text { raw: sql.to_owned(), normalized, key }
    }

    /// The text as written.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The text normalized.
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// The normalized text's hash.
    pub fn key(&self) -> u64 {
        self.key
    }
}

/// `sql`'s tokens one space apart, each literal a `?`.
fn normalized(sql: &str) -> String {
    let Ok(tokens) = lex(sql) else { return sql.split_whitespace().collect::<Vec<_>>().join(" ") };
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut joined = true;
    for token in &tokens {
        let written: String = chars.get(token.start..token.end).map(|c| c.iter().collect()).unwrap_or_default();
        let (piece, glued_before, glued_after) = match &token.tok {
            Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_) | Tok::CharacterString(_) | Tok::BinaryString(_) => {
                (PLACEHOLDER.to_owned(), false, false)
            }
            Tok::KeyWord(word) => ((*word).to_owned(), false, false),
            Tok::Identifier(name) => (if written.starts_with('"') { written } else { name.clone() }, false, false),
            Tok::Introducer(_) | Tok::LargeObjectLength(_) => (written, false, false),
            Tok::Delimiter(d) => ((*d).to_owned(), glued_left(d), glued_right(d)),
            Tok::End => break,
        };
        if !(joined || glued_before) {
            out.push(' ');
        }
        out.push_str(&piece);
        joined = glued_after;
    }
    out
}

/// Whether the delimiter `d` is written against what comes before it.
fn glued_left(d: &str) -> bool {
    [delim::COMMA, delim::RIGHT_PAREN, delim::PERIOD, delim::SEMICOLON].contains(&d)
}

/// Whether what comes after the delimiter `d` is written against it.
fn glued_right(d: &str) -> bool {
    [delim::LEFT_PAREN, delim::PERIOD].contains(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D521: texts differing in their literals,
    /// their spacing or the case of their key words and regular
    /// identifiers alone are one normalized text, of one key; a delimited
    /// identifier stays as written; a dynamic parameter is a `?` as a
    /// literal is; a text the lexer refuses is its words.
    #[test]
    fn literals_become_placeholders() {
        let one = Text::of("select a, b from t where a = 1 and b = 'x'");
        let two = Text::of("SELECT  A,B\nFROM T WHERE A = 42 AND B = 'yy'");
        assert_eq!(one.normalized(), "SELECT A, B FROM T WHERE A = ? AND B = ?");
        assert_eq!((one.key(), one.raw()), (two.key(), "select a, b from t where a = 1 and b = 'x'"));
        assert_eq!(Text::of("SELECT \"a\".x FROM \"a\" WHERE x = ?").normalized(), "SELECT \"a\".X FROM \"a\" WHERE X = ?");
        assert_eq!(Text::of("VALUES (1.5E3, X'0A')").normalized(), "VALUES (?, ?)");
        assert_ne!(Text::of("SELECT 1 FROM T").key(), Text::of("SELECT 1 FROM U").key());
        assert_eq!(Text::of("SELECT 'open  ").normalized(), "SELECT 'open");
    }
}
