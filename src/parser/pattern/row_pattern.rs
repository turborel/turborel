//! [ISO/IEC 9075-2:2023 §7.9] `<row pattern>`: alternation, concatenation
//! by juxtaposition, the primaries and the quantifiers, each read as `{ n,
//! m }` or `{ n, }` with its reluctance \[SR19\].
//!
//! The question mark is a quantifier here, never a `<dynamic parameter
//! specification>`, and the asterisk never multiplication. As the
//! standard's own lexical rules have it [§5.2], `??(` and `??)` are the
//! bracket trigraphs, so a reluctant `?` before a parenthesis is written
//! with a separator between them: `(A?? )`.

use crate::ast::{Quantified, RowPattern};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim};
use crate::parser::Parser;

/// PERMUTE, a word SQL-92 does not reserve, before "(" a `<row pattern
/// permute>`.
const PERMUTE: &str = "PERMUTE";

/// The bounds of a `<row pattern quantifier>`, and whether a `?` after it
/// makes it reluctant.
struct Bounds {
    /// The least number of repetitions.
    min: u32,
    /// The most, none if unbounded.
    max: Option<u32>,
    /// Whether the BNF lets a `?` follow, as of every quantifier but `{ n }`.
    may_be_reluctant: bool,
}

impl Parser {
    /// `<row pattern>` ::= `<row pattern term>` | `<row pattern>` |
    /// `<row pattern term>`, so concatenation binds tighter than alternation.
    /// Each nested pattern counts one level toward `MAX_NESTING_DEPTH`.
    pub(in crate::parser) fn row_pattern(&mut self) -> Result<RowPattern> {
        self.nested(|p| {
            let mut terms = vec![p.row_pattern_term()?];
            while p.accept_delim(delim::VERTICAL_BAR) {
                terms.push(p.row_pattern_term()?);
            }
            Ok(one_or(terms, RowPattern::Alternation))
        })
    }

    /// `<row pattern term>` ::= `<row pattern factor>`...
    fn row_pattern_term(&mut self) -> Result<RowPattern> {
        let mut factors = vec![self.row_pattern_factor()?];
        while self.at_row_pattern_primary() {
            factors.push(self.row_pattern_factor()?);
        }
        Ok(one_or(factors, RowPattern::Concat))
    }

    /// Whether a `<row pattern primary>` begins here.
    fn at_row_pattern_primary(&self) -> bool {
        match self.peek() {
            Tok::Identifier(_) | Tok::Introducer(_) => true,
            Tok::Delimiter(d) => [delim::DOLLAR_SIGN, delim::CIRCUMFLEX, delim::LEFT_PAREN, delim::LEFT_BRACE_MINUS].contains(d),
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::End => false,
        }
    }

