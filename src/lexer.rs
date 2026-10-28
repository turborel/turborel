//! Lexical analysis: [X3.135-1986 §5.1] `<character>`, [X3.135-1986 §5.2] `<literal>`, [X3.135-1986 §5.3] `<token>`,
//! with the identifiers and key words of [ISO/IEC 9075:1992 §5.2], which
//! where the two differ takes priority (README).
//!
//! Notable consequences:
//! * A `<regular identifier>` is a letter, then letters, digits and
//!   underscores, at most 128 of them [ISO/IEC 9075:1992 §5.2 SR1, SR8];
//!   its lower case letters are the upper case ones \[SR10\], and a `<key
//!   word>` is one in any case \[SR15\]. A `<delimited identifier>`, in
//!   double quotes, keeps its case and may be a key word \[SR9, SR14\].
//! * The `<key word>`s are SQL-92's `<reserved word>`s; none is an
//!   `<identifier>` \[SR11\], so SQL-86 text naming a column ADD or VALUE
//!   must write it as a `<delimited identifier>`.
//! * A `<separator>` is spaces, newlines and comments. A tab is not a `<space>`.
//! * A `<comment>` is "--" up to and including a `<newline>`, so it cannot be the
//!   last thing in the input.
//! * A `<character string literal>` may be empty, and may go on after a
//!   `<separator>` with a `<newline>` in another quoted part [ISO/IEC
//!   9075:1992 §5.3 SR1, SR4, SR9].

use crate::error::{Code, Result, SqlError};

/// Declares a constant for each `<key word>`, named as it is written, in
/// `kw`, and `KEY_WORDS`, the list of them all.
macro_rules! key_words {
    ($($word:ident)*) => {
        /// [ISO/IEC 9075:1992 §5.2] The `<reserved word>`s, one constant
        /// each, so that a misspelled key word does not compile.
        pub mod kw {
            $(
                #[doc = concat!("The `<key word>` ", stringify!($word), ".")]
                pub const $word: &str = stringify!($word);
            )*
        }

        /// [ISO/IEC 9075:1992 §5.2] `<reserved word>`: every reserved word of the language, including
        /// ones whose statements are not implemented here. None may be used as a `<regular identifier>`
        /// \[SR11\]. (END-EXEC, of embedded SQL, is not a word this lexer reads at all.) The
        /// `<non-reserved word>`s are `<regular identifier>`s here, which a statement that uses one
        /// matches as such (the levels of isolation, `parser/session.rs`).
        pub const KEY_WORDS: &[&str] = &[$(kw::$word),*];
    };
}

key_words! {
    ABSOLUTE ACTION ADD ALL ALLOCATE ALTER AND ANY ARE AS ASC ASSERTION AT AUTHORIZATION AVG
    BEGIN BETWEEN BIT BIT_LENGTH BOTH BY
    CASCADE CASCADED CASE CAST CATALOG CHAR CHARACTER CHAR_LENGTH CHARACTER_LENGTH CHECK CLOSE
    COALESCE COLLATE COLLATION COLUMN COMMIT CONNECT CONNECTION CONSTRAINT CONSTRAINTS CONTINUE
    CONVERT CORRESPONDING COUNT CREATE CROSS CURRENT CURRENT_DATE CURRENT_TIME CURRENT_TIMESTAMP
    CURRENT_USER CURSOR
    DATE DAY DEALLOCATE DEC DECIMAL DECLARE DEFAULT DEFERRABLE DEFERRED DELETE DESC DESCRIBE
    DESCRIPTOR DIAGNOSTICS DISCONNECT DISTINCT DOMAIN DOUBLE DROP
    ELSE END ESCAPE EXCEPT EXCEPTION EXEC EXECUTE EXISTS EXTERNAL EXTRACT
    FALSE FETCH FIRST FLOAT FOR FOREIGN FOUND FROM FULL
    GET GLOBAL GO GOTO GRANT GROUP
    HAVING HOUR
    IDENTITY IMMEDIATE IN INDICATOR INITIALLY INNER INPUT INSENSITIVE INSERT INT INTEGER INTERSECT
    INTERVAL INTO IS ISOLATION
    JOIN
    KEY
    LANGUAGE LAST LEADING LEFT LEVEL LIKE LOCAL LOWER
    MATCH MAX MIN MINUTE MODULE MONTH
    NAMES NATIONAL NATURAL NCHAR NEXT NO NOT NULL NULLIF NUMERIC
    OCTET_LENGTH OF ON ONLY OPEN OPTION OR ORDER OUTER OUTPUT OVERLAPS
    PAD PARTIAL POSITION PRECISION PREPARE PRESERVE PRIMARY PRIOR PRIVILEGES PROCEDURE PUBLIC
    READ REAL REFERENCES RELATIVE RESTRICT REVOKE RIGHT ROLLBACK ROWS
    SCHEMA SCROLL SECOND SECTION SELECT SESSION SESSION_USER SET SIZE SMALLINT SOME SPACE SQL
    SQLCODE SQLERROR SQLSTATE SUBSTRING SUM SYSTEM_USER
    TABLE TEMPORARY THEN TIME TIMESTAMP TIMEZONE_HOUR TIMEZONE_MINUTE TO TRAILING TRANSACTION
    TRANSLATE TRANSLATION TRIM TRUE
    UNION UNIQUE UNKNOWN UPDATE UPPER USAGE USER USING
    VALUE VALUES VARCHAR VARYING VIEW
    WHEN WHENEVER WHERE WITH WORK WRITE
    YEAR
    ZONE
}

