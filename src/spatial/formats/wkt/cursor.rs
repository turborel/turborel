//! A cursor over well-known text tokens, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5.1.67, with the rules every production shares: `<empty set>`,
//! parenthesised lists, `<point>` against the coordinate dimensions
//! (rule ao) and the `<z m>` and `<just z>` markers (rules ai to am).

use super::lexer::{Token, invalid};
use crate::error::Result;
use crate::spatial::curves::fault::{Fault, fault};
use crate::spatial::curves::types::{Coord, Dims};
use crate::spatial::formats::tree::{Kind, Ring};

/// The tokens and the read position, with the value's dimensions.
pub struct Cursor {
    tokens: Vec<Token>,
    at: usize,
    /// The dimensions the outermost `<z m>` set.
    pub dims: Dims,
}

/// A 2FF25 error for `detail`.
pub fn mixed(detail: &str) -> crate::error::SqlError {
    fault(Fault::MixedDimensions, super::lexer::CLAUSE, detail)
}

impl Cursor {
    /// A cursor at the first of `tokens`, two-dimensional until a marker
    /// says otherwise.
    pub fn new(tokens: Vec<Token>) -> Cursor {
        Cursor { tokens, at: 0, dims: Dims::Xy }
    }

    /// The next token, not consumed.
    pub fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// Whether every token is read.
    pub fn done(&self) -> bool {
        self.at >= self.tokens.len()
    }

    /// The next token.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 at the end of the text.
    pub fn token(&mut self) -> Result<Token> {
        let token = self.tokens.get(self.at).cloned().ok_or_else(|| invalid("the text ends early"))?;
        self.at = self.at.saturating_add(1);
        Ok(token)
    }

    /// Consumes `token`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 when the next token is another.
    pub fn expect(&mut self, token: &Token) -> Result<()> {
        let found = self.token()?;
        if &found == token { Ok(()) } else { Err(invalid(&format!("expected {token:?}, found {found:?}"))) }
    }

    /// Whether the next token is keyword `word`.
    pub fn is_word(&self, word: &str) -> bool {
        if let Some(Token::Word(w)) = self.peek() { w == word } else { false }
    }

