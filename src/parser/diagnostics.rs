//! `<get diagnostics statement>` [ISO/IEC 9075:1992 §18.1], with
//! ISO/IEC 9075-2:2023 §23.1's `<all information>` (Feature F123) and
//! CONDITION.

use super::Parser;
use crate::ast::{AllQualifier, ConditionItem, GetDiagnostics, ParameterSpecification, StatementItem, TargetKind, ValueSpecification};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §23.1] CONDITION, which begins a `<condition
/// information>` in EXCEPTION's place and qualifies ALL. It is no key word
/// of SQL-92, so it is read as an identifier, a key word only here.
const CONDITION: &str = "CONDITION";
/// [ISO/IEC 9075-2:2023 §23.1] STATEMENT, an `<all qualifier>`, read as
/// CONDITION is.
pub(super) const STATEMENT: &str = "STATEMENT";

impl Parser {
    /// [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>` ::= GET
    /// DIAGNOSTICS {`<statement information>` | `<condition information>`};
    /// [ISO/IEC 9075-2:2023 §23.1] | `<all information>`.
    pub(super) fn get_diagnostics(&mut self) -> Result<GetDiagnostics> {
        self.expect_kw(kw::GET)?;
        self.expect_kw(kw::DIAGNOSTICS)?;
        // CONDITION followed by = is a target, a parameter of that name.
        let condition = self.is_word(CONDITION) && *self.peek_at(1) != Tok::Delimiter(delim::EQUALS);
        if self.accept_kw(kw::EXCEPTION) || condition {
            if condition {
                self.advance();
            }
            let number = self.condition_number()?;
            let items = self.information_items(ConditionItem::named, "<condition information item name>")?;
            return Ok(GetDiagnostics::Condition { number, items });
        }
        let target = self.simple_target_specification()?;
        self.expect_delim(delim::EQUALS)?;
        if self.accept_kw(kw::ALL) {
            let qualifier = self.all_qualifier()?;
            return Ok(GetDiagnostics::All { target, qualifier });
        }
        let first = self.information_item(StatementItem::named, "<statement information item name>")?;
        let mut items = vec![(target, first)];
        if self.accept_delim(delim::COMMA) {
            items.extend(self.information_items(StatementItem::named, "<statement information item name>")?);
        }
        Ok(GetDiagnostics::Statement(items))
    }

    /// [ISO/IEC 9075-2:2023 §23.1] `<all qualifier>` ::= STATEMENT |
    /// CONDITION \[`<condition number>`\], or none.
    fn all_qualifier(&mut self) -> Result<AllQualifier> {
        if self.accept_word(STATEMENT) {
            return Ok(AllQualifier::Statement);
        }
        if !self.accept_word(CONDITION) {
            return Ok(AllQualifier::Everything);
        }
        if *self.peek() == Tok::End {
            return Ok(AllQualifier::Condition(None));
        }
        Ok(AllQualifier::Condition(Some(self.condition_number()?)))
    }

    /// `<condition number>` ::= `<simple value specification>`, which
    /// [ISO/IEC 9075:1992 §6.2] is `<parameter name>` | `<literal>`.
    fn condition_number(&mut self) -> Result<ValueSpecification> {
        if self.at_parameter() {
            return Ok(ValueSpecification::Parameter(self.simple_target_specification()?));
        }
        Ok(ValueSpecification::Literal(self.literal()?))
    }

    /// `<simple target specification>` = `item`, \[{, ...}...\], each item
    /// a name `named` knows, a `what`.
    fn information_items<T>(&mut self, named: impl Fn(&str) -> Option<T>, what: &str) -> Result<Vec<(ParameterSpecification, T)>> {
        let mut items = Vec::new();
        loop {
            let target = self.simple_target_specification()?;
            self.expect_delim(delim::EQUALS)?;
            items.push((target, self.information_item(&named, what)?));
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }

    /// An item's name, one `named` knows, a `what`.
    fn information_item<T>(&mut self, named: impl Fn(&str) -> Option<T>, what: &str) -> Result<T> {
        let item = if let Tok::Identifier(name) = self.peek() { named(name) } else { None };
        let Some(item) = item else { return Err(self.syntax(format!("expected a {what}"))) };
        self.advance();
        Ok(item)
    }

    /// [ISO/IEC 9075:1992 §6.2] `<simple target specification>` ::=
    /// `<parameter name>`, of a parameter declared: with no indicator.
    fn simple_target_specification(&mut self) -> Result<ParameterSpecification> {
        // [ISO/IEC 9075-2:2023 §6.4 SR7] of a <triggered action>.
        if self.trigger.is_some() {
            return self.transition_target();
        }
        // [ISO/IEC 9075-2:2023 §6.8 SR1] of an <SQL routine body>.
        if self.routine.is_some() {
            return self.routine_target();
        }
        let at = self.here();
        let name = self.identifier()?;
        if !self.params.contains(&name) {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[X3.135-1986 §7.3 SR3] parameter {name} is not declared")));
        }
        Ok(ParameterSpecification::of(name, TargetKind::Parameter, at))
    }
}

// Unit tests: each form of the statement parsed, and the forms refused
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