/// [X3.135-1986 §5.3] The `<delimiter token>`s other than `<character string
/// literal>`, one constant each.
pub mod delim {
    /// The not equals operator.
    pub const NOT_EQUALS: &str = "<>";
    /// The greater than or equals operator.
    pub const GREATER_OR_EQUAL: &str = ">=";
    /// The less than or equals operator.
    pub const LESS_OR_EQUAL: &str = "<=";
    /// Comma.
    pub const COMMA: &str = ",";
    /// Left parenthesis.
    pub const LEFT_PAREN: &str = "(";
    /// Right parenthesis.
    pub const RIGHT_PAREN: &str = ")";
    /// The less than operator.
    pub const LESS: &str = "<";
    /// The greater than operator.
    pub const GREATER: &str = ">";
    /// Period.
    pub const PERIOD: &str = ".";
    /// Colon.
    pub const COLON: &str = ":";
    /// The equals operator.
    pub const EQUALS: &str = "=";
    /// Asterisk.
    pub const ASTERISK: &str = "*";
    /// Plus sign.
    pub const PLUS: &str = "+";
    /// Minus sign.
    pub const MINUS: &str = "-";
    /// Solidus.
    pub const SOLIDUS: &str = "/";
    /// [ISO/IEC 9075:1992 §5.2] The concatenation operator.
    pub const CONCATENATION: &str = "||";
    /// [ISO/IEC 9075:1992 §5.1] The question mark: an `<SQL special
    /// character>`, so a `<delimiter token>` [§5.2], and the `<dynamic
    /// parameter specification>` [§6.2].
    pub const QUESTION: &str = "?";
}

/// Every `<delimiter token>` but `<character string literal>`, longest first
/// so that "<>" wins over "<".
const DELIMITERS: &[&str] = &[
    delim::CONCATENATION,
    delim::NOT_EQUALS,
    delim::GREATER_OR_EQUAL,
    delim::LESS_OR_EQUAL,
    delim::COMMA,
    delim::LEFT_PAREN,
    delim::RIGHT_PAREN,
    delim::LESS,
    delim::GREATER,
    delim::PERIOD,
    delim::COLON,
    delim::EQUALS,
    delim::ASTERISK,
    delim::PLUS,
    delim::MINUS,
    delim::SOLIDUS,
    delim::QUESTION,
];

/// The quote that delimits a `<character string literal>` [X3.135-1986 §5.2].
const QUOTE: char = '\'';
/// The double quote that delimits a `<delimited identifier>` [ISO/IEC 9075:1992 §5.2].
const DOUBLE_QUOTE: char = '"';
/// [X3.135-1986 §5.2] `<quote representation>` ::= '': two quotes, standing for one.
const QUOTE_REPRESENTATION_LEN: usize = 2;
/// The implementor-defined end-of-line indicators: "\n", and "\r\n".
const LF: &str = "\n";
const CRLF: &str = "\r\n";

/// [ISO/IEC 9075:1992 §5.2 SR8, SR9] the most characters of an `<identifier>`.
pub const MAX_IDENTIFIER_LENGTH: usize = 128;

