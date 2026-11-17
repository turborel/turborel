//! [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11] `<value expression>`, bound
//! and typed: its primaries, operators, set functions [§6.5], the string
//! functions [§6.6, §6.7, §6.13], and the \[EXTENSION\] function call;
//! CASE, CAST and scalar subqueries in `binder/case.rs`, datetimes and
//! intervals in `binder/datetime.rs` and `binder/interval.rs`.

use std::sync::Arc;

use super::arguments::registered_subject;
use super::arithmetic::distinct_operand;
use super::composite::{grouping_operand, ordering_operand};
use super::invocation::{Called, Passed};
use super::set_functions::{column_references, is_aggregate_call};
use super::subquery;
use super::{Binder, NOT_AGGREGATED, Scope, nondeterministic, rule, value_specification};
use crate::ast::{Composite, Literal, SearchCondition, SetArg, SetFunctionSpecification, Sign, ValueExpression, ValueSpecification};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{
    ANY_VALUE, ARRAY_AGG, AggregateRef, ArrayAggregate, CONCATENATION, CollatedExtreme, GREATEST, LEAST, LISTAGG, ScalarRef,
    regex_function, trim_named, width_bucket,
};
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, Slot, render_expr};
use crate::params::ParameterType;
use crate::value::collation::CollationRef;
use crate::value::types::composite::row_ordered;
use crate::value::{DataType, SortOrder, Value};

