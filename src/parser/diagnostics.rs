//! `<get diagnostics statement>` [ISO/IEC 9075:1992 §18.1]. Split from
//! parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{ConditionItem, GetDiagnostics, ParameterSpecification, StatementItem, ValueSpecification};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>` ::= GET
    /// DIAGNOSTICS {`<statement information>` | `<condition information>`}
    pub(super) fn get_diagnostics(&mut self) -> Result<GetDiagnostics> {
        self.expect_kw(kw::GET)?;
        self.expect_kw(kw::DIAGNOSTICS)?;
        if !self.accept_kw(kw::EXCEPTION) {
            let items = self.information_items(StatementItem::named, "<statement information item name>")?;
            return Ok(GetDiagnostics::Statement(items));
        }
        // [§6.2] <simple value specification> ::= <parameter name> | <literal>
        let number = if self.at_parameter() {
            ValueSpecification::Parameter(self.simple_target_specification()?)
        } else {
            ValueSpecification::Literal(self.literal()?)
        };
        let items = self.information_items(ConditionItem::named, "<condition information item name>")?;
        Ok(GetDiagnostics::Condition { number, items })
    }

    /// `<simple target specification>` = `item`, \[{, ...}...\], each item
    /// a name `named` knows, a `what`.
    fn information_items<T>(&mut self, named: impl Fn(&str) -> Option<T>, what: &str) -> Result<Vec<(ParameterSpecification, T)>> {
        let mut items = Vec::new();
        loop {
            let target = self.simple_target_specification()?;
            self.expect_delim(delim::EQUALS)?;
            let item = if let Tok::Identifier(name) = self.peek() { named(name) } else { None };
            let Some(item) = item else { return Err(self.syntax(format!("expected a {what}"))) };
            self.advance();
            items.push((target, item));
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }

    /// [ISO/IEC 9075:1992 §6.2] `<simple target specification>` ::=
    /// `<parameter name>`, of a parameter declared: with no indicator.
    fn simple_target_specification(&mut self) -> Result<ParameterSpecification> {
        let at = self.here();
        let name = self.identifier()?;
        if !self.params.contains(&name) {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[X3.135-1986 §7.3 SR3] parameter {name} is not declared")));
        }
        Ok(ParameterSpecification { name, indicator: None, at })
    }
}