/// A `<token>` [X3.135-1986 §5.3], or the end of the input.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// A `<key word>`, as it appears in `KEY_WORDS`.
    KeyWord(&'static str),
    /// An `<identifier>`: a `<regular identifier>` in upper case, or the
    /// body of a `<delimited identifier>`.
    Identifier(String),
    /// Unsigned `<exact numeric literal>` text, e.g. "12.50". A sign written
    /// directly against it is handled by the parser (see `Parser::literal`).
    ExactNumeric(String),
    /// Unsigned-mantissa `<approximate numeric literal>` text, e.g. "1.5E-3".
    ApproximateNumeric(String),
    /// The value of a `<character string literal>`, with '' already turned into '.
    CharacterString(String),
    /// A `<delimiter token>` other than a `<character string literal>`.
    Delimiter(&'static str),
    /// The end of the input.
    End,
}

/// A token and where it was written.
#[derive(Debug, Clone)]
pub struct Token {
    /// The token.
    pub tok: Tok,
    /// The character offset of its first character.
    pub start: usize,
    /// The character offset just past its last character.
    pub end: usize,
}

/// [ISO/IEC 9075:1992 §5.2 SR1] `<identifier start>`: a letter of the
/// character repertoire, Unicode's [X3.135-1986 §5.1, implementor-defined].
fn is_identifier_start(c: char) -> bool {
    c.is_alphabetic()
}
fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}
/// [ISO/IEC 9075:1992 §5.2] after the first: `<underscore>` | `<identifier part>`.
fn is_identifier_part(c: char) -> bool {
    is_identifier_start(c) || is_digit(c) || c == '_'
}

fn error(code: Code, pos: usize, msg: impl Into<String>) -> SqlError {
    SqlError::at(code, pos, msg)
}

/// [X3.135-1986 §5.3] Splits `src` into `<token>`s, dropping the
/// `<separator>`s between them, and ends the list with `Tok::End`.
///
/// # Errors
///
/// A character that cannot start a `<token>`, an unterminated `<comment>` or
/// `<character string literal>`, a token not followed by a separator or
/// delimiter, or an `<identifier>` too long.
pub fn lex(src: &str) -> Result<Vec<Token>> {
    let mut s = Scanner { c: src.chars().collect(), at: 0 };
    let mut toks = Vec::new();
    while let Some(first) = s.peek(0) {
        let skipped = s.skip_separator()?;
        if skipped {
            continue;
        }
        let start = s.at;
        let tok = s.token(first)?;
        s.check_followed(&tok, start)?;
        toks.push(Token { tok, start, end: s.at });
    }
    toks.push(Token { tok: Tok::End, start: s.at, end: s.at });
    Ok(toks)
}

/// \[EXTENSION\] [ISO/IEC 9075:1992 §5.4] `<authorization identifier>` ::=
/// `<identifier>`: `text`, one identifier alone, as a user names itself
/// outside SQL text, to a server or a tool. A regular identifier is in upper
/// case, a delimited one's body as written.
///
/// # Errors
///
/// SQLCODE -101 if `text` is not made of tokens; -102 if it is not one
/// `<identifier>` alone, as a key word is not.
pub fn authorization_identifier(text: &str) -> Result<String> {
    match lex(text)?.as_slice() {
        [Token { tok: Tok::Identifier(name), .. }, Token { tok: Tok::End, .. }] => Ok(name.clone()),
        _ => Err(SqlError::new(Code::Syntax, format!("[ISO/IEC 9075:1992 §5.4] {text} is not an <authorization identifier>"))),
    }
}

/// The input as characters, and the offset of the next one to read. Reads
/// past the end see no character, so no offset can go out of bounds.
struct Scanner {
    c: Vec<char>,
    at: usize,
}

impl Scanner {
    /// The character `k` places after the next one.
    fn peek(&self, k: usize) -> Option<char> {
        self.c.get(self.at.saturating_add(k)).copied()
    }

    /// Whether the character `k` places on is one that `f` accepts.
    fn next_is(&self, k: usize, f: impl Fn(char) -> bool) -> bool {
        self.peek(k).is_some_and(f)
    }

    fn advance(&mut self, n: usize) {
        self.at = self.at.saturating_add(n);
    }

    fn skip_while(&mut self, f: impl Fn(char) -> bool) {
        while self.next_is(0, &f) {
            self.advance(1);
        }
    }

    /// The characters from offset `start` up to the next one.
    fn text(&self, start: usize) -> String {
        self.c.get(start..self.at).unwrap_or_default().iter().collect()
    }

    /// The implementor-defined end-of-line indicator: "\n" (or "\r\n"); its length.
    fn newline_len(&self) -> Option<usize> {
        match (self.peek(0), self.peek(1)) {
            (Some('\n'), _) => Some(LF.len()),
            (Some('\r'), Some('\n')) => Some(CRLF.len()),
            _ => None,
        }
    }