/// [ISO/IEC 9075:1992 §17.6 GR3] `e`, if it is a `<dynamic parameter
/// specification>`, alone or in any number of parentheses: its index, and
/// where it is.
pub(super) fn dynamic_of(e: &ValueExpression) -> Option<(usize, usize)> {
    let mut e = e;
    while let ValueExpression::Nested(inner) = e {
        e = inner;
    }
    if let ValueExpression::Value(ValueSpecification::Dynamic { index, at }) = e {
        return Some((*index, *at));
    }
    None
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §17.6 GR5] `e` bound, a `<dynamic parameter
    /// specification>` taking the type `taken` its context gives it.
    pub(in crate::binder) fn typed(&self, e: &ValueExpression, taken: ParameterType, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        // [ISO/IEC 9075-2:2023 §6.5 SR1] ARRAY[] or MULTISET[] of the
        // collection type of its kind its context gives, or of the distinct
        // type whose source type it is [§6.13 SR6, §11.5 SR4e], the
        // collection of no element.
        if let ParameterType::Of(t) = &taken
            && let Some(empty) = super::composite::empty_specification(e).and_then(|kind| kind.empty_of(t))
        {
            return Ok((Expr::Const(empty), t.clone()));
        }
        if let Some((index, at)) = dynamic_of(e) {
            let (value, t) = self.params.dynamic_typed(index, Some(taken)).or_at(at)?;
            // Of a statement to be kept, a slot its value fills as each
            // execution begins (RULES.md D45).
            let bound = if self.params.slots() { Expr::Dynamic(Slot { index, data_type: t.clone(), at }) } else { Expr::Const(value) };
            return Ok((bound, t));
        }
        self.value_expression(e, scope)
    }

    /// [ISO/IEC 9075:1992 §17.6 GR5h, m] Two operands of one operator, or
    /// of a comparison, bound: a `<dynamic parameter specification>` among
    /// them taking the other's type. Both of them one takes none \[GR4c, i\].
    pub(super) fn pair(&self, (a, b): (&ValueExpression, &ValueExpression), scope: Scope<'_>) -> Result<[(Expr, DataType); 2]> {
        match (dynamic_of(a), dynamic_of(b)) {
            (Some(_), None) => {
                let y = self.value_expression(b, scope)?;
                Ok([self.typed(a, ParameterType::Of(y.1.clone()), scope)?, y])
            }
            (None, Some(_)) => {
                let x = self.value_expression(a, scope)?;
                let y = self.typed(b, ParameterType::Of(x.1.clone()), scope)?;
                Ok([x, y])
            }
            (Some(_), Some(_)) | (None, None) => Ok([self.value_expression(a, scope)?, self.value_expression(b, scope)?]),
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.15] The set function `func` of `arg`, bound
    /// from `written`, as its argument's collation has it: MAX and MIN
    /// order by it, and DISTINCT compares by it; and that collation, if it
    /// shows in the call's label.
    fn collated_call(
        &self,
        (func, arg, arg_type): (AggregateRef, Option<Expr>, Option<DataType>),
        written: Option<&ValueExpression>,
        (extreme, distinct): (bool, bool),
    ) -> Result<(AggregateRef, Option<Expr>, Option<CollationRef>)> {
        let collation = match (written, arg_type) {
            (Some(v), Some(t)) if t.is_character() => self.collation_of(&[self.derived(v)?])?,
            (Some(_) | None, _) => None,
        };
        Ok(match collation {
            Some(c) if extreme => {
                let keeps = CollatedExtreme { max: func.0.name() == kw::MAX, collation: c.clone() };
                (AggregateRef(Arc::new(keeps)), arg, Some(c))
            }
            Some(c) if distinct => (func, arg.map(|e| Expr::sort_key(e, &c)), Some(c)),
            Some(_) | None => (func, arg, None),
        })
    }

    /// [X3.135-1986 §5.6] A `<value specification>`, which in a constraint
    /// is no authorization identifier [ISO/IEC 9075:1992 §11.9 SR5, §11.34
    /// SR6].
    pub(super) fn value_specification(&self, vs: &ValueSpecification) -> Result<(Value, DataType)> {
        if self.purpose.constrains() && matches!(vs, ValueSpecification::User(_)) {
            return Err(rule(
                "[ISO/IEC 9075:1992 §11.9 SR5, §11.34 SR6; ISO/IEC 9075-2:2023 §11.9 SR5, §11.47 SR7] a constraint's condition shall not contain USER, CURRENT_USER, CURRENT_ROLE, SESSION_USER or SYSTEM_USER",
            ));
        }
        if let ValueSpecification::Literal(Literal::Datetime(w)) = vs {
            return super::datetime::literal(w, self.now.zone());
        }
        if let ValueSpecification::Literal(Literal::Interval(i)) = vs {
            return Ok((Value::Interval(*i), DataType::Interval(i.qualifier)));
        }
        value_specification(vs, self.user, self.params)
    }

    /// [X3.135-1986 §5.9] `<value expression>`: bind and type-check.
    ///
    /// \[DESIGN\] Each arm here is one call, its result its own; those
    /// that do more are bound by `decorated`. So the frame a nesting of
    /// expressions recurs through holds no arm's locals (RULES.md R-CF-6).
    pub(super) fn value_expression(&self, v: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match v {
            ValueExpression::Column(cs) => self.column_expr(cs, scope),
            ValueExpression::Nested(inner) => self.value_expression(inner, scope),
            ValueExpression::CollationFor(inner) => self.collation_for(inner, scope),
            ValueExpression::Collated(inner, _) => self.collated_value(inner, scope),
            ValueExpression::Monadic(sign, inner) => self.monadic(*sign, inner, scope),
            ValueExpression::Dyadic(left, op, right) => self.dyadic(left, *op, right, scope),
            ValueExpression::Subquery(s) => self.scalar_subquery(s, scope),
            ValueExpression::Datetime(f) => self.datetime_function(*f),
            ValueExpression::Value(_)
            | ValueExpression::SetFunction(_)
            | ValueExpression::Call { .. }
            | ValueExpression::Function { .. }
            | ValueExpression::Window(_)
            | ValueExpression::NestedWindow(_)
            | ValueExpression::Case(_)
            | ValueExpression::Cast { .. }
            | ValueExpression::Boolean(_)
            | ValueExpression::NextValue { .. }
            | ValueExpression::Extract { .. }
            | ValueExpression::Difference { .. }
            | ValueExpression::AtZone { .. }
            | ValueExpression::Composite(..)
            | ValueExpression::Structured(..)
            | ValueExpression::Json(..)
            | ValueExpression::Navigation(_)
            | ValueExpression::Classifier { .. }
            | ValueExpression::MatchNumber { .. } => self.decorated(v, scope),
        }
    }

    /// `v`, a `<value expression>` whose binding does more than one call:
    /// each error placed where it was written, a value drawn or not, or a
    /// literal made a constant. Of the others, as `value_expression`
    /// binds them.
    fn decorated(&self, v: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match v {
            ValueExpression::Value(vs) => {
                let (val, t) = self.value_specification(vs)?;
                Ok((Expr::Const(val), t))
            }
            // [ISO/IEC 9075-2:2023 §6.14 SR2] none draws a value in a set
            // function, a CASE or a search condition.
            ValueExpression::SetFunction(sf) => self.drawing(false, || self.set_function(sf, scope)).or_at(sf.at),
            ValueExpression::Call { schema, name, args, at, extension } => {
                self.routine_call(v, (schema.as_deref(), name, *extension), args, (*at, scope))
            }
            ValueExpression::Function { name, args, at } => self.function(name, args, scope).or_at(*at),
            ValueExpression::Window(w) => self.window_function(w, scope).or_at(w.at),
            ValueExpression::NestedWindow(n) => self.nested_window(n, scope).or_at(n.at),
            ValueExpression::Case(case) => self.drawing(false, || self.case(case, scope)),
            ValueExpression::Cast { operand, target, format, at } => {
                self.cast(operand.as_deref(), target, format.as_deref(), scope).or_at(*at)
            }
            // [ISO/IEC 9075-2:2023 §6.46 SR1] A truth value, of type BOOLEAN.
            ValueExpression::Boolean(c) => {
                let cond = self.drawing(false, || self.search_condition(c, scope))?;
                Ok((Expr::Truth(Box::new(cond)), DataType::Boolean))
            }
            ValueExpression::NextValue { name, at } => self.next_value(name).or_at(*at),
            ValueExpression::Extract { field, source, at } => self.extract(*field, source, scope).or_at(*at),
            ValueExpression::Difference { minuend, subtrahend, qualifier, at } => {
                self.difference((minuend, subtrahend), *qualifier, scope).or_at(*at)
            }
            ValueExpression::AtZone { source, zone, at } => self.at_zone(source, zone.as_deref(), scope).or_at(*at),
            ValueExpression::Composite(c, at) => self.composite((c, *at), scope).or_at(*at),
            ValueExpression::Structured(s, at) => self.structured_expression((s, *at), scope).or_at(*at),
            ValueExpression::Json(j, at) => self.json_expression(j, scope).or_at(*at),
            ValueExpression::Navigation(n) => self.navigation(n, scope).or_at(n.at),
            ValueExpression::Classifier { variable, at } => self.classifier(variable.as_deref(), scope).or_at(*at),
            ValueExpression::MatchNumber { at } => self.match_number(scope).or_at(*at),
            ValueExpression::Column(_)
            | ValueExpression::Nested(_)
            | ValueExpression::CollationFor(_)
            | ValueExpression::Collated(..)
            | ValueExpression::Monadic(..)
            | ValueExpression::Dyadic(..)
            | ValueExpression::Subquery(_)
            | ValueExpression::Datetime(_) => self.value_expression(v, scope),
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.4] `v`, a call of `name` of `schema` with
    /// `args`, written at `at`: a method invocation, an SQL-invoked
    /// function's, or a function's of the database.
    fn routine_call(
        &self,
        v: &ValueExpression,
        (schema, name, extension): (Option<&str>, &str, bool),
        args: &[ValueExpression],
        (at, scope): (usize, Scope<'_>),
    ) -> Result<(Expr, DataType)> {
        // [ISO/IEC 9075-2:2023 §10.4, §9.18 SR10h iv 1] a function of
        // the schema, an SQL-invoked function or a distinct type's
        // (D69), then one registered (D53, D74); of a call grouping
        // reads as a registered aggregate's, that aggregate.
        // [ISO/IEC 9075-2:2023 §6.6 SR11, §6.18] of a column of a
        // structured type, a method invocation of it.
        if let Some(subject) = schema.and_then(|c| self.lone_basis(c, at)) {
            return self.method_call(&subject, (name, args), scope).or_at(at);
        }
        let passed = if is_aggregate_call(v, (self.db, self.schema)) {
            Passed::default()
        } else {
            match self.invocation((schema, name), args, scope).or_at(at)? {
                Called::Done(invoked) => return Ok(invoked),
                Called::Pass(passed) => passed,
            }
        };
        let aggregate = self.db.functions.aggregate(name).is_some();
        let called = || self.call((schema, name, extension), args, passed, scope);
        self.drawing(self.drawing.get() && !aggregate, called).or_at(at)
    }

    /// [ISO/IEC 9075:1992 §6.6, §6.7, §6.13] A string function or
    /// concatenation: the function registered under `name`, its operands'
    /// types checked by it.
    fn function(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if let Some(bound) = self.array_function(name, args, scope) {
            return bound;
        }
        if name == CONCATENATION {
            return self.concatenation(args, scope);
        }
        if name == GREATEST || name == LEAST {
            return self.greatest_or_least(name == GREATEST, args, scope);
        }
        if let Some(trim) = trim_named(name) {
            return self.trim(trim, args, scope);
        }
        // [ISO/IEC 9075-2:2023 §6.31, §6.33] a regular expression
        // function, each call its own, keeping its last pattern compiled.
        // [ISO/IEC 9075-2:2023 §6.31] WIDTH_BUCKET, each call its own,
        // which learns its result type as it is bound [SR21].
        let f = match regex_function(name).or_else(|| width_bucket(name)) {
            Some(f) => ScalarRef(f),
            None => self.db.functions.scalar(name).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {name} is not registered")))?,
        };
        let (exprs, types) = self.arguments(&*f.0, args, scope)?;
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// [ISO/IEC 9075:1992 §6.13] `<concatenation>` of character or binary
    /// strings, the function registered under its name; [ISO/IEC
    /// 9075-2:2023 §6.47] `<array concatenation>`, if an operand is an
    /// array, a `?` taking its type [§9.5].
    fn concatenation(&self, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let bound = self.prebound(args, scope)?;
        let array = bound.iter().flatten().find(|(_, t)| matches!(t, DataType::Array(_))).map(|(_, t)| t.clone());
        if let Some(array) = array {
            let typed = |(b, a): (Option<(Expr, DataType)>, &ValueExpression)| match b {
                Some(done) => Ok(done),
                None => self.typed(a, ParameterType::Of(array.clone()), scope),
            };
            let pair = bound.into_iter().zip(args).map(typed).collect::<Result<Vec<_>>>()?;
            let concatenated = if let [left, right] = pair.as_slice() { Self::array_concatenation((left, right)) } else { None };
            return concatenated.unwrap_or_else(|| {
                Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §6.47 SR2] an array is concatenated with an array, not with a value of another type than {array}"
                )))
            });
        }
        let f = self
            .db
            .functions
            .scalar(CONCATENATION)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {CONCATENATION} is not registered")))?;
        let (exprs, types) = self.arguments_of(&*f.0, args, bound, scope)?;
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// [X3.135-1986 §5.9 GR4] monadic plus or minus, folded into a constant operand.
    pub(super) fn monadic(&self, sign: Sign, inner: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(inner, scope)?;
        if t.is_character() {
            return Err(rule("[X3.135-1986 §5.9 SR3] a character string <primary> cannot have an operator"));
        }
        if t.is_binary() || t.is_datetime() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.12] {t} is no <numeric primary>")));
        }
        distinct_operand(&t)?;
        Ok(match (sign, e) {
            (Sign::Plus, e) => (e, t),
            (Sign::Minus, Expr::Const(c)) => (Expr::Const(c.negate()), t),
            (Sign::Minus, e) => (Expr::Neg(Box::new(e)), t),
        })
    }

    /// [X3.135-1986 §5.8] a `<set function specification>`.
    pub(super) fn set_function(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let scope = self.pattern_set_function(sf, scope)?;
        match &sf.arg {
            SetArg::Grouping(columns) => return self.grouping_operation(columns, scope),
            SetArg::Hypothetical(values) => return self.hypothetical(sf, values, scope),
            SetArg::Fraction(fraction) => return self.inverse_distribution(sf, fraction, scope),
            SetArg::Variable(name, at) => return self.pattern_count(sf, name, scope).or_at(*at),
            SetArg::CountStar | SetArg::Distinct(_) | SetArg::All(_) => {}
        }
        if let Some(json) = self.json_aggregate(sf, scope) {
            return json;
        }
        if sf.name == ARRAY_AGG {
            return self.array_aggregate(sf, scope);
        }
        if sf.name == LISTAGG {
            return self.listagg(sf, scope);
        }
        if let Some(gathered) = self.multiset_aggregate(sf, scope) {
            return gathered;
        }
        let func: AggregateRef = self
            .db
            .functions
            .aggregate(sf.name)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the set function {} is not registered", sf.name)))?;
        match &sf.arg {
            SetArg::CountStar => self.aggregate(func, (None, false), sf.filter.as_deref(), scope),
            SetArg::Distinct(argument) => self.aggregate(func, (Some(argument), true), sf.filter.as_deref(), scope),
            SetArg::All(argument) => self.aggregate(func, (Some(argument), false), sf.filter.as_deref(), scope),
            SetArg::Grouping(columns) => self.grouping_operation(columns, scope),
            SetArg::Hypothetical(values) => self.hypothetical(sf, values, scope),
            SetArg::Fraction(fraction) => self.inverse_distribution(sf, fraction, scope),
            SetArg::Variable(name, at) => self.pattern_count(sf, name, scope).or_at(*at),
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.9] ARRAY_AGG ( V ORDER BY K1, ... ): the
    /// aggregate of the row ( V, K1, ... ), so a null V is kept \[NOTE
    /// 528\], ordered by its keys, each as its collation orders it [§9.15]
    /// and an operand of an ordering operation [§9.14 SR2f].
    fn array_aggregate(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (SetArg::All(value) | SetArg::Distinct(value)) = &sf.arg else {
            return Err(SqlError::new(Code::Api, "[DESIGN] ARRAY_AGG is read with an argument"));
        };
        let mut parts = vec![(**value).clone()];
        let mut collations = Vec::new();
        for key in &sf.order_by {
            // A key not a character string is its own sort key.
            collations.push(self.collation_of(&[self.derived(&key.key)?])?);
            parts.push(key.key.clone());
        }
        let row = ValueExpression::Composite(Box::new(Composite::Row(parts)), sf.at);
        let orders = sf.order_by.iter().map(|k| SortOrder::of(k.descending, k.nulls_first)).collect();
        let func = AggregateRef(Arc::new(ArrayAggregate { orders, collations, by_query: false }));
        self.sorted(|| self.aggregate(func, (Some(&row), false), sf.filter.as_deref(), scope))
    }

    /// \[EXTENSION\] A call by name of a registered scalar or aggregate
    /// function, of no schema, read while the session took function calls
    /// (`extension`), no function of the schema being invoked: its
    /// arguments as `passed` holds them if bound to choose among those.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names a schema, which a routine invocation does
    /// and a call of a function registered does not; -104 if the session
    /// did not take function calls, as SQL-86 has none; -103 if no
    /// function of the name is registered, or none takes its arguments;
    /// the refusal `passed` holds, of a function of the schema of the name,
    /// if no function registered is of it.
    pub(super) fn call(
        &self,
        (schema, name, extension): (Option<&str>, &str, bool),
        args: &[ValueExpression],
        passed: Passed,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let Passed { bound, refusal } = passed;
        let registered = self.db.functions.scalars(name);
        let refusal = match refusal {
            Some(refused) if schema.is_some() || registered.is_empty() || !extension => return Err(refused),
            other => other,
        };
        if let Some(s) = schema {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10b] schema {s} has no function {name}")));
        }
        if !extension {
            return Err(SqlError::new(
                Code::Syntax,
                "[X3.135-1986 §5.9 F] function calls are not SQL-86 (they are an [EXTENSION], taken while SQL.FUNCTION_CALLS is ON); expected a <value expression>",
            ));
        }
        if !registered.is_empty() {
            let bound = match bound {
                Some(b) => b,
                None => self.prebound(args, scope)?,
            };
            let Some(f) = registered_subject(name, registered, &bound)? else {
                let types = bound.iter().map(|b| b.as_ref().map(|(_, t)| t.clone())).collect::<Vec<_>>();
                let shown = super::invocation::shown_types(&types);
                return Err(
                    refusal.unwrap_or_else(|| rule(format!("[EXTENSION] no function {name} registered takes arguments of ({shown})")))
                );
            };
            let (exprs, types) = self.arguments_of(&*f.0, args, bound, scope)?;
            let t = f.0.return_type(&types)?;
            return Ok((Expr::Call(f, exprs), t));
        }
        let Some(f) = self.db.functions.aggregate(name) else {
            return Err(rule(format!("[EXTENSION] no function named {name} is registered")));
        };
        let [arg] = args else {
            return Err(rule(format!("[EXTENSION] aggregate {name} takes exactly one argument")));
        };
        self.aggregate(f, (Some(arg), false), None, scope)
    }

    /// [X3.135-1986 §5.23 GR3] The set function `func` of `arg`, if `arg`
    /// is an outer reference: the enclosing query's (`binder/subquery.rs`),
    /// filtered by `filter` there. None if it is not.
    fn outer_set_function(
        &self,
        func: &AggregateRef,
        (arg, distinct): (Option<&ValueExpression>, bool),
        filter: Option<&SearchCondition>,
    ) -> Result<Option<(Expr, DataType)>> {
        let (Some(v), Some(outer)) = (arg, self.outer) else { return Ok(None) };
        if !self.is_outer_argument(v)? {
            return Ok(None);
        }
        self.outer_filter(filter)?;
        // [ISO/IEC 9075:1992 §6.5 SR4] the outer reference alone:
        // SQL-86's rule of no operators [X3.135-1986 §5.8 SR4] is Entry
        // SQL's [§6.5, Leveling Rules].
        let mut columns = Vec::new();
        column_references(v, &mut columns);
        if columns.len() > 1 {
            return Err(rule("[ISO/IEC 9075:1992 §6.5 SR4] an outer reference in a set function's argument is its only column reference"));
        }
        subquery::outer_aggregate(outer, func.clone(), (v, distinct), filter).map(Some)
    }

    /// Bind a set function [X3.135-1986 §5.8] or extension aggregate and register it with the grouping.
    /// The aggregate function of a window function is bound and kept apart
    /// instead (`binder/window.rs`).
    pub(super) fn aggregate(
        &self,
        func: AggregateRef,
        argued: (Option<&ValueExpression>, bool),
        filter: Option<&SearchCondition>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        if self.windowed_next() {
            return self.window_aggregate(func, argued, filter, scope);
        }
        if let Some(outer) = self.outer_set_function(&func, argued, filter)? {
            return Ok(outer);
        }
        let g = match scope {
            Scope::Groups(g, _) => g,
            Scope::Rows(why) => return Err(rule(why)),
            Scope::Select => return Err(rule(NOT_AGGREGATED)),
        };
        let rows = Scope::Rows("[ISO/IEC 9075:1992 §6.5 SR4] a set function argument cannot contain a set function");
        // [ISO/IEC 9075-2:2023 §6.11 SR1, §9.20] a group's set function,
        // made a column of the groups before any window function, holds no
        // nested window function.
        let (call, collation) = self.unnested(|| self.aggregate_call(func, argued, filter, rows))?;
        Ok(self.registered_in(g, call, collation))
    }

    /// The set function `func` of `arg`, bound in `scope`, DISTINCT if so,
    /// filtered by `filter` if written: its argument, as the Syntax Rules
    /// have it, and its result type; and the collation of its argument, if
    /// it is compared by one.
    pub(super) fn aggregate_call(
        &self,
        func: AggregateRef,
        (arg, distinct): (Option<&ValueExpression>, bool),
        filter: Option<&SearchCondition>,
        scope: Scope<'_>,
    ) -> Result<(AggCall, Option<CollationRef>)> {
        let arg_of = arg;
        let (arg, arg_type) = match arg {
            None => (None, None),
            Some(v) => {
                // [ISO/IEC 9075:1992 §6.5 SR4] no set function or subquery in
                // the argument; SQL-86's rule that it reference a column
                // [X3.135-1986 §5.8 SR4] is Entry SQL's alone [Leveling Rules].
                if v.any(&|e| matches!(e, ValueExpression::Subquery(_))) {
                    return Err(rule("[ISO/IEC 9075:1992 §6.5 SR4] the argument of a set function cannot contain a subquery"));
                }
                let (e, t) = self.value_expression(v, scope)?;
                (Some(e), Some(t))
            }
        };
        let extreme = func.0.name() == kw::MAX || func.0.name() == kw::MIN;
        if let Some(t) = &arg_type {
            operand_rules(self.db, t, func.0.name(), (extreme, distinct))?;
        }
        // [ISO/IEC 9075-2:2023 §9.12, §9.14] MAX, MIN and DISTINCT of a
        // structured type ordered by MAP compare its values' keys.
        let arg = match (arg, &arg_type) {
            (Some(e), Some(t)) if extreme || distinct => {
                let e = super::structured::keyed_operand(self.context(), (e, t), func.0.name())?;
                Some(self.keyed_value(e, t, func.0.name())?)
            }
            // [§9.13 SR1b] INTERSECTION groups its multisets' elements.
            (Some(e), Some(t)) if func.0.name() == "INTERSECTION" => Some(self.keyed_elements(e, t, "INTERSECTION")?),
            (arg, _) => arg,
        };
        if self.purpose.constrains() && extreme && arg_type.clone().is_some_and(|t| t.is_character()) {
            return Err(nondeterministic("§7.8 SR3a, §7.9 SR13c i", "MAX or MIN of a character string"));
        }
        if let Some(item) = possibly_nondeterministic(func.0.name()).filter(|_| self.purpose.constrains()) {
            return Err(nondeterministic_source(item, func.0.name()));
        }
        let (func, arg, named) = self.collated_call((func, arg, arg_type.clone()), arg_of, (extreme, distinct))?;
        let result = func.0.result_type(arg_type)?;
        let collated = named.as_ref().map_or_else(String::new, |c| format!(" COLLATE {}", c.collation.name()));
        let label = match &arg {
            None => format!("{}(*)", func.0.name()),
            Some(e) => {
                let (name, suffix) = (func.0.name(), func.0.label_suffix());
                format!("{name}({}{}{collated}{suffix})", if distinct { "DISTINCT " } else { "" }, render_expr(e, &self.names))
            }
        };
        let filter = self.filter_of(filter, scope)?;
        let label = self.filtered_label(label, filter.as_ref());
        Ok((AggCall { func, arg, distinct, filter, distribution: None, result, label }, named))
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 SR7c, SR7d] The argument of set function
/// `name`, of type `t`: an operand of a grouping operation under DISTINCT
/// [§9.12]; of MAX or MIN, if `extreme`, of an ordering operation [§9.14],
/// and not row-ordered \[SR7d ii\].
fn operand_rules(db: &crate::db::State, t: &DataType, name: &str, (extreme, distinct): (bool, bool)) -> Result<()> {
    if distinct {
        grouping_operand(db, t, &format!("{name}(DISTINCT ...)"))?;
    }
    if !extreme {
        return Ok(());
    }
    ordering_operand(db, t, name)?;
    if row_ordered(t) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR7d ii] {name} of {t}: rows are not totally ordered, (1, NULL) and (NULL, 1) neither before the other"
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.16 SR1, §11.9 CR2] A constraint's condition
/// contains `what`, a potential source of non-determinism by the item
/// `item` of §9.16 SR1, which a CHECK may contain only with Feature F672,
/// not claimed: so SQL-92's rule refuses it [ISO/IEC 9075:1992 §11.9 SR6].
pub(super) fn nondeterministic_source(item: &str, what: &str) -> SqlError {
    rule(format!(
        "[ISO/IEC 9075-2:2023 §9.16 SR1{item}, §11.9 CR2] a CHECK condition shall not contain a potential source of non-determinism: {what}"
    ))
}

/// [ISO/IEC 9075-2:2023 §9.16 SR1ap, SR1ar, SR1as] The item of §9.16 SR1
/// that makes a call of the set function `name` a potential source of
/// non-determinism: ANY_VALUE, ARRAY_AGG and LISTAGG; of others, none.
fn possibly_nondeterministic(name: &str) -> Option<&'static str> {
    [(ANY_VALUE, "ap"), (ARRAY_AGG, "ar"), (LISTAGG, "as")].into_iter().find(|(f, _)| *f == name).map(|(_, item)| item)
}
