//! [ISO/IEC 9075-2:2023 §6.11] `<nested window function>` ::= `<nested row
//! number function>` | `<value_of expression at row>`. VALUE_OF and the
//! row markers, which SQL:2023 reserves and SQL-92 does not, are key words
//! only here: ROW_NUMBER is the nested function only of one marker between
//! its parentheses, VALUE_OF only where AT and a marker are among its
//! arguments, so a column or a function may have any of those names.

use crate::ast::{NestedWindow, RowMarker, ValueExpression};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// The functions of this module, by name.
const ROW_NUMBER: &str = "ROW_NUMBER";
const VALUE_OF: &str = "VALUE_OF";
/// The tokens of a `<nested row number function>`: ROW_NUMBER, "(", its
/// marker and ")".
const ROW_NUMBER_TOKENS: u32 = 4;

/// The `<row marker>`s, by name.
const MARKERS: [(&str, RowMarker); 6] = [
    ("BEGIN_PARTITION", RowMarker::BeginPartition),
    ("BEGIN_FRAME", RowMarker::BeginFrame),
    ("CURRENT_ROW", RowMarker::CurrentRow),
    ("FRAME_ROW", RowMarker::FrameRow),
    ("END_FRAME", RowMarker::EndFrame),
    ("END_PARTITION", RowMarker::EndPartition),
];

/// A nested window function at hand, as its name and arguments say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::parser) enum Nesting {
    /// ROW_NUMBER ( `<row marker>` ).
    RowNumber(RowMarker),
    /// VALUE_OF ( ... AT `<row marker>` ... ).
    ValueOf,
}

impl Parser {
    /// A nested window function, if one is at hand: ROW_NUMBER "(" and one
    /// marker ")", or VALUE_OF "(" with AT and a marker at the depth of its
    /// arguments. Else none, so a function of either name stays callable.
    pub(in crate::parser) fn at_nested_window(&self) -> Option<Nesting> {
        let Tok::Identifier(name) = self.peek() else { return None };
        if !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return None;
        }
        if name == ROW_NUMBER {
            let marker = self.marker_at(2)?;
            return matches!(self.peek_at(3), Tok::Delimiter(delim::RIGHT_PAREN)).then_some(Nesting::RowNumber(marker));
        }
        (name == VALUE_OF && self.at_marked(2)).then_some(Nesting::ValueOf)
    }

    /// The row marker `at` places ahead, if one is.
    fn marker_at(&self, at: usize) -> Option<RowMarker> {
        let Tok::Identifier(word) = self.peek_at(at) else { return None };
        MARKERS.into_iter().find(|(m, _)| word == m).map(|(_, marker)| marker)
    }

    /// Whether AT and a row marker follow the token `from` places ahead,
    /// before the ")" that closes the arguments it begins, not in the
    /// parentheses of one of them. Each token is looked at once.
    fn at_marked(&self, from: usize) -> bool {
        let (mut depth, mut at) = (0_usize, from);
        loop {
            match self.peek_at(at) {
                Tok::End => return false,
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => match depth.checked_sub(1) {
                    Some(d) => depth = d,
                    None => return false,
                },
                Tok::KeyWord(kw::AT) => {
                    if depth == 0 && self.marker_at(at.saturating_add(1)).is_some() {
                        return true;
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

    /// [ISO/IEC 9075-2:2023 §6.11] The nested window function `nesting` at
    /// hand, as `at_nested_window` saw it.
    pub(in crate::parser) fn nested_window(&mut self, nesting: Nesting) -> Result<ValueExpression> {
        let at = self.here();
        let nested = match nesting {
            Nesting::RowNumber(marker) => {
                // Its name, "(", the marker and ")", as seen.
                for _ in 0_u32..ROW_NUMBER_TOKENS {
                    self.advance();
                }
                NestedWindow { marker, delta: None, value: None, at }
            }
            Nesting::ValueOf => self.value_of(at)?,
        };
        Ok(ValueExpression::NestedWindow(Box::new(nested)))
    }

    /// `<value_of expression at row>` ::= VALUE_OF ( `<value expression>`
    /// AT `<row marker expression>` \[ , `<value_of default value>` \] ),
    /// begun at `at`.
    fn value_of(&mut self, at: usize) -> Result<NestedWindow> {
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let value = Box::new(self.value_expression()?);
        self.expect_kw(kw::AT)?;
        let Some(marker) = self.marker_at(0) else { return Err(self.syntax("expected a <row marker>".to_owned())) };
        self.advance();
        // <row marker delta> ::= <plus sign> <row marker offset> | <minus
        // sign> <row marker offset>
        let delta = if self.accept_delim(delim::PLUS) {
            Some((false, Box::new(self.number_of_rows()?)))
        } else if self.accept_delim(delim::MINUS) {
            Some((true, Box::new(self.number_of_rows()?)))
        } else {
            None
        };
        let default = if self.accept_delim(delim::COMMA) { Some(Box::new(self.value_expression()?)) } else { None };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(NestedWindow { marker, delta, value: Some((value, default)), at })
    }
}
