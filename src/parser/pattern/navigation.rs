//! The scalar expressions only row pattern matching reads: the `<row
//! pattern navigation operation>`s [ISO/IEC 9075-2:2023 §6.27], `<match
//! number function>` [§6.31], `<classifier function>` [§6.33], `<running
//! or final>` before an aggregate [§6.9], `<row pattern count function>`
//! [§10.9], and `<window row pattern measure>` [§6.10].
//!
//! NEXT, FIRST and LAST are SQL-92's reserved words, PREV, RUNNING, FINAL,
//! CLASSIFIER and MATCH_NUMBER not. Each is read as this syntax only
//! before "(", or before FIRST or LAST and "(", or before an aggregate's
//! name and "(", so NEXT VALUE FOR, NULLS FIRST, FETCH FIRST and a column
//! of any of those names read as they did. A name before OVER and "(" or a
//! window name is a `<window row pattern measure>`.

use crate::ast::{Logical, Navigation, Physical, Semantics, SetArg, SetFunctionSpecification, ValueExpression, WindowFunction, WindowKind};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;
use crate::parser::window::OVER;

use super::is_named;

/// PREV, a `<prev or next>`.
const PREV: &str = "PREV";
/// RUNNING, a `<running or final>`.
const RUNNING: &str = "RUNNING";
/// FINAL, a `<running or final>`.
const FINAL: &str = "FINAL";
/// CLASSIFIER, before "(" the `<classifier function>`.
const CLASSIFIER: &str = "CLASSIFIER";
/// MATCH_NUMBER, before "(" the `<match number function>`.
const MATCH_NUMBER: &str = "MATCH_NUMBER";

/// Whether `t` is FIRST or LAST.
fn is_first_or_last(t: &Tok) -> bool {
    matches!(t, Tok::KeyWord(kw::FIRST | kw::LAST))
}

/// Whether `t` is "(".
fn is_left_paren(t: &Tok) -> bool {
    matches!(t, Tok::Delimiter(delim::LEFT_PAREN))
}

impl Parser {
    /// A `<primary>` that begins with an identifier and is one of this
    /// module's expressions, if one is next.
    pub(in crate::parser) fn row_pattern_primary_expression(&mut self) -> Result<Option<ValueExpression>> {
        let opens = is_left_paren(self.peek_at(1));
        if self.is_word(PREV) && opens {
            return self.row_pattern_navigation().map(Some);
        }
        if self.is_word(CLASSIFIER) && opens {
            return self.classifier_function().map(Some);
        }
        if self.is_word(MATCH_NUMBER) && opens {
            let at = self.here();
            self.advance();
            self.advance();
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(Some(ValueExpression::MatchNumber { at }));
        }
        if self.at_window_row_pattern_measure() {
            return self.window_row_pattern_measure().map(Some);
        }
        self.running_or_final()
    }

    /// A `<primary>` that begins with NEXT: a navigation before "(", else
    /// NEXT VALUE FOR [§6.14].
    pub(in crate::parser) fn next_primary(&mut self) -> Result<ValueExpression> {
        if is_left_paren(self.peek_at(1)) { self.row_pattern_navigation() } else { self.next_value_expression() }
    }

    /// A `<primary>` that begins with FIRST or LAST: a navigation before
    /// "(", else no primary at all.
    pub(in crate::parser) fn first_or_last_primary(&mut self) -> Result<ValueExpression> {
        if is_left_paren(self.peek_at(1)) { self.row_pattern_navigation() } else { Err(self.syntax("expected a <value expression>")) }
    }

    /// Whether a `<row pattern navigation: logical>` begins here:
    /// \[`<running or final>`\] `<first or last>` (.
    fn at_logical_navigation(&self) -> bool {
        if is_first_or_last(self.peek()) {
            return is_left_paren(self.peek_at(1));
        }
        (self.is_word(RUNNING) || self.is_word(FINAL)) && is_first_or_last(self.peek_at(1)) && is_left_paren(self.peek_at(2))
    }

    /// [ISO/IEC 9075-2:2023 §6.27] `<row pattern navigation operation>`:
    /// logical, physical, or compound, a physical one's argument logical.
    /// A logical step of no `<running or final>` is RUNNING \[SR5a\]; one
    /// alone has no physical step \[SR8\], and a physical one alone no
    /// logical step \[SR7\].
    fn row_pattern_navigation(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let next = if self.accept_word(PREV) {
            false
        } else if self.accept_kw(kw::NEXT) {
            true
        } else {
            let (expr, logical) = self.logical_navigation()?;
            return Ok(navigation(Navigation { expr, logical: Some(logical), physical: None, at }));
        };
        self.expect_delim(delim::LEFT_PAREN)?;
        let (expr, logical) = if self.at_logical_navigation() {
            let (expr, logical) = self.logical_navigation()?;
            (expr, Some(logical))
        } else {
            (self.value_expression()?, None)
        };
        let offset = self.navigation_offset()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(navigation(Navigation { expr, logical, physical: Some(Physical { next, offset }), at }))
    }