    /// Consumes keyword `word` when it is next.
    pub fn take_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        if found {
            self.at = self.at.saturating_add(1);
        }
        found
    }

    /// Consumes keyword `word`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 when another token is next.
    pub fn word(&mut self, word: &str) -> Result<()> {
        if self.take_word(word) { Ok(()) } else { Err(invalid(&format!("expected {word}"))) }
    }

    /// Consumes `,` when it is next.
    pub fn take_comma(&mut self) -> bool {
        let found = self.peek() == Some(&Token::Comma);
        if found {
            self.at = self.at.saturating_add(1);
        }
        found
    }

    /// Reads `<empty set>` or `(`: whether a body follows.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 for anything else.
    pub fn opens(&mut self) -> Result<bool> {
        if self.take_word("EMPTY") {
            return Ok(false);
        }
        self.expect(&Token::Open)?;
        Ok(true)
    }

    /// Reads `item`s separated by commas up to `)`, after the `(`.
    ///
    /// # Errors
    ///
    /// As `item`'s, and 2FF22 for a missing comma or parenthesis.
    pub fn items<T>(&mut self, mut item: impl FnMut(&mut Cursor) -> Result<T>) -> Result<Vec<T>> {
        let mut out = vec![item(self)?];
        while self.take_comma() {
            out.push(item(self)?);
        }
        self.expect(&Token::Close)?;
        Ok(out)
    }

    /// `<empty set>` or a parenthesised list of `item`s; empty for the
    /// former.
    ///
    /// # Errors
    ///
    /// As [`Cursor::items`].
    pub fn list<T>(&mut self, item: impl FnMut(&mut Cursor) -> Result<T>) -> Result<Vec<T>> {
        if self.opens()? { self.items(item) } else { Ok(Vec::new()) }
    }

    /// A `<number>`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 for another token.
    pub fn number(&mut self) -> Result<f64> {
        let Token::Number(value, _) = self.token()? else {
            return Err(invalid("expected a number"));
        };
        Ok(value)
    }

    /// A `<signed integer>` in `0..=limit`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 for another token or a value out of range.
    pub fn integer(&mut self, low: i64, limit: i64) -> Result<i64> {
        let token = self.token()?;
        let value = match token {
            Token::Number(value, true) => format!("{value}").parse::<i64>().ok(),
            Token::Number(_, false) | Token::Word(_) | Token::Quoted(_) | Token::Open | Token::Close | Token::Comma => None,
        };
        value.filter(|v| (low..=limit).contains(v)).ok_or_else(|| invalid(&format!("expected an integer from {low} to {limit}")))
    }

    /// `<length text>` and its kin: `<empty set>` (NaN) or a number.
    ///
    /// # Errors
    ///
    /// As [`Cursor::number`].
    pub fn length(&mut self) -> Result<f64> {
        if self.take_word("EMPTY") { Ok(f64::NAN) } else { self.number() }
    }

    /// An optional `<z m>`: its dimensions, if given.
    fn marker(&mut self) -> Option<Dims> {
        let dims = match self.peek() {
            Some(Token::Word(w)) => match w.as_str() {
                "ZM" => Some(Dims::Xyzm),
                "Z" => Some(Dims::Xyz),
                "M" => Some(Dims::Xym),
                _ => None,
            },
            Some(Token::Number(..) | Token::Quoted(_) | Token::Open | Token::Close | Token::Comma) | None => None,
        };
        if dims.is_some() {
            self.at = self.at.saturating_add(1);
        }
        dims
    }

    /// A type keyword and its optional `<z m>`. The outermost sets the
    /// value's dimensions; a nested one must repeat them (rule ai).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF22 for an unknown keyword, 2FF25 for a marker that
    /// differs from the outermost one.
    pub fn keyword(&mut self, outermost: bool) -> Result<Kind> {
        let Token::Word(w) = self.token()? else {
            return Err(invalid("expected a geometry type"));
        };
        let kind = Kind::of_keyword(&w).ok_or_else(|| invalid(&format!("{w} names no geometry type")))?;
        let marker = self.marker();
        if outermost {
            self.dims = marker.unwrap_or(Dims::Xy);
        } else if marker.is_some_and(|m| m != self.dims) {
            return Err(mixed("a nested <z m> differs from the outermost one"));
        }
        Ok(kind)
    }

    /// `<just z>` where rule ak wants it: present exactly when the value
    /// has z.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF25 when it is missing or extra.
    pub fn just_z(&mut self) -> Result<()> {
        if self.take_word("Z") == self.dims.has_z() { Ok(()) } else { Err(mixed("<just z> must be given exactly when the value has z")) }
    }

    /// A `<point>` of `dims` (rule ao).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF25 for another count of coordinates.
    pub fn point(&mut self, dims: Dims) -> Result<Coord> {
        let mut values = Vec::with_capacity(dims.stride());
        while let Some(Token::Number(value, _)) = self.peek() {
            values.push(*value);
            self.at = self.at.saturating_add(1);
        }
        if values.len() != dims.stride() {
            return Err(mixed(&format!("a point has {} coordinates, not {}", values.len(), dims.stride())));
        }
        let at = |i: usize| values.get(i).copied().unwrap_or_default();
        let (z, m) = match dims {
            Dims::Xy => (0.0_f64, 0.0_f64),
            Dims::Xyz => (at(2), 0.0_f64),
            Dims::Xym => (0.0_f64, at(2)),
            Dims::Xyzm => (at(2), at(3)),
        };
        Ok(Coord { x: at(0), y: at(1), z, m })
    }

    /// `<point text>`: `<empty set>` or a parenthesised point.
    ///
    /// # Errors
    ///
    /// As [`Cursor::point`].
    pub fn point_text(&mut self, dims: Dims) -> Result<Option<Coord>> {
        if !self.opens()? {
            return Ok(None);
        }
        let point = self.point(dims)?;
        self.expect(&Token::Close)?;
        Ok(Some(point))
    }

    /// `<linestring text>`: `<empty set>` or a list of points.
    ///
    /// # Errors
    ///
    /// As [`Cursor::point`].
    pub fn points(&mut self) -> Result<Ring> {
        let dims = self.dims;
        self.list(|c| c.point(dims))
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexer::tokens;
    use super::*;

    fn cursor(text: &str) -> Cursor {
        Cursor::new(tokens(text).unwrap())
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67 ai ao] The
    /// outermost marker sets the dimensions; nested ones must agree, and
    /// points carry exactly the coordinates the dimensions name.
    #[test]
    fn markers_and_points() {
        let mut c = cursor("LINESTRING M (1 2 3) POINT Z");
        assert_eq!(c.keyword(true).unwrap(), Kind::LineString);
        assert_eq!(c.dims, Dims::Xym);
        assert_eq!(c.points().unwrap(), vec![Coord::xyzm(1.0, 2.0, 0.0, 3.0)]);
        assert_eq!(c.keyword(false).unwrap_err().sqlstate().as_str(), "2FF25");
        let mut short = cursor("(1 2)");
        short.dims = Dims::Xyz;
        assert_eq!(short.points().unwrap_err().sqlstate().as_str(), "2FF25");
        assert_eq!(cursor("SQUARE").keyword(true).unwrap_err().sqlstate().as_str(), "2FF22");
        assert_eq!(cursor("Z").just_z().unwrap_err().sqlstate().as_str(), "2FF25");
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] Integers
    /// are range checked and lengths may be empty.
    #[test]
    fn integers_and_lengths() {
        assert_eq!(cursor("7").integer(0, 255).unwrap(), 7);
        assert!(cursor("7.5").integer(0, 255).is_err());
        assert!(cursor("300").integer(0, 255).is_err());
        assert!(cursor("EMPTY").length().unwrap().is_nan());
        assert_eq!(cursor("(1 2").points().unwrap_err().sqlstate().as_str(), "2FF22");
        assert!(cursor("").token().is_err());
    }
}