    /// `<row pattern factor>` ::= `<row pattern primary>` \[`<row pattern
    /// quantifier>`\].
    fn row_pattern_factor(&mut self) -> Result<RowPattern> {
        let pattern = self.row_pattern_primary()?;
        let at = self.here();
        let Some(bounds) = self.row_pattern_quantifier()? else { return Ok(pattern) };
        let reluctant = if bounds.may_be_reluctant { self.accept_delim(delim::QUESTION) } else { false };
        // [SR20] Of { n, m }, and so of { n } and { , m } [SR19h, SR19i],
        // n ≤ m and m > 0.
        if let Some(max) = bounds.max
            && (bounds.min > max || max == 0)
        {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                format!(
                    "[ISO/IEC 9075-2:2023 §7.9 SR20] a quantifier's least repetitions, {}, are at most its most, {max}, which are more than 0",
                    bounds.min
                ),
            ));
        }
        Ok(RowPattern::Quantified(Box::new(Quantified { pattern, min: bounds.min, max: bounds.max, reluctant, at })))
    }

    /// The bounds of the `<row pattern quantifier>` next, its reluctant `?`
    /// not taken \[SR19\]: `*` { 0, }, `+` { 1, }, `?` { 0, 1 }, and
    /// braces; none if no quantifier is next.
    fn row_pattern_quantifier(&mut self) -> Result<Option<Bounds>> {
        let (min, max) = if self.accept_delim(delim::ASTERISK) {
            (0, None)
        } else if self.accept_delim(delim::PLUS) {
            (1, None)
        } else if self.accept_delim(delim::QUESTION) {
            (0, Some(1))
        } else if self.accept_delim(delim::LEFT_BRACE) {
            return self.braced_bounds().map(Some);
        } else {
            return Ok(None);
        };
        Ok(Some(Bounds { min, max, may_be_reluctant: true }))
    }

    /// `{` \[`<unsigned integer>`\] , \[`<unsigned integer>`\] `}`, the
    /// least 0 if not written \[SR19e to SR19h\], or `{` `<unsigned
    /// integer>` `}`, both bounds the one written \[SR19i\]; `{` taken.
    fn braced_bounds(&mut self) -> Result<Bounds> {
        let min = if matches!(self.peek(), Tok::ExactNumeric(_)) { Some(self.unsigned_integer()?) } else { None };
        if !self.accept_delim(delim::COMMA) {
            let Some(n) = min else { return Err(self.syntax("expected an <unsigned integer> or \",\" in a <row pattern quantifier>")) };
            self.expect_delim(delim::RIGHT_BRACE)?;
            return Ok(Bounds { min: n, max: Some(n), may_be_reluctant: false });
        }
        let max = if matches!(self.peek(), Tok::ExactNumeric(_)) { Some(self.unsigned_integer()?) } else { None };
        self.expect_delim(delim::RIGHT_BRACE)?;
        Ok(Bounds { min: min.unwrap_or(0), max, may_be_reluctant: true })
    }

    /// `<row pattern primary>` ::= `<row pattern primary variable name>` |
    /// `$` | `^` | ( \[`<row pattern>`\] ) | `{-` `<row pattern>` `-}` |
    /// `<row pattern permute>`; a pattern in parentheses is the pattern.
    fn row_pattern_primary(&mut self) -> Result<RowPattern> {
        let at = self.here();
        if self.accept_delim(delim::DOLLAR_SIGN) {
            return Ok(RowPattern::End);
        }
        if self.accept_delim(delim::CIRCUMFLEX) {
            return Ok(RowPattern::Start);
        }
        if self.accept_delim(delim::LEFT_PAREN) {
            if self.accept_delim(delim::RIGHT_PAREN) {
                return Ok(RowPattern::Empty);
            }
            let inner = self.row_pattern()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(inner);
        }
        if self.accept_delim(delim::LEFT_BRACE_MINUS) {
            let inner = self.row_pattern()?;
            self.expect_delim(delim::RIGHT_MINUS_BRACE)?;
            return Ok(RowPattern::Excluded(Box::new(inner)));
        }
        if self.is_word(PERMUTE) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return self.row_pattern_permute();
        }
        if !matches!(self.peek(), Tok::Identifier(_) | Tok::Introducer(_)) {
            return Err(self.syntax("expected a <row pattern primary>"));
        }
        Ok(RowPattern::Variable(self.identifier()?, at))
    }

    /// `<row pattern permute>` ::= PERMUTE ( `<row pattern>` \[{, `<row
    /// pattern>`}...\] ), kept as written \[SR21\].
    fn row_pattern_permute(&mut self) -> Result<RowPattern> {
        self.advance();
        self.advance();
        let mut patterns = vec![self.row_pattern()?];
        while self.accept_delim(delim::COMMA) {
            patterns.push(self.row_pattern()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(RowPattern::Permute(patterns))
    }
}

/// The one pattern of `list`, or `many` of them all if there are more.
fn one_or(list: Vec<RowPattern>, many: fn(Vec<RowPattern>) -> RowPattern) -> RowPattern {
    match <[RowPattern; 1]>::try_from(list) {
        Ok([only]) => only,
        Err(all) => many(all),
    }
}

// Unit tests: each primary and quantifier, and how they combine (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
