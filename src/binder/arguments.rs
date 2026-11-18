//! The arguments of a scalar function, bound: a `<dynamic parameter
//! specification>` among them typed as the function says [ISO/IEC
//! 9075:1992 §17.6 GR5], and \[EXTENSION\] each of a type the function
//! declares (`ScalarFunction::argument_type`) stored in it by store
//! assignment [§9.2], as a column's value is. The argument of a set
//! function, a `?` in it typed as the function says too.

use std::sync::Arc;

use super::expression::dynamic_of;
use super::invocation::Prebound;
use super::statements::assignable;
use super::{Binder, Scope, datetime, rule};
use crate::ast::{Composite, ValueExpression};
use crate::error::{At, Result};
use crate::functions::{AggregateFunction, ScalarFunction, ScalarRef};
use crate::logical::Expr;
use crate::value::precedence::{subject_routines, takes};
use crate::value::{DataType, Value, assign};

/// The rule an argument's assignment cites: store assignment's, a
/// function's argument being no target the standard names.
const ARGUMENT_RULE: &str = "ISO/IEC 9075:1992 §9.2 GR3";

/// The name of the call that stores an argument in its declared type.
pub(super) const ASSIGN: &str = "ASSIGN";

/// \[EXTENSION\] [ISO/IEC 9075:1992 §9.2] An argument stored in the type
/// its function declares for it, as `value::assign` stores a value in a
/// column of that type.
struct Assigned {
    to: DataType,
}

impl ScalarFunction for Assigned {
    fn name(&self) -> &'static str {
        ASSIGN
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.to.clone())
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
        let bound = self.prebound(args, scope)?;
        self.arguments_of(f, args, bound, scope)
    }

    /// [ISO/IEC 9075:1992 §17.6 GR5] `v`, the argument of the set
    /// function `f`, bound: a `?` as it, or as a field of it if it is a
    /// ROW, taking the type `f` gives there
    /// (`AggregateFunction::parameter_type`).
    pub(super) fn aggregate_argument(&self, f: &dyn AggregateFunction, v: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let typed = |(position, field): (usize, &ValueExpression)| dynamic_of(field).is_some() && f.parameter_type(position).is_some();
        if let ValueExpression::Composite(c, at) = v
            && let Composite::Row(fields) = &**c
            && fields.iter().enumerate().any(typed)
        {
            return self.row_typed(fields, &|position| f.parameter_type(position), scope).or_at(*at);
        }
        match f.parameter_type(0).filter(|_| dynamic_of(v).is_some()) {
            Some(taken) => self.typed(v, taken, scope),
            None => self.value_expression(v, scope),
        }
    }

    /// `args` bound, each but a `<dynamic parameter specification>`, which
    /// takes its type from the others.
    pub(super) fn prebound(&self, args: &[ValueExpression], scope: Scope<'_>) -> Result<Prebound> {
        args.iter().map(|a| if dynamic_of(a).is_some() { Ok(None) } else { self.value_expression(a, scope).map(Some) }).collect()
    }

    /// `arguments`, of `args` bound as `prebound` binds them.
    pub(super) fn arguments_of(
        &self,
        f: &dyn ScalarFunction,
        args: &[ValueExpression],
        bound: Prebound,
        scope: Scope<'_>,
    ) -> Result<(Vec<Expr>, Vec<DataType>)> {
        let known: Vec<Option<DataType>> = bound.iter().map(|b| b.as_ref().map(|(_, t)| t.clone())).collect();
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
        let stored = datetime::stored(e, t, &to.clone(), self.now);
        Ok((Expr::Call(ScalarRef(Arc::new(Assigned { to: to.clone() })), vec![stored]), to))
    }
}

/// \[EXTENSION\] [ISO/IEC 9075-2:2023 §9.18 SR10, §9.6] Of `registered`,
/// the scalar functions registered of `name`, the one a call with
/// arguments `bound`, each but a `?` bound, invokes: one that declares
/// no parameter types, alone of its name; else of those whose
/// parameters take the arguments, the one subject routine (RULES.md
/// D74). None if none takes them.
///
/// # Errors
///
/// SQLCODE -103 if more than one is alike.
pub(super) fn registered_subject(name: &str, registered: Vec<ScalarRef>, bound: &Prebound) -> Result<Option<ScalarRef>> {
    if let [only] = registered.as_slice()
        && only.0.parameter_types().is_none()
    {
        return Ok(Some(only.clone()));
    }
    let types: Vec<Option<DataType>> = bound.iter().map(|b| b.as_ref().map(|(_, t)| t.clone())).collect();
    let declared: Vec<(ScalarRef, Vec<DataType>)> =
        registered.into_iter().filter_map(|f| f.0.parameter_types().map(|p| (f, p))).filter(|(_, p)| takes(p, &types)).collect();
    let lists: Vec<&[DataType]> = declared.iter().map(|(_, p)| p.as_slice()).collect();
    match subject_routines(&lists, &types).as_slice() {
        [one] => Ok(declared.get(*one).map(|(f, _)| f.clone())),
        [] => Ok(None),
        [_, _, ..] => Err(rule(format!(
            "[EXTENSION] [ISO/IEC 9075-2:2023 §9.6] more than one function {name} registered takes arguments of ({}) alike",
            super::invocation::shown_types(&types)
        ))),
    }
}
