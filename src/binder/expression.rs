//! [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11] `<value expression>`, bound
//! and typed: its primaries, operators, set functions [§6.5], the string
//! functions [§6.6, §6.7, §6.13], and the \[EXTENSION\] function call;
//! CASE, CAST and scalar subqueries in `binder/case.rs`. Split from
//! binder.rs (RULES.md R-SIZE-2).

use super::set_functions::column_specifications;
use super::subquery;
use super::{Binder, NOT_AGGREGATED, Scope, nondeterministic, rule, value_specification};
use crate::ast::{Literal, SetArg, SetFunctionSpecification, Sign, ValueExpression, ValueSpecification};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{AggregateRef, ScalarFunction};
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, render_expr};
use crate::params::ParameterType;
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, MAX_EXACT_PRECISION, Value};

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
    Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale })
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
        if let Some((index, at)) = dynamic_of(e) {
            let (value, t) = self.params.dynamic_value(index, Some(taken)).or_at(at)?;
            return Ok((Expr::Const(value), t));
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
                Ok([self.typed(a, ParameterType::Of(y.1), scope)?, y])
            }
            (None, Some(_)) => {
                let x = self.value_expression(a, scope)?;
                let y = self.typed(b, ParameterType::Of(x.1), scope)?;
                Ok([x, y])
            }
            (Some(_), Some(_)) | (None, None) => Ok([self.value_expression(a, scope)?, self.value_expression(b, scope)?]),
        }
    }

    /// [X3.135-1986 §5.6] A `<value specification>`, which in a constraint
    /// is no authorization identifier [ISO/IEC 9075:1992 §11.9 SR5].
    pub(super) fn value_specification(&self, vs: &ValueSpecification) -> Result<(Value, DataType)> {
        if self.purpose.constrains() && matches!(vs, ValueSpecification::User(_)) {
            return Err(rule(
                "[ISO/IEC 9075:1992 §11.9 SR5] a CHECK condition shall not contain USER, CURRENT_USER, SESSION_USER or SYSTEM_USER",
            ));
        }
        if let ValueSpecification::Literal(Literal::Datetime(w)) = vs {
            return super::datetime::literal(w, self.now);
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
            ValueExpression::Monadic(sign, inner) => self.monadic(*sign, inner, scope),
            ValueExpression::Dyadic(left, op, right) => self.dyadic(left, *op, right, scope),
            ValueExpression::SetFunction(sf) => self.set_function(sf, scope).or_at(sf.at),
            ValueExpression::Call { name, args, at } => self.call(name, args, scope).or_at(*at),
            ValueExpression::Function { name, args, at } => self.function(name, args, scope).or_at(*at),
            ValueExpression::Case(case) => self.case(case, scope),
            ValueExpression::Cast { operand, target, at } => self.cast(operand.as_deref(), target, scope).or_at(*at),
            ValueExpression::Subquery(s) => self.scalar_subquery(s, scope),
            ValueExpression::Datetime(f) => Ok(self.datetime_function(*f)),
            ValueExpression::Extract { field, source, at } => self.extract(*field, source, scope).or_at(*at),
            ValueExpression::Difference { minuend, subtrahend, qualifier, at } => {
                self.difference((minuend, subtrahend), *qualifier, scope).or_at(*at)
            }
            ValueExpression::AtZone { source, zone, at } => self.at_zone(source, zone.as_deref(), scope).or_at(*at),
        }
    }

    /// [ISO/IEC 9075:1992 §6.6, §6.7, §6.13] A string function or
    /// concatenation: the function registered under `name`, its operands'
    /// types checked by it.
    fn function(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let f = self.db.functions.scalar(name).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {name} is not registered")))?;
        let (exprs, types) = self.arguments(&*f.0, args, scope)?;
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// The arguments `args` of the scalar function `f`, bound: a `<dynamic
    /// parameter specification>` among them taking the type `f` gives it,
    /// the others bound first [ISO/IEC 9075:1992 §17.6 GR5b-d], refused
    /// where it gives none \[GR4\].
    fn arguments(&self, f: &dyn ScalarFunction, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Vec<Expr>, Vec<DataType>)> {
        let mut bound = Vec::new();
        for a in args {
            bound.push(if dynamic_of(a).is_some() { None } else { Some(self.value_expression(a, scope)?) });
        }
        let known: Vec<Option<DataType>> = bound.iter().map(|b| b.as_ref().map(|(_, t)| *t)).collect();
        let mut out = Vec::new();
        for (position, (slot, a)) in bound.into_iter().zip(args).enumerate() {
            out.push(match (slot, f.parameter_type(position, &known)) {
                (Some(done), _) => done,
                (None, Some(taken)) => self.typed(a, taken, scope)?,
                (None, None) => self.value_expression(a, scope)?,
            });
        }
        Ok(out.into_iter().unzip())
    }

    /// [X3.135-1986 §5.9 GR4] monadic plus or minus, folded into a constant operand.
    pub(super) fn monadic(&self, sign: Sign, inner: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(inner, scope)?;
        if t.is_character() {
            return Err(rule("[X3.135-1986 §5.9 SR3] a character string <primary> cannot have an operator"));
        }
        if t.is_bit() || t.is_datetime() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.12] {t} is no <numeric primary>")));
        }
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
        // [ISO/IEC 9075:1992 §17.6 GR5h] a `?` takes the other operand's
        // type. Without one, each is bound as it is, so a chain of operators
        // nests no deeper in the stack than it did.
        let [(a, left_type), (b, right_type)] = if dynamic_of(left).is_some() || dynamic_of(right).is_some() {
            self.pair((left, right), scope)?
        } else {
            [self.value_expression(left, scope)?, self.value_expression(right, scope)?]
        };
        if left_type.is_character() || right_type.is_character() {
            return Err(rule("[X3.135-1986 §5.9 SR3] character string values cannot be used with arithmetic operators"));
        }
        if left_type.is_bit() || right_type.is_bit() {
            return Err(rule("[ISO/IEC 9075:1992 §6.12] bit string values cannot be used with arithmetic operators"));
        }
        // [§6.14, §6.15] datetimes and intervals.
        if left_type.is_datetime() || right_type.is_datetime() || left_type.is_interval() || right_type.is_interval() {
            return self.temporal((a, left_type), op, (b, right_type));
        }
        let t = match (left_type, right_type) {
            (DataType::Exact { scale: s1, .. }, DataType::Exact { scale: s2, .. }) => exact_result_type(op, s1, s2)?,
            // [X3.135-1986 §5.9 SR5]
            _ => DataType::Approximate { precision: DOUBLE_PRECISION },
        };
        Ok((Expr::Arith(Box::new(a), op, Box::new(b)), t))
    }

    /// [X3.135-1986 §5.8] a `<set function specification>`.
    pub(super) fn set_function(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let func: AggregateRef = self
            .db
            .functions
            .aggregate(sf.name)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the set function {} is not registered", sf.name)))?;
        match &sf.arg {
            SetArg::CountStar => self.aggregate(func, None, false, scope),
            SetArg::Distinct(argument) => self.aggregate(func, Some(argument), true, scope),
            SetArg::All(argument) => self.aggregate(func, Some(argument), false, scope),
        }
    }

    /// \[EXTENSION\] A call by name of a registered scalar or aggregate function.
    pub(super) fn call(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
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
        self.aggregate(f, Some(arg), false, scope)
    }

    /// Bind a set function [X3.135-1986 §5.8] or extension aggregate and register it with the grouping.
    pub(super) fn aggregate(
        &self,
        func: AggregateRef,
        arg: Option<&ValueExpression>,
        distinct: bool,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        // [X3.135-1986 §5.23 GR3] over an outer reference, it is the
        // enclosing query's (`binder/subquery.rs`).
        if let (Some(v), Some(outer)) = (arg, self.outer)
            && self.is_outer_argument(v)?
        {
            // [ISO/IEC 9075:1992 §6.5 SR4] the outer reference alone:
            // SQL-86's rule of no operators [X3.135-1986 §5.8 SR4] is
            // Entry SQL's [§6.5, Leveling Rules].
            let mut columns = Vec::new();
            column_specifications(v, &mut columns);
            if columns.len() > 1 {
                return Err(rule(
                    "[ISO/IEC 9075:1992 §6.5 SR4] an outer reference in a set function's argument is its only column reference",
                ));
            }
            return subquery::outer_aggregate(outer, func, v, distinct);
        }
        let g = match scope {
            Scope::Groups(g, _) => g,
            Scope::Rows(why) => return Err(rule(why)),
            Scope::Select => return Err(rule(NOT_AGGREGATED)),
        };
        let (arg, arg_type) = match arg {
            None => (None, None),
            Some(v) => {
                // [ISO/IEC 9075:1992 §6.5 SR4] no set function or subquery in
                // the argument; SQL-86's rule that it reference a column
                // [X3.135-1986 §5.8 SR4] is Entry SQL's alone [Leveling Rules].
                if v.any(&|e| matches!(e, ValueExpression::Subquery(_))) {
                    return Err(rule("[ISO/IEC 9075:1992 §6.5 SR4] the argument of a set function cannot contain a subquery"));
                }
                let (e, t) = self.value_expression(
                    v,
                    Scope::Rows("[ISO/IEC 9075:1992 §6.5 SR4] a set function argument cannot contain a set function"),
                )?;
                (Some(e), Some(t))
            }
        };
        let extreme = func.0.name() == kw::MAX || func.0.name() == kw::MIN;
        if self.purpose.constrains() && extreme && arg_type.is_some_and(|t| t.is_character()) {
            return Err(nondeterministic("§7.8 SR3a, §7.9 SR13c i", "MAX or MIN of a character string"));
        }
        let result = func.0.result_type(arg_type)?;
        let label = match &arg {
            None => format!("{}(*)", func.0.name()),
            Some(e) => format!("{}({}{})", func.0.name(), if distinct { "DISTINCT " } else { "" }, render_expr(e, &self.names)),
        };
        // The same set function written twice (say in SELECT and HAVING) is
        // computed once: the label names the function, argument and DISTINCT.
        let mut aggs = g.aggs.borrow_mut();
        let k = if let Some(k) = aggs.iter().position(|a| a.label == label) {
            k
        } else {
            aggs.push(AggCall { func, arg, distinct, result, label });
            aggs.len().saturating_sub(1)
        };
        Ok((Expr::Column(g.columns.len().saturating_add(k)), result))
    }
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
