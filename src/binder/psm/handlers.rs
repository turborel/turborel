//! [ISO/IEC 9075-4:2006 draft §14.2] The handlers of a compound statement:
//! each condition value resolved to the condition it represents [SR2,
//! SR10, SR11], none twice in a list [SR5] nor in two handlers of the
//! compound statement [SR4], and a general one alone in its list [SR3].

use super::ProgramBinder;
use crate::ast::{ConditionValue, HandlerDeclaration};
use crate::binder::rule;
use crate::error::{At, Result, SqlState};
use crate::logical::{Handled, Handler};

impl ProgramBinder<'_> {
    /// The handlers `declared` of one compound statement, bound.
    pub(super) fn handlers(&mut self, declared: &[HandlerDeclaration]) -> Result<Vec<Handler>> {
        let mut handlers: Vec<Handler> = Vec::new();
        for d in declared {
            let mut conditions: Vec<Handled> = Vec::new();
            for (value, at) in &d.conditions {
                let handled = self.handled(value).or_at(*at)?;
                if conditions.contains(&handled) {
                    let msg = "[ISO/IEC 9075-4:2006 draft §14.2 SR5] a handler names one condition twice";
                    return Err(rule(msg).or_at(*at));
                }
                if handlers.iter().any(|h| h.conditions.contains(&handled)) {
                    let msg = "[ISO/IEC 9075-4:2006 draft §14.2 SR4] two handlers of one compound statement handle one condition";
                    return Err(rule(msg).or_at(*at));
                }
                conditions.push(handled);
            }
            if conditions.len() > 1 && conditions.iter().any(|c| !c.specific()) {
                let msg =
                    "[ISO/IEC 9075-4:2006 draft §14.2 SR3] SQLEXCEPTION, SQLWARNING or NOT FOUND is the one condition value of its handler";
                return Err(rule(msg).or_at(d.at));
            }
            let action = self.statement(&d.action)?;
            handlers.push(Handler { kind: d.kind, conditions, action });
        }
        Ok(handlers)
    }

    /// [§14.2 SR2, SR10, SR11] The condition `value` represents: of a
    /// `<condition name>`, the innermost condition of the name in scope,
    /// its SQLSTATE value if it is declared for one.
    fn handled(&self, value: &ConditionValue) -> Result<Handled> {
        Ok(match value {
            ConditionValue::Sqlstate(s) => Handled::State(
                SqlState::parse(s).ok_or_else(|| rule(format!("[ISO/IEC 9075-4:2006 draft §8.2 SR4] '{s}' is no SQLSTATE value")))?,
            ),
            ConditionValue::Name(n) => match self.conditions.iter().rev().find(|(name, _)| name == n) {
                Some((_, Some(state))) => Handled::State(*state),
                Some((_, None)) => Handled::Name(n.clone()),
                None => return Err(rule(format!("[ISO/IEC 9075-4:2006 draft §14.2 SR2] no condition {n} is declared here"))),
            },
            ConditionValue::Exception => Handled::Exception,
            ConditionValue::Warning => Handled::Warning,
            ConditionValue::NotFound => Handled::NotFound,
        })
    }
}
