//! [ISO/IEC 9075-4:2006 draft §17.2, §17.3, §8.2] `<signal statement>`,
//! `<resignal statement>` and `<sqlstate value>`.

use super::{RESIGNAL, SIGNAL, rule_at};
use crate::ast::{ConditionItem, SignalStatement, SignalValue, ValueExpression};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// [§17.2 SR2] The `<condition information item name>`s a `<signal
/// information item>` may set.
const SETTABLE: [ConditionItem; 11] = [
    ConditionItem::ClassOrigin,
    ConditionItem::SubclassOrigin,
    ConditionItem::ConstraintCatalog,
    ConditionItem::ConstraintSchema,
    ConditionItem::ConstraintName,
    ConditionItem::CatalogName,
    ConditionItem::SchemaName,
    ConditionItem::TableName,
    ConditionItem::ColumnName,
    ConditionItem::CursorName,
    ConditionItem::MessageText,
];

impl Parser {
    /// [§17.2] `<signal statement>` ::= SIGNAL `<signal value>` \[`<set
    /// signal information>`\]; [§17.3] `<resignal statement>` ::= RESIGNAL
    /// \[`<signal value>`\] \[`<set signal information>`\].
    pub(super) fn signal_statement(&mut self) -> Result<SignalStatement> {
        let at = self.here();
        let resignal = self.accept_word(RESIGNAL);
        if !resignal {
            self.expect_word(SIGNAL)?;
        }
        let value = if self.is_kw(kw::SQLSTATE) {
            let (state, state_at) = self.sqlstate_value()?;
            Some(SignalValue::Sqlstate(state, state_at))
        } else if matches!(self.peek(), Tok::Identifier(_)) {
            Some(SignalValue::Name(self.signalled_condition()?))
        } else {
            None
        };
        if value.is_none() && !resignal {
            return Err(self.syntax("expected a <signal value> [ISO/IEC 9075-4:2006 draft §17.2]"));
        }
        let items = if self.accept_kw(kw::SET) { self.signal_items()? } else { Vec::new() };
        Ok(SignalStatement { resignal, value, items, at })
    }

    /// A `<condition name>` of a `<signal value>`: of a condition a
    /// statement it is in declares [§17.2 SR1a ii, §17.3 SR2a ii].
    fn signalled_condition(&mut self) -> Result<crate::ast::Label> {
        let name = self.label()?;
        if self.blocks.iter().any(|b| b.conditions.contains(&name.name)) {
            return Ok(name);
        }
        let msg =
            format!("[ISO/IEC 9075-4:2006 draft §17.2 SR1a ii] no compound statement this is in declares the condition {}", name.name);
        Err(rule_at(name.at, msg))
    }

    /// `<signal information item list>`: each `<condition information item
    /// name>` = `<simple value specification>`, of the items SR2 allows,
    /// each once.
    fn signal_items(&mut self) -> Result<Vec<(ConditionItem, ValueExpression)>> {
        let mut items: Vec<(ConditionItem, ValueExpression)> = Vec::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            let item = ConditionItem::named(&name).filter(|i| SETTABLE.contains(i)).ok_or_else(|| {
                rule_at(at, format!("[ISO/IEC 9075-4:2006 draft §17.2 SR2] SIGNAL and RESIGNAL set no condition information item {name}"))
            })?;
            if items.iter().any(|(i, _)| *i == item) {
                return Err(rule_at(at, format!("[ISO/IEC 9075-4:2006 draft §17.2 SR2] {name} is set twice")));
            }
            self.expect_delim(delim::EQUALS)?;
            let value_at = self.here();
            let value = self.value_expression()?;
            if !matches!(value, ValueExpression::Value(_) | ValueExpression::Column(_)) {
                let msg = "expected a <simple value specification> [ISO/IEC 9075-4:2006 draft §17.2]";
                return Err(SqlError::at(Code::Syntax, value_at, msg));
            }
            items.push((item, value));
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }

    /// [ISO/IEC 9075-4:2006 draft §8.2] `<sqlstate value>` ::= SQLSTATE
    /// \[VALUE\] `<character string literal>`: five digits or simple Latin
    /// upper case letters, not of class 00 [SR4, SR5]; and where its
    /// literal is.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is not one.
    pub(super) fn sqlstate_value(&mut self) -> Result<(String, usize)> {
        self.expect_kw(kw::SQLSTATE)?;
        self.accept_kw(kw::VALUE);
        let at = self.here();
        let state = self.character_string_literal("<sqlstate value>")?;
        if SqlState::parse(&state).is_none() {
            let msg = format!(
                "[ISO/IEC 9075-4:2006 draft §8.2 SR4, SR5] '{state}' is no SQLSTATE value: five digits or upper case letters, not of \
                 the class 00 of successful completion"
            );
            return Err(rule_at(at, msg));
        }
        Ok((state, at))
    }
}
