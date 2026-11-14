//! [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11] `<value expression>`, bound
//! and typed: its primaries, operators, set functions [§6.5], the string
//! functions [§6.6, §6.7, §6.13], and the \[EXTENSION\] function call;
//! CASE, CAST and scalar subqueries in `binder/case.rs`, datetimes and
//! intervals in `binder/datetime.rs` and `binder/interval.rs`.

use std::sync::Arc;

use super::composite::{grouping_operand, ordering_operand};
use super::interval::assumed;
use super::set_functions::{column_references, registered};
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
use crate::value::Spelled;
use crate::value::collation::CollationRef;
use crate::value::types::composite::row_ordered;
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, SortOrder, Value};

/// [X3.135-1986 §5.9 SR4] The type of an operation on two exact numbers. Its
/// precision is implementor-defined: `MAX_EXACT_PRECISION`. Its scale is
/// max(s1, s2) for + and - \[SR4b\], s1 + s2 for * \[SR4c\], and for / at least
/// max(s1, s2): a quotient keeps as many more fractional digits as fit, so
/// as to lose no leading digit \[GR6b\], the implementor-defined choice of
/// \[SR4d\] (`Decimal::checked_div`).
///
/// A scale above the precision is not a type [X3.135-1986 §5.5 SR4], so a
/// product whose scale would exceed `MAX_EXACT_PRECISION` is valid SQL-86 that
/// this implementation does not process: SQLCODE -900. Capping the scale
/// instead made every row fail at run time.
fn exact_result_type(op: ArithOp, s1: u32, s2: u32) -> Result<DataType> {
    let scale = match op {
        ArithOp::Mul => s1.checked_add(s2).filter(|s| *s <= MAX_EXACT_PRECISION).ok_or_else(|| {
            SqlError::new(
                Code::Unsupported,
                format!(
                    "[X3.135-1986 §5.9 SR4c, §5.5 SR4] a product of scales {s1} and {s2} needs a scale above this implementation's maximum precision of {MAX_EXACT_PRECISION}"
                ),
            )
        })?,
        ArithOp::Add | ArithOp::Sub | ArithOp::Div => s1.max(s2),
    };
    Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric })
}

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
    pub(super) fn typed(&self, e: &ValueExpression, taken: ParameterType, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        // [ISO/IEC 9075-2:2023 §6.5 SR1] ARRAY[] of the array type its
        // context gives, or of the distinct type whose source type it is
        // [§6.13 SR6, §11.5 SR4e], an empty array.
        if let ParameterType::Of(t) = &taken
            && matches!(t.represented(), DataType::Array(_))
            && super::composite::is_empty_array(e)
        {
            return Ok((Expr::Const(Value::Array(Vec::new().into())), t.clone()));
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

    /// [ISO/IEC 9075:1992 §17.6 GR5f-h] The two operands of `op` bound: a
    /// `<dynamic parameter specification>` among them taking the type
    /// `interval::assumed` gives. Both of them one takes none \[GR4c\].
    fn operands(&self, (a, b): (&ValueExpression, &ValueExpression), op: ArithOp, scope: Scope<'_>) -> Result<[(Expr, DataType); 2]> {
        match (dynamic_of(a), dynamic_of(b)) {
            (Some(_), None) => {
                let y = self.value_expression(b, scope)?;
                Ok([self.typed(a, ParameterType::Of(assumed(op, y.1.clone(), true)), scope)?, y])
            }
            (None, Some(_)) => {
                let x = self.value_expression(a, scope)?;
                let y = self.typed(b, ParameterType::Of(assumed(op, x.1.clone(), false)), scope)?;
                Ok([x, y])
            }
            (Some(_), Some(_)) | (None, None) => self.pair((a, b), scope),
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
                "[ISO/IEC 9075:1992 §11.9 SR5, §11.34 SR6] a constraint's condition shall not contain USER, CURRENT_USER, SESSION_USER or SYSTEM_USER",
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
    pub(super) fn value_expression(&self, v: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match v {
            ValueExpression::Value(vs) => {
                let (val, t) = self.value_specification(vs)?;
                Ok((Expr::Const(val), t))
            }
            ValueExpression::Column(cs) => self.column_expr(cs, scope),
            ValueExpression::Nested(inner) => self.value_expression(inner, scope),
            ValueExpression::CollationFor(inner) => self.collation_for(inner, scope),
            ValueExpression::Collated(inner, _) => self.collated_value(inner, scope),
            ValueExpression::Monadic(sign, inner) => self.monadic(*sign, inner, scope),
            ValueExpression::Dyadic(left, op, right) => self.dyadic(left, *op, right, scope),
            // [ISO/IEC 9075-2:2023 §6.14 SR2] none draws a value in a set
            // function, a CASE or a search condition.
            ValueExpression::SetFunction(sf) => self.drawing(false, || self.set_function(sf, scope)).or_at(sf.at),
            ValueExpression::Call { schema, name, args, at, extension } => {
                if let Some(called) = self.routine_call((schema.as_deref(), name), args, scope).or_at(*at)? {
                    return Ok(called);
                }
                let aggregate = self.db.functions.aggregate(name).is_some();
                self.drawing(self.drawing.get() && !aggregate, || self.call((schema.as_deref(), name, *extension), args, scope)).or_at(*at)
            }
            ValueExpression::Function { name, args, at } => self.function(name, args, scope).or_at(*at),
            ValueExpression::Window(w) => self.window_function(w, scope).or_at(w.at),
            ValueExpression::NestedWindow(n) => self.nested_window(n, scope).or_at(n.at),
            ValueExpression::Case(case) => self.drawing(false, || self.case(case, scope)),
            ValueExpression::Cast { operand, target, format, at } => {
                self.cast(operand.as_deref(), target, format.as_deref(), scope).or_at(*at)
            }
            ValueExpression::Subquery(s) => self.scalar_subquery(s, scope),
            // [ISO/IEC 9075-2:2023 §6.46 SR1] A truth value, of type BOOLEAN.
            ValueExpression::Boolean(c) => {
                let cond = self.drawing(false, || self.search_condition(c, scope))?;
                Ok((Expr::Truth(Box::new(cond)), DataType::Boolean))
            }
            ValueExpression::NextValue { name, at } => self.next_value(name).or_at(*at),
            ValueExpression::Datetime(f) => self.datetime_function(*f),
            ValueExpression::Extract { field, source, at } => self.extract(*field, source, scope).or_at(*at),
            ValueExpression::Difference { minuend, subtrahend, qualifier, at } => {
                self.difference((minuend, subtrahend), *qualifier, scope).or_at(*at)
            }
            ValueExpression::AtZone { source, zone, at } => self.at_zone(source, zone.as_deref(), scope).or_at(*at),
            ValueExpression::Composite(c, at) => self.composite((c, *at), scope).or_at(*at),
        }
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

    /// [X3.135-1986 §5.9 SR4, SR5] a dyadic operator and the type of its result.
    pub(super) fn dyadic(
        &self,
        left: &ValueExpression,
        op: ArithOp,
        right: &ValueExpression,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        // [ISO/IEC 9075:1992 §17.6 GR5f-h] a `?` takes the type its other
        // operand gives it. Without one, each is bound as it is, so a chain
        // of operators nests no deeper in the stack than it did.
        let dynamic = dynamic_of(left).is_some() || dynamic_of(right).is_some();
        let [(a, left_type), (b, right_type)] = if dynamic {
            self.operands((left, right), op, scope)?
        } else {
            [self.value_expression(left, scope)?, self.value_expression(right, scope)?]
        };
        if left_type.is_character() || right_type.is_character() {
            return Err(rule("[X3.135-1986 §5.9 SR3] character string values cannot be used with arithmetic operators"));
        }
        if left_type.is_binary() || right_type.is_binary() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.30 SR3] binary string values cannot be used with arithmetic operators"));
        }
        distinct_operand(&left_type)?;
        distinct_operand(&right_type)?;
        // [§6.14, §6.15] datetimes and intervals.
        if left_type.is_datetime() || right_type.is_datetime() || left_type.is_interval() || right_type.is_interval() {
            return self.temporal((a, left_type), op, (b, right_type), dynamic);
        }
        let t = match (left_type, right_type) {
            (DataType::Exact { scale: s1, .. }, DataType::Exact { scale: s2, .. }) => exact_result_type(op, s1, s2)?,
            // [ISO/IEC 9075-2:2023 §6.30 SR1a] DECFLOAT of 34 digits (IV134).
            (DataType::DecFloat { .. }, _) | (_, DataType::DecFloat { .. }) => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // [X3.135-1986 §5.9 SR5]
            _ => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        };
        Ok((Expr::Arith(Box::new(a), op, Box::new(b)), t))
    }

    /// [X3.135-1986 §5.8] a `<set function specification>`.
    pub(super) fn set_function(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match &sf.arg {
            SetArg::Grouping(columns) => return self.grouping_operation(columns, scope),
            SetArg::Hypothetical(values) => return self.hypothetical(sf, values, scope),
            SetArg::Fraction(fraction) => return self.inverse_distribution(sf, fraction, scope),
            SetArg::CountStar | SetArg::Distinct(_) | SetArg::All(_) => {}
        }
        if sf.name == ARRAY_AGG {
            return self.array_aggregate(sf, scope);
        }
        if sf.name == LISTAGG {
            return self.listagg(sf, scope);
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
        self.aggregate(func, (Some(&row), false), sf.filter.as_deref(), scope)
    }

    /// \[EXTENSION\] A call by name of a registered scalar or aggregate
    /// function, of no schema, read while the session took function calls
    /// (`extension`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names a schema, which a routine invocation does
    /// and a call of a function registered does not; -104 if the session
    /// did not take function calls, as SQL-86 has none; -103 if no
    /// function of the name is registered.
    pub(super) fn call(
        &self,
        (schema, name, extension): (Option<&str>, &str, bool),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        if let Some(s) = schema {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10b] schema {s} has no function {name}")));
        }
        if !extension {
            return Err(SqlError::new(
                Code::Syntax,
                "[X3.135-1986 §5.9 F] function calls are not SQL-86 (they are an [EXTENSION], taken while SQL.FUNCTION_CALLS is ON); expected a <value expression>",
            ));
        }
        if let Some(f) = self.db.functions.scalar(name) {
            let (exprs, types) = self.arguments(&*f.0, args, scope)?;
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
        Ok(registered(g, call, collation))
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
            operand_rules(t, func.0.name(), (extreme, distinct))?;
        }
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
fn operand_rules(t: &DataType, name: &str, (extreme, distinct): (bool, bool)) -> Result<()> {
    if distinct {
        grouping_operand(t, &format!("{name}(DISTINCT ...)"))?;
    }
    if !extreme {
        return Ok(());
    }
    ordering_operand(t, name)?;
    if row_ordered(t) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR7d ii] {name} of {t}: rows are not totally ordered, (1, NULL) and (NULL, 1) neither before the other"
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §6.30 SR3, §4.9.5] That `t`, an arithmetic
/// operand's type, is no distinct type: its values are of no numeric type,
/// whatever its source, and are operands of comparison alone, or of the
/// functions its definition makes (`binder/routines.rs`).
fn distinct_operand(t: &DataType) -> Result<()> {
    if let DataType::Distinct(_) = t {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.30 SR3] a <numeric primary> is numeric, not of the distinct type {t}")));
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

// Unit tests: the exact result types of [X3.135-1986 §5.9 SR4] (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.9 SR4b, SR4c, §5.5 SR4] product scale is s1 + s2 up to
    /// `MAX_EXACT_PRECISION` and rejected beyond it; sums keep max(s1, s2).
    #[test]
    fn exact_result_scales() {
        let half = 9_u32; // any split of MAX_EXACT_PRECISION into two scales
        let scale = |op, s1, s2| {
            exact_result_type(op, s1, s2)
                .map(|t| if let DataType::Exact { scale, .. } = t { Some(scale) } else { None })
                .map_err(|e| e.code)
        };
        assert_eq!(scale(ArithOp::Mul, half, MAX_EXACT_PRECISION - half), Ok(Some(MAX_EXACT_PRECISION)));
        assert_eq!(scale(ArithOp::Mul, half, MAX_EXACT_PRECISION - half + 1), Err(Code::Unsupported));
        assert_eq!(scale(ArithOp::Add, MAX_EXACT_PRECISION, MAX_EXACT_PRECISION), Ok(Some(MAX_EXACT_PRECISION)));
        assert_eq!(scale(ArithOp::Sub, 2, 5), Ok(Some(5)));
        assert_eq!(scale(ArithOp::Div, 5, 2), Ok(Some(5)));
    }
}