    fn delimiter(&self) -> Option<&'static str> {
        DELIMITERS.iter().copied().find(|d| d.chars().enumerate().all(|(k, dc)| self.peek(k) == Some(dc)))
    }

    /// Skips one part of a `<separator>` ::= {`<comment>` | `<space>` |
    /// `<newline>`}..., and says whether there was one.
    fn skip_separator(&mut self) -> Result<bool> {
        if self.peek(0) == Some(' ') {
            self.advance(1);
            return Ok(true);
        }
        if let Some(n) = self.newline_len() {
            self.advance(n);
            return Ok(true);
        }
        if self.peek(0) != Some('-') || self.peek(1) != Some('-') {
            return Ok(false);
        }
        // <comment> ::= --[<character>...]<newline>
        let start = self.at;
        while self.peek(0).is_some() && self.newline_len().is_none() {
            self.advance(1);
        }
        let n =
            self.newline_len().ok_or_else(|| error(Code::Lexical, start, "[X3.135-1986 §5.3] a <comment> must end with a <newline>"))?;
        self.advance(n);
        Ok(true)
    }

    /// The `<token>` that starts with `first`, the next character.
    fn token(&mut self, first: char) -> Result<Tok> {
        // [ISO/IEC 9075:1992 §5.3] `<national character string literal>` ::=
        // N, then a character string literal's quoted parts: of the
        // national character set, SQL_TEXT, the one there is [§6.1 SR2].
        if matches!(first, 'N' | 'n') && self.peek(1) == Some(QUOTE) {
            self.advance(1);
            return self.character_string();
        }
        if is_identifier_start(first) {
            self.regular_identifier()
        } else if is_digit(first) || (first == '.' && self.next_is(1, is_digit)) {
            Ok(self.number())
        } else if first == QUOTE {
            self.character_string()
        } else if first == DOUBLE_QUOTE {
            self.delimited_identifier()
        } else if let Some(d) = self.delimiter() {
            self.advance(d.chars().count());
            Ok(Tok::Delimiter(d))
        } else {
            Err(error(Code::Lexical, self.at, format!("[X3.135-1986 §5.3 F] character {first:?} cannot start a <token>")))
        }
    }

    /// [ISO/IEC 9075:1992 §5.2] `<regular identifier>` ::= `<identifier start>`
    /// \[{`<underscore>` | `<identifier part>`}...\], in upper case \[SR10\]; or
    /// a `<key word>`, in any case \[SR15\].
    fn regular_identifier(&mut self) -> Result<Tok> {
        let start = self.at;
        self.advance(1);
        self.skip_while(is_identifier_part);
        // [SR8] Its <identifier start>s and <identifier part>s, which an
        // <underscore> is not, are counted.
        let length = self.text(start).chars().filter(|&c| c != '_').count();
        let word = self.text(start).to_uppercase();
        // [ISO/IEC 9075:1992 §5.2 SR11] a <regular identifier> is not a <reserved word>.
        if let Some(kw) = KEY_WORDS.iter().find(|k| **k == word) {
            return Ok(Tok::KeyWord(kw));
        }
        self.identifier_length(start, length)?;
        Ok(Tok::Identifier(word))
    }

    /// [ISO/IEC 9075:1992 §5.2] `<delimited identifier>` ::= `<double quote>`
    /// `<delimited identifier part>`... `<double quote>`, where a
    /// `<doublequote symbol>` "" stands for one; its case is kept.
    fn delimited_identifier(&mut self) -> Result<Tok> {
        let start = self.at;
        self.advance(1);
        let mut body = String::new();
        let mut parts = 0_usize;
        loop {
            let Some(ch) = self.peek(0) else {
                return Err(error(Code::Lexical, start, "[ISO/IEC 9075:1992 §5.2] unterminated <delimited identifier>"));
            };
            if self.newline_len().is_some() {
                return Err(error(
                    Code::Lexical,
                    self.at,
                    "[ISO/IEC 9075:1992 §5.2 SR3] a <newline> cannot appear inside a <delimited identifier>",
                ));
            }
            self.advance(1);
            if ch == DOUBLE_QUOTE {
                if self.peek(0) != Some(DOUBLE_QUOTE) {
                    break;
                }
                self.advance(1);
            }
            body.push(ch);
            parts = parts.saturating_add(1);
        }
        if body.is_empty() {
            return Err(error(Code::Syntax, start, "[ISO/IEC 9075:1992 §5.2] a <delimited identifier body> has at least one part"));
        }
        self.identifier_length(start, parts)?;
        Ok(Tok::Identifier(body))
    }

    /// [ISO/IEC 9075:1992 §5.2 SR8, SR9] an `<identifier>` of `length`
    /// characters, at `start`, is at most `MAX_IDENTIFIER_LENGTH`.
    fn identifier_length(&self, start: usize, length: usize) -> Result<()> {
        if length <= MAX_IDENTIFIER_LENGTH {
            return Ok(());
        }
        Err(error(
            Code::SyntaxRule,
            start,
            format!(
                "[ISO/IEC 9075:1992 §5.2 SR8, SR9] <identifier> {} is longer than {MAX_IDENTIFIER_LENGTH} characters",
                self.text(start)
            ),
        ))
    }

    /// [X3.135-1986 §5.2] `<exact numeric literal>` (unsigned part):
    ///   `<unsigned integer>`[.`<unsigned integer>`] | `<unsigned integer>`. | .`<unsigned integer>`
    ///
    /// or `<approximate numeric literal>` ::= `<mantissa>`E`<exponent>`, where
    /// `<exponent>` ::= `<signed integer>` ::= [+|-] `<unsigned integer>`.
    fn number(&mut self) -> Tok {
        let start = self.at;
        self.skip_while(is_digit);
        if self.peek(0) == Some('.') {
            self.advance(1);
            self.skip_while(is_digit);
        }
        if self.peek(0) == Some('E') {
            // The E, then the sign if there is one.
            let before_digits = usize::from(matches!(self.peek(1), Some('+' | '-'))).saturating_add(1);
            if self.next_is(before_digits, is_digit) {
                self.advance(before_digits);
                self.skip_while(is_digit);
                return Tok::ApproximateNumeric(self.text(start));
            }
        }
        Tok::ExactNumeric(self.text(start))
    }

    /// [ISO/IEC 9075:1992 §5.3] `<character string literal>` ::= '
    /// \[`<character representation>`...\] ' \[{`<separator>`... '
    /// \[`<character representation>`...\] '}...\], where `<quote
    /// representation>` ::= ''. The parts are one literal [SR1] if each
    /// separator has a `<newline>` [SR4]; it may be empty [SR9].
    fn character_string(&mut self) -> Result<Tok> {
        let mut value = String::new();
        loop {
            self.quoted_part(&mut value)?;
            let after = self.at;
            while self.skip_separator()? {}
            let newline = self.c.get(after..self.at).unwrap_or_default().contains(&'\n');
            if !(newline && self.peek(0) == Some(QUOTE)) {
                self.at = after;
                return Ok(Tok::CharacterString(value));
            }
        }
    }

    /// One quoted part of a `<character string literal>`, its characters
    /// added to `value`.
    fn quoted_part(&mut self, value: &mut String) -> Result<()> {
        let start = self.at;
        self.advance(1);
        loop {
            let Some(ch) = self.peek(0) else {
                return Err(error(Code::Lexical, start, "[X3.135-1986 §5.2 F] unterminated <character string literal>"));
            };
            // [X3.135-1986 §5.1 SR1] the end-of-line indicator is not a <character>.
            if self.newline_len().is_some() {
                return Err(error(
                    Code::Lexical,
                    self.at,
                    "[X3.135-1986 §5.1 SR1] a <newline> cannot appear inside a <character string literal>",
                ));
            }
            if ch == QUOTE {
                if self.peek(1) != Some(QUOTE) {
                    self.advance(1);
                    break;
                }
                value.push(QUOTE);
                self.advance(QUOTE_REPRESENTATION_LEN);
                continue;
            }
            value.push(ch);
            self.advance(1);
        }
        Ok(())
    }

    /// Whether `next`, the next character, ends a `<nondelimiter token>`: a
    /// `<space>`, a quote or double quote (which start `<delimiter token>`s),
    /// a `<newline>`, or another `<delimiter token>`.
    fn at_token_boundary(&self, next: char) -> bool {
        next == ' ' || next == QUOTE || next == DOUBLE_QUOTE || self.newline_len().is_some() || self.delimiter().is_some()
    }

    /// [X3.135-1986 §5.3 SR2] a `<nondelimiter token>` shall be followed by a
    /// `<delimiter token>` or a `<separator>` (or the end of the input).
    /// `tok` began at `start` and the next character follows it.
    fn check_followed(&self, tok: &Tok, start: usize) -> Result<()> {
        // A <delimited identifier> is a <delimiter token> [ISO/IEC 9075:1992 §5.2].
        let delimited = self.c.get(start) == Some(&DOUBLE_QUOTE);
        let nondelimiter =
            !delimited && matches!(tok, Tok::KeyWord(_) | Tok::Identifier(_) | Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_));
        let Some(next) = self.peek(0) else { return Ok(()) };
        if !nondelimiter || self.at_token_boundary(next) {
            return Ok(());
        }
        Err(error(
            Code::Lexical,
            self.at,
            format!("[X3.135-1986 §5.3 SR2] {:?} must be followed by a <delimiter token> or a <separator>, not {next:?}", self.text(start)),
        ))
    }
}