    /// \[`<running or final>`\] `<first or last>` ( `<value expression>`
    /// \[, `<logical offset>`\] ): the expression and the step.
    fn logical_navigation(&mut self) -> Result<(ValueExpression, Logical)> {
        let final_ = self.accept_word(FINAL);
        if !final_ {
            self.accept_word(RUNNING);
        }
        let last = self.accept_kw(kw::LAST);
        if !last {
            self.expect_kw(kw::FIRST)?;
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let expr = self.value_expression()?;
        let offset = self.navigation_offset()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok((expr, Logical { final_, last, offset }))
    }

    /// \[, `<logical offset>` | `<physical offset>`\], each a `<simple
    /// value specification>` or `<dynamic parameter specification>`; none
    /// if not written, the binder taking 0 or 1 \[SR5c ii, SR6b\].
    fn navigation_offset(&mut self) -> Result<Option<Box<ValueExpression>>> {
        if !self.accept_delim(delim::COMMA) {
            return Ok(None);
        }
        Ok(Some(Box::new(self.number_of_rows()?)))
    }

    /// [ISO/IEC 9075-2:2023 §6.33] `<classifier function>` ::= CLASSIFIER
    /// ( \[`<row pattern variable name>`\] ).
    fn classifier_function(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.advance();
        let variable = if self.accept_delim(delim::RIGHT_PAREN) {
            None
        } else {
            let name = self.identifier()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(name)
        };
        Ok(ValueExpression::Classifier { variable, at })
    }

    /// Whether a `<window row pattern measure>` is next, the name at hand
    /// followed by OVER and then "(" or a window name, as `windowed` reads
    /// OVER. Only a `<primary>` that begins with an identifier asks, so the
    /// token at hand is a name.
    fn at_window_row_pattern_measure(&self) -> bool {
        is_named(self.peek_at(1), OVER) && matches!(self.peek_at(2), Tok::Delimiter(delim::LEFT_PAREN) | Tok::Identifier(_))
    }

    /// [ISO/IEC 9075-2:2023 §6.10] `<window function>` ::= `<window row
    /// pattern measure>` OVER `<window name or specification>`, where
    /// `<window row pattern measure>` ::= `<measure name>`.
    fn window_row_pattern_measure(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let name = self.identifier()?;
        self.advance();
        let over = self.window_over()?;
        Ok(ValueExpression::Window(Box::new(WindowFunction { kind: WindowKind::Measure(name), over, at })))
    }

    /// [ISO/IEC 9075-2:2023 §6.9] `<running or final>` `<aggregate
    /// function>`, or before `<first or last>` a logical navigation, if
    /// RUNNING or FINAL is next before an aggregate's name and "(".
    fn running_or_final(&mut self) -> Result<Option<ValueExpression>> {
        let semantics = if self.is_word(RUNNING) {
            Semantics::Running
        } else if self.is_word(FINAL) {
            Semantics::Final
        } else {
            return Ok(None);
        };
        if self.at_logical_navigation() {
            return self.row_pattern_navigation().map(Some);
        }
        let named = matches!(
            self.peek_at(1),
            Tok::Identifier(_) | Tok::KeyWord(kw::COUNT | kw::AVG | kw::MAX | kw::MIN | kw::SUM | kw::ANY | kw::SOME)
        );
        if !(named && is_left_paren(self.peek_at(2))) {
            return Ok(None);
        }
        let at = self.here();
        self.advance();
        let ValueExpression::SetFunction(mut aggregate) = self.primary()? else { return Err(not_aggregate(at)) };
        if !is_aggregate_function(&aggregate) {
            return Err(not_aggregate(at));
        }
        aggregate.semantics = Some(semantics);
        Ok(Some(ValueExpression::SetFunction(aggregate)))
    }

    /// [ISO/IEC 9075-2:2023 §10.9] `<row pattern count function>` ::=
    /// COUNT ( `<row pattern variable name>` . * ), if the name, "." and
    /// "*" before ")" are next, COUNT and "(" taken. The name and "." and
    /// "*" are taken.
    pub(in crate::parser) fn row_pattern_count_function(&mut self) -> Result<Option<SetArg>> {
        let Some(span) = self.variable_name_span() else { return Ok(None) };
        let starred = matches!(self.peek_at(span), Tok::Delimiter(delim::PERIOD))
            && matches!(self.peek_at(span.saturating_add(1)), Tok::Delimiter(delim::ASTERISK))
            && matches!(self.peek_at(span.saturating_add(2)), Tok::Delimiter(delim::RIGHT_PAREN));
        if !starred {
            return Ok(None);
        }
        let at = self.here();
        let name = self.identifier()?;
        self.advance();
        self.advance();
        Ok(Some(SetArg::Variable(name, at)))
    }

    /// How many tokens the `<identifier>` next spans, if one is: 1, or 2 if
    /// an `<introducer>` begins it [ISO/IEC 9075:1992 §5.4].
    fn variable_name_span(&self) -> Option<usize> {
        match self.peek() {
            Tok::Identifier(_) => Some(1),
            Tok::Introducer(_) | Tok::LargeObjectLength(_) => matches!(self.peek_at(1), Tok::Identifier(_)).then_some(2),
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        }
    }
}

/// `n` as a value expression.
fn navigation(n: Navigation) -> ValueExpression {
    ValueExpression::Navigation(Box::new(n))
}

/// Whether `f`, read after RUNNING or FINAL, is an `<aggregate function>`
/// and not a `<grouping operation>`. It has no `<running or final>` of its
/// own, since only a name and "(" follow the one read, and a second needs a
/// name between them.
fn is_aggregate_function(f: &SetFunctionSpecification) -> bool {
    !matches!(f.arg, SetArg::Grouping(_))
}

/// [ISO/IEC 9075-2:2023 §6.9] `<running or final>` written at `at` before
/// something other than one `<aggregate function>`.
fn not_aggregate(at: usize) -> SqlError {
    SqlError::at(Code::Syntax, at, "[ISO/IEC 9075-2:2023 §6.9] RUNNING or FINAL is followed by an <aggregate function>")
}

// Unit tests: each expression, and the syntax of those words it leaves as
// it was (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
