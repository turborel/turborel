//! \[DESIGN\] A statement's text, and its normalized form (RULES.md D521):
//! its tokens [X3.135-1986 §5.3] one space apart, each literal a `?`, so
//! that runs differing in their literals alone are tallied as one, with
//! the dynamic parameters a prepared statement writes. Key words are as
//! the lexer folds them, a regular identifier upper case, a delimited one
//! quoted as written. Its key is the normalized text's XXH3-64
//! (`hash/xxhash.rs`). A text the lexer refuses is its words one space
//! apart. It is made once for a prepared statement, and once for each
//! text a session runs directly while it keeps it (`Activity::text_of`),
//! never at each run; of a text run directly, from the tokens its parser
//! reads, so the text is lexed once (`lexer::Lexed`).

use crate::hash::xxhash::xxh3_64;
use crate::lexer::{Lexed, Tok, delim, lexed};

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
        Text::of_lexed(sql, lexed(sql).ok().as_ref())
    }

    /// The text `sql`, normalized from its tokens, `lexed`, if the lexer
    /// takes it: a statement run directly is lexed once, for this and its
    /// parser alike.
    pub fn of_lexed(sql: &str, lexed: Option<&Lexed>) -> Text {
        let normalized = lexed.map_or_else(|| sql.split_whitespace().collect::<Vec<_>>().join(" "), |l| normalized(sql, l));
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

/// `sql`'s tokens, `lexed`, one space apart, each literal a `?`: each
/// piece written straight into the text, none copied apart first.
fn normalized(sql: &str, Lexed { toks, chars }: &Lexed) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut joined = true;
    for token in toks {
        let (glued_before, glued_after) = match &token.tok {
            Tok::Delimiter(d) => (glued_left(d), glued_right(d)),
            Tok::End => break,
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_) => (false, false),
        };
        if !(joined || glued_before) {
            out.push(' ');
        }
        piece(&mut out, &token.tok, chars.get(token.start..token.end).unwrap_or_default());
        joined = glued_after;
    }
    out
}

/// The token `tok`, written as `written`, as a normalized text has it.
fn piece(out: &mut String, tok: &Tok, written: &[char]) {
    match tok {
        Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_) | Tok::CharacterString(_) | Tok::BinaryString(_) => out.push_str(PLACEHOLDER),
        Tok::KeyWord(word) => out.push_str(word),
        Tok::Identifier(name) => {
            if written.first() == Some(&'"') {
                out.extend(written);
            } else {
                out.push_str(name);
            }
        }
        Tok::Introducer(_) | Tok::LargeObjectLength(_) => out.extend(written),
        Tok::Delimiter(d) => out.push_str(d),
        Tok::End => {}
    }
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

    /// Verifies: [DESIGN] RULES.md D521: a text normalized from the tokens
    /// its parser reads keeps its introducers, large object lengths and
    /// delimited identifiers as written, its literals `?`; of no tokens, as
    /// of a text the lexer refuses, it is its words.
    #[test]
    fn a_text_lexed_once_is_normalized_from_its_tokens() {
        let normalized = |sql: &str| Text::of_lexed(sql, Some(&lexed(sql).unwrap())).normalized().to_owned();
        assert_eq!(normalized("insert into \"Mixed\" (k, c) values (1, _latin1'x')"), "INSERT INTO \"Mixed\" (K, C) VALUES (?, _latin1 ?)");
        assert_eq!(normalized("CREATE TABLE B (L BLOB(64k), K INTEGER)  "), "CREATE TABLE B (L BLOB (64k), K INTEGER)");
        assert_eq!(normalized("SELECT X.Y FROM X WHERE Y IN (1.5, X'0A', ?);"), "SELECT X.Y FROM X WHERE Y IN (?, ?, ?);");
        assert_eq!(Text::of_lexed("a  b\nc", None).normalized(), "a b c");
    }
}
