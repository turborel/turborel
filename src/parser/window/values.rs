//! [ISO/IEC 9075-2:2023 §6.10] The `<window function type>`s of arguments:
//! `<ntile function>`, `<lead or lag function>`, `<first or last value
//! function>` and `<nth value function>`. Their names, which SQL:2023
//! reserves and SQL-92 does not, and RESPECT and IGNORE, which neither
//! reserves, are key words only where OVER follows the function, so a
//! column or a function may have any of those names. FIRST, LAST and FROM
//! SQL-92 reserves.

use super::OVER;
use crate::ast::{OffsetFunction, ValueExpression, ValueFunction, Which, WindowFunction, WindowKind};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;
use crate::parser::constraints::NULLS;

/// The functions of this module, by name.
const NTILE: &str = "NTILE";
const LEAD: &str = "LEAD";
const LAG: &str = "LAG";
const FIRST_VALUE: &str = "FIRST_VALUE";
const LAST_VALUE: &str = "LAST_VALUE";
const NTH_VALUE: &str = "NTH_VALUE";
/// The words of a `<window function null treatment>`.
const RESPECT: &str = "RESPECT";
const IGNORE: &str = "IGNORE";

/// One of this module's functions, as its name says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::parser) enum Named {
    Ntile,
    Lead,
    Lag,
    FirstValue,
    LastValue,
    NthValue,
}

/// The functions, by name.
const NAMED: [(&str, Named); 6] = [
    (NTILE, Named::Ntile),
    (LEAD, Named::Lead),
    (LAG, Named::Lag),
    (FIRST_VALUE, Named::FirstValue),
    (LAST_VALUE, Named::LastValue),
    (NTH_VALUE, Named::NthValue),
];

impl Parser {
    /// One of this module's functions, if one is at hand: its name and "(",
    /// and after the matching ")", OVER before "(" or a window name,
    /// perhaps after FROM FIRST or FROM LAST and a null treatment. Else
    /// none, so a function of that name stays callable.
    pub(in crate::parser) fn at_value_function(&self) -> Option<Named> {
        let Tok::Identifier(name) = self.peek() else { return None };
        let (_, named) = NAMED.into_iter().find(|(n, _)| name == n)?;
        if !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return None;
        }
        let mut at = self.closed(1)?;
        if matches!(self.peek_at(at), Tok::KeyWord(kw::FROM))
            && matches!(self.peek_at(at.saturating_add(1)), Tok::KeyWord(kw::FIRST | kw::LAST))
        {
            at = at.saturating_add(2);
        }
        if self.is_word_at(at, RESPECT) || self.is_word_at(at, IGNORE) {
            if !self.is_word_at(at.saturating_add(1), NULLS) {
                return None;
            }
            at = at.saturating_add(2);
        }
        let over = self.is_word_at(at, OVER)
            && matches!(self.peek_at(at.saturating_add(1)), Tok::Delimiter(delim::LEFT_PAREN) | Tok::Identifier(_));
        over.then_some(named)
    }

    /// Whether the token `at` places ahead is the `<non-reserved word>`
    /// `word`.
    fn is_word_at(&self, at: usize, word: &str) -> bool {
        matches!(self.peek_at(at), Tok::Identifier(s) if s == word)
    }

    /// The place after the ")" that closes the "(" `open` places ahead;
    /// none if the statement ends first. Each token is looked at once.
    fn closed(&self, open: usize) -> Option<usize> {
        let (mut depth, mut at) = (0_usize, open);
        loop {
            match self.peek_at(at) {
                Tok::End => return None,
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(at.saturating_add(1));
                    }
                }
                Tok::Delimiter(_)
                | Tok::Identifier(_)
                | Tok::KeyWord(_)
                | Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BinaryString(_)
                | Tok::Introducer(_) => {}
            }
            at = at.saturating_add(1);
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.10] The `<window function>` of the function
    /// `named` at hand, as `at_value_function` saw it.
    pub(in crate::parser) fn value_function(&mut self, named: Named) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let kind = match named {
            Named::Ntile => {
                let tiles = self.number_of_rows()?;
                self.expect_delim(delim::RIGHT_PAREN)?;
                WindowKind::Ntile(Box::new(tiles))
            }
            Named::Lead | Named::Lag => WindowKind::Offset(Box::new(self.offset_function(named == Named::Lead)?)),
            Named::FirstValue | Named::LastValue | Named::NthValue => WindowKind::Value(Box::new(self.nth_function(named)?)),
        };
        self.expect_word(OVER)?;
        let over = self.window_over()?;
        Ok(ValueExpression::Window(Box::new(WindowFunction { kind, over, at })))
    }

    /// `<number of tiles>` or `<nth row>` ::= `<simple value
    /// specification>` | `<dynamic parameter specification>`.
    pub(super) fn number_of_rows(&mut self) -> Result<ValueExpression> {
        let spec = if self.is_delim(delim::QUESTION) { self.value_specification()? } else { self.simple_value_specification()? };
        Ok(ValueExpression::Value(spec))
    }

    /// The rest of a `<lead or lag function>`, its name and "(" taken: the
    /// extent, the `<offset>` and `<default expression>` if written, ")",
    /// and the null treatment if written.
    fn offset_function(&mut self, lead: bool) -> Result<OffsetFunction> {
        let extent = self.value_expression()?;
        let (mut offset, mut default) = (None, None);
        if self.accept_delim(delim::COMMA) {
            offset = Some(self.unsigned_integer()?);
            if self.accept_delim(delim::COMMA) {
                default = Some(self.value_expression()?);
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let ignore_nulls = self.null_treatment()?;
        Ok(OffsetFunction { lead, extent, offset, default, ignore_nulls })
    }

    /// The rest of a `<first or last value function>` or an `<nth value
    /// function>` `named`, its name and "(" taken.
    fn nth_function(&mut self, named: Named) -> Result<ValueFunction> {
        let arg = self.value_expression()?;
        let which = if named == Named::NthValue {
            self.expect_delim(delim::COMMA)?;
            let row = Box::new(self.number_of_rows()?);
            self.expect_delim(delim::RIGHT_PAREN)?;
            // <from first or last> ::= FROM FIRST | FROM LAST
            let from_last = if self.accept_kw(kw::FROM) {
                if self.accept_kw(kw::FIRST) {
                    false
                } else {
                    self.expect_kw(kw::LAST)?;
                    true
                }
            } else {
                false
            };
            Which::Nth { row, from_last }
        } else {
            self.expect_delim(delim::RIGHT_PAREN)?;
            if named == Named::FirstValue { Which::First } else { Which::Last }
        };
        let ignore_nulls = self.null_treatment()?;
        Ok(ValueFunction { which, arg, ignore_nulls })
    }

    /// `<window function null treatment>` ::= RESPECT NULLS | IGNORE
    /// NULLS, if written: whether IGNORE NULLS.
    fn null_treatment(&mut self) -> Result<bool> {
        let ignore = if self.accept_word(IGNORE) {
            true
        } else if self.accept_word(RESPECT) {
            false
        } else {
            return Ok(false);
        };
        self.expect_word(NULLS)?;
        Ok(ignore)
    }
}
