//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14.5] The tokens of the
//! linear referencing well-known text: key words, `<text>`, `<number>`,
//! integers, `<unit of measure>` and the punctuation, read front to back
//! by byte offset, each slice taken with `str::get` so no read panics.

use super::super::failed;
use super::{NULL, STOP_WORDS};
use crate::error::{Result, SqlError, SqlState};

/// The `<double quote>` around a `<text>`.
const QUOTE: char = '"';

/// A reader of linear referencing well-known text.
pub(crate) struct Lexer<'t> {
    /// The text.
    text: &'t str,
    /// The byte offset of the next character.
    at: usize,
}

impl<'t> Lexer<'t> {
    /// A reader of `text` from its start.
    pub fn new(text: &'t str) -> Lexer<'t> {
        Lexer { text, at: 0 }
    }

    /// What is left, from the next character.
    fn rest(&self) -> &'t str {
        self.text.get(self.at..).unwrap_or_default()
    }

    /// What is left, from the next token, for a look ahead.
    pub fn remaining(&mut self) -> &'t str {
        self.skip_spaces();
        self.rest()
    }

    /// Steps past the spaces before the next token.
    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.at = self.at.saturating_add(rest.len().saturating_sub(rest.trim_start().len()));
    }

    /// Steps past `n` bytes.
    fn advance(&mut self, n: usize) {
        self.at = self.at.saturating_add(n).min(self.text.len());
    }

    /// The next character, after spaces, not taken.
    pub fn peek(&mut self) -> Option<char> {
        self.skip_spaces();
        self.rest().chars().next()
    }

    /// The word next: letters, digits and underscores, not taken.
    pub fn peek_word(&mut self) -> &'t str {
        self.skip_spaces();
        let rest = self.rest();
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        rest.get(..end).unwrap_or_default()
    }

    /// Takes the key word `word`, in any case, if it is next.
    pub fn eat(&mut self, word: &str) -> bool {
        let next = self.peek_word();
        if next.eq_ignore_ascii_case(word) {
            self.advance(next.len());
            return true;
        }
        false
    }

    /// Takes the key word `word`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if it is not next.
    pub fn expect(&mut self, word: &str) -> Result<()> {
        if self.eat(word) {
            return Ok(());
        }
        Err(self.invalid(&format!("{word} expected")))
    }

    /// Takes `c`, if it is next.
    pub fn eat_char(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.advance(c.len_utf8());
            return true;
        }
        false
    }

    /// Takes `c`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if it is not next.
    pub fn expect_char(&mut self, c: char) -> Result<()> {
        if self.eat_char(c) {
            return Ok(());
        }
        Err(self.invalid(&format!("{c} expected")))
    }

    /// [§15.14.5 `<text>`] A double-quoted string. \[EXTENSION\] A double
    /// quote inside it is written twice, as in an SQL delimited
    /// identifier, since `<letters>` has no way to write one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if none is next or it is not closed.
    pub fn text(&mut self) -> Result<String> {
        self.expect_char(QUOTE)?;
        let mut out = String::new();
        loop {
            let rest = self.rest();
            let Some(end) = rest.find(QUOTE) else { return Err(self.invalid("a text is not closed")) };
            out.push_str(rest.get(..end).unwrap_or_default());
            self.advance(end.saturating_add(QUOTE.len_utf8()));
            if !self.rest().starts_with(QUOTE) {
                return Ok(out);
            }
            out.push(QUOTE);
            self.advance(QUOTE.len_utf8());
        }
    }

    /// A `<text>`, or \[EXTENSION\] the key word NULL for the null value.
    ///
    /// # Errors
    ///
    /// As `text`.
    pub fn text_or_null(&mut self) -> Result<Option<String>> {
        if self.eat(NULL) {
            return Ok(None);
        }
        self.text().map(Some)
    }

    /// [§15.14.5 `<number>`] An SQL `<exact numeric literal>` or
    /// `<approximate numeric literal>`, with a sign as `<signed numeric
    /// literal>` allows, since a measure or a coordinate may be negative.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if none is next or it is not finite.
    pub fn number(&mut self) -> Result<f64> {
        self.skip_spaces();
        let rest = self.rest();
        let end = rest.find(|c: char| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))).unwrap_or(rest.len());
        let literal = rest.get(..end).unwrap_or_default();
        let value = literal.parse::<f64>().ok().filter(|v| v.is_finite());
        let Some(v) = value else { return Err(self.invalid("a number expected")) };
        self.advance(end);
        Ok(v)
    }

    /// [§15.14.5 `<positive integer>`, `<unsigned integer>`] An integer.
    /// Lenient reading: a sign and zero are taken too, as an INTEGER
    /// attribute holds them and the writer writes any.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if none is next or it is out of range.
    pub fn integer(&mut self) -> Result<i32> {
        self.skip_spaces();
        let rest = self.rest();
        let digits = rest.strip_prefix(|c: char| c == '+' || c == '-').unwrap_or(rest);
        let end =
            rest.len().saturating_sub(digits.len()).saturating_add(digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len()));
        let value = rest.get(..end).and_then(|literal| literal.parse::<i32>().ok());
        let Some(v) = value else { return Err(self.invalid("an integer expected")) };
        self.advance(end);
        Ok(v)
    }

    /// [§15.14.2 `<unit of measure>`] The unit after a measure value, if
    /// one is: its words, up to a comma, a parenthesis or a key word that
    /// may follow a measure; or \[EXTENSION\] a `<text>`, for a unit name
    /// those would end.
    ///
    /// # Errors
    ///
    /// As `text`.
    pub fn unit(&mut self) -> Result<Option<String>> {
        if self.peek() == Some(QUOTE) {
            return self.text().map(Some);
        }
        let mut words: Vec<&str> = Vec::new();
        loop {
            let word = self.peek_unit_word();
            if word.is_empty() || STOP_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word)) {
                break;
            }
            words.push(word);
            self.advance(word.len());
        }
        Ok((!words.is_empty()).then(|| words.join(" ")))
    }

    /// The word of a unit name next, not taken: `<letter>`s other than
    /// the parentheses, the space and the comma.
    fn peek_unit_word(&mut self) -> &'t str {
        self.skip_spaces();
        let rest = self.rest();
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '\''))).unwrap_or(rest.len());
        rest.get(..end).unwrap_or_default()
    }

    /// [§5.1.67] The extent of a geometry's well-known text next: its
    /// words, then EMPTY or its balanced parentheses, taken.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if no geometry is next or its parentheses are not
    /// balanced.
    pub fn geometry_text(&mut self) -> Result<&'t str> {
        self.skip_spaces();
        let start = self.at;
        while !self.peek_word().is_empty() {
            let word = self.peek_word();
            self.advance(word.len());
            if word.eq_ignore_ascii_case(super::EMPTY) {
                return Ok(self.text.get(start..self.at).unwrap_or_default());
            }
        }
        if self.peek() != Some('(') || self.at == start {
            return Err(self.invalid("a geometry expected"));
        }
        let mut open = 0_usize;
        for (i, c) in self.rest().char_indices() {
            match c {
                '(' => open = open.saturating_add(1),
                ')' => open = open.saturating_sub(1),
                _ => continue,
            }
            if open == 0 {
                self.advance(i.saturating_add(1));
                return Ok(self.text.get(start..self.at).unwrap_or_default());
            }
        }
        Err(self.invalid("a geometry's parentheses are not balanced"))
    }

    /// Checks that nothing but spaces is left.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 if something is.
    pub fn finish(&mut self) -> Result<()> {
        if self.peek().is_none() {
            return Ok(());
        }
        Err(self.invalid("text follows its end"))
    }

    /// SQLSTATE 2FF22, at the next character.
    pub fn invalid(&self, detail: &str) -> SqlError {
        failed(
            SqlState::SPATIAL_INVALID_WKT,
            "§15.14",
            &format!(
                "invalid well-known text representation: {detail} at character {}",
                self.text.get(..self.at).unwrap_or_default().chars().count()
            ),
        )
    }
}
