//! The arguments of a scalar function, bound: a `<dynamic parameter
//! specification>` among them typed as the function says [ISO/IEC
//! 9075:1992 §17.6 GR5], and \[EXTENSION\] each of a type the function
//! declares (`ScalarFunction::argument_type`) stored in it by store
//! assignment [§9.2], as a column's value is. Split from
//! `binder/expression.rs` (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::expression::dynamic_of;
use super::statements::assignable;
use super::{Binder, Scope, datetime, rule};
use crate::ast::ValueExpression;
use crate::error::Result;
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::Expr;
use crate::value::{DataType, Value, assign};

/// The rule an argument's assignment cites: store assignment's, a
/// function's argument being no target the standard names.
const ARGUMENT_RULE: &str = "ISO/IEC 9075:1992 §9.2 GR3";

/// \[EXTENSION\] [ISO/IEC 9075:1992 §9.2] An argument stored in the type
/// its function declares for it, as `value::assign` stores a value in a
/// column of that type.
struct Assigned {
    to: DataType,
}

impl ScalarFunction for Assigned {
    fn name(&self) -> &'static str {
        "ASSIGN"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        assign(args.first().cloned().unwrap_or(Value::Null), &self.to, ARGUMENT_RULE)
    }
}

impl Binder<'_> {
    /// The arguments `args` of the scalar function `f`, bound: a `<dynamic
    /// parameter specification>` among them taking the type `f` gives it,
    /// the others bound first [ISO/IEC 9075:1992 §17.6 GR5b-d], refused
    /// where it gives none \[GR4\]; then each of a type `f` declares, as
    /// stored in it (`declared`).
    ///
    /// # Errors
    ///
    /// An argument breaks a rule, or is not assignable to its declared type.
    pub(super) fn arguments(
        &self,
        f: &dyn ScalarFunction,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Vec<Expr>, Vec<DataType>)> {
        let mut bound = Vec::new();
        for a in args {
            bound.push(if dynamic_of(a).is_some() { None } else { Some(self.value_expression(a, scope)?) });
        }
        let known: Vec<Option<DataType>> = bound.iter().map(|b| b.as_ref().map(|(_, t)| *t)).collect();
        let mut out = Vec::new();
        for (position, (slot, a)) in bound.into_iter().zip(args).enumerate() {
            let given = match (slot, f.parameter_type(position, &known)) {
                (Some(done), _) => done,
                (None, Some(taken)) => self.typed(a, taken, scope)?,
                (None, None) => self.value_expression(a, scope)?,
            };
            out.push(self.declared(f, (position, &known), given)?);
        }
        Ok(out.into_iter().unzip())
    }

    /// \[EXTENSION\] Argument `position` of `f`, `given` as bound, stored
    /// in the type `f` declares for it, if it declares one and it is not
    /// its own [ISO/IEC 9075:1992 §9.2]: of a datetime without time zone in
    /// one with, of the session's displacement \[GR3l\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its type cannot be assigned to the declared one
    /// \[SR2\].
    fn declared(
        &self,
        f: &dyn ScalarFunction,
        (position, known): (usize, &[Option<DataType>]),
        given: (Expr, DataType),
    ) -> Result<(Expr, DataType)> {
        let (e, t) = given;
        let Some(to) = f.argument_type(position, known).filter(|to| *to != t) else { return Ok((e, t)) };
        if !assignable(&to, &t) {
            return Err(rule(format!(
                "[EXTENSION] [ISO/IEC 9075:1992 §9.2 SR2] argument {} of {}, a {t} value, cannot be assigned to its type {to}",
                position.saturating_add(1),
                f.name()
            )));
        }
        let stored = datetime::stored(e, t, to, self.now);
        Ok((Expr::Call(ScalarRef(Arc::new(Assigned { to })), vec![stored]), to))
    }
}
