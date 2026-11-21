//! \[DESIGN\] Whether an expression can raise an exception condition, judged
//! by the declared types of the columns it reads (RULES.md D373, D15).
//! `Expr::can_raise` takes every arithmetic operator and CAST as able to
//! raise. Of exact numbers whose declared precision and scale bound the
//! result within `MAX_EXACT_PRECISION` digits, none can: a sum or
//! difference of `(p1, s1)` and `(p2, s2)` has at most
//! `max(p1 - s1, p2 - s2) + 1` digits before the point and `max(s1, s2)`
//! after [X3.135-1986 §5.9 SR4b], a product `p1 + p2` digits in all
//! [SR4c], and an exact result raises only past those digits [GR6a]. A
//! quotient by a constant not zero, at least 1 in magnitude, keeps the
//! dividend's digits before the point and needs no digit past
//! `MAX_EXACT_PRECISION` after it while the constant's digits before the
//! point and the dividend's scale sum to no more [GR6b]. A CAST that widens
//! cannot raise either: to an exact type of no fewer digits before the
//! point [ISO/IEC 9075:1992 §6.10 GR3a ii], or to a character string type
//! at least as long as the longest literal of its operand's values [GR5a
//! iv, GR5b iii 4]. Anything else is judged as `Expr::can_raise` judges it,
//! and a column of no known type as of no exact type.

use super::{Cond, Expr, LogicalPlan, Slot};
use crate::catalog::Catalog;
use crate::value::{ArithOp, DataType, Decimal, MAX_EXACT_PRECISION, Value, constant_prefix};

/// The declared type of a column of the rows an expression is over, if
/// known.
pub type Types<'a> = &'a dyn Fn(usize) -> Option<DataType>;

/// \[DESIGN\] RULES.md R-CF-6: the deepest expression or plan judged; one
/// deeper is taken as able to raise, or of no known type.
pub const MAX_JUDGED_DEPTH: usize = 256;

/// The characters of an exact number's literal beyond its digits: a sign,
/// a period and the 0 before it of a number of no digit before the point
/// (`value/cast.rs`).
const LITERAL_EXTRA: u32 = 3;

/// Whether evaluating `e` can raise an exception condition, its columns of
/// the declared types `types` gives, as the module says.
pub fn can_raise_typed(e: &Expr, types: Types<'_>) -> bool {
    expr_raises(e, types, 0)
}

/// Whether evaluating `c` can raise an exception condition, its columns of
/// the declared types `types` gives: as `Cond::can_raise` judges it, each
/// expression judged by `can_raise_typed`.
pub fn cond_can_raise_typed(c: &Cond, types: Types<'_>) -> bool {
    cond_raises(c, types, 0)
}

/// `cond_can_raise_typed` at nesting `depth`.
fn cond_raises(c: &Cond, types: Types<'_>, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return true;
    }
    let deeper = depth.saturating_add(1);
    let own = match c {
        Cond::Like(_, Expr::Const(pattern), None) => constant_prefix(pattern, None).is_none(),
        Cond::Like(_, Expr::Const(pattern), Some(Expr::Const(escape))) => constant_prefix(pattern, Some(escape)).is_none(),
        Cond::Like(..) | Cond::Quantified(_, _, super::Quantifier::One, _) => true,
        Cond::Or(a, b) | Cond::And(a, b) => cond_raises(a, types, deeper) || cond_raises(b, types, deeper),
        Cond::Not(a) | Cond::Is(a, ..) => cond_raises(a, types, deeper),
        Cond::Const(_)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => false,
    };
    let mut raises = own || c.subqueries().iter().any(|s| s.can_raise());
    own_exprs(c, &mut |e| raises = raises || expr_raises(e, types, deeper));
    raises
}

/// Calls `f` with each expression `c` compares itself, not those of the
/// conditions it is made of, which `cond_raises` judges apart.
fn own_exprs<'a>(c: &'a Cond, f: &mut dyn FnMut(&'a Expr)) {
    match c {
        Cond::Or(..) | Cond::And(..) | Cond::Not(_) | Cond::Is(..) | Cond::Const(_) => {}
        Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::In(..)
        | Cond::Value(_)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. } => c.for_each_expr(f),
    }
}

/// `can_raise_typed` at nesting `depth`.
fn expr_raises(e: &Expr, types: Types<'_>, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return true;
    }
    let deeper = depth.saturating_add(1);
    let mut parts = false;
    e.for_each_child(&mut |child| parts = parts || expr_raises(child, types, deeper));
    match e {
        Expr::Arith(a, op, b) => parts || !arith_safe((a, *op, b), types, deeper),
        Expr::Cast(x, to) => parts || !widens(x, to, types, deeper),
        // A `?` cut by its cast warns as it is evaluated (RULES.md D391), as
        // `Expr::can_raise` says.
        Expr::Domain(..) | Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_) | Expr::Warned(..) => true,
        Expr::Call(f, _) => parts || (!f.0.raise_free() && (!f.0.index_predicate() || !f.0.deterministic())),
        Expr::Truth(c) => cond_raises(c, types, deeper),
        Expr::Case(whens, _) => parts || whens.iter().any(|(c, _)| cond_raises(c, types, deeper)),
        Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Neg(_) | Expr::SortKey(..) => parts,
    }
}

/// Whether `a op b`, its operands raising nothing, cannot raise.
fn arith_safe((a, op, b): (&Expr, ArithOp, &Expr), types: Types<'_>, depth: usize) -> bool {
    match op {
        ArithOp::Add | ArithOp::Sub | ArithOp::Mul => arith_shape((a, op, b), types, depth).is_some(),
        ArithOp::Div => {
            let (Some((_, scale)), Expr::Const(Value::Exact(divisor))) = (exact_shape(a, types, depth), b) else { return false };
            quotient_fits(scale, *divisor)
        }
    }
}

/// Whether a quotient of an exact dividend of scale `scale` by the constant
/// `divisor` always fits, as the module says.
fn quotient_fits(scale: u32, divisor: Decimal) -> bool {
    let whole = whole_digits(divisor);
    let at_least_one = divisor.units.unsigned_abs() >= magnitude(divisor.scale);
    at_least_one && whole.saturating_add(scale) <= MAX_EXACT_PRECISION
}

/// 10 to the power `n`, as the magnitude of `Decimal::units`, or the
/// largest it holds.
fn magnitude(n: u32) -> u128 {
    10_u128.checked_pow(n).unwrap_or(u128::MAX)
}

/// The digits of `d` before its point.
fn whole_digits(d: Decimal) -> u32 {
    d.digits().saturating_sub(d.scale)
}

/// Whether a CAST of `x` to `to` widens, as the module says.
fn widens(x: &Expr, to: &DataType, types: Types<'_>, depth: usize) -> bool {
    if let Some((p, s)) = exact_shape(x, types, depth) {
        if let DataType::Exact { precision, scale, .. } = to {
            return precision.saturating_sub(*scale) >= p.saturating_sub(s);
        }
        let (DataType::Character { length, .. } | DataType::Varying { length, .. }) = to else { return false };
        return *length >= p.saturating_add(LITERAL_EXTRA);
    }
    let (DataType::Character { length, units } | DataType::Varying { length, units }) = to else { return false };
    string_within(x, types).is_some_and(|(longest, of)| of == *units && longest <= *length)
}

/// Of a column of a character string type, its length and what it counts.
fn string_within(x: &Expr, types: Types<'_>) -> Option<(u32, crate::value::CharUnits)> {
    let Expr::Column(c) = x else { return None };
    let (DataType::Character { length, units } | DataType::Varying { length, units }) = types(*c)? else { return None };
    Some((length, units))
}

/// The precision and scale `e`'s exact values are within, if it is of an
/// exact type whose values the module's rules bound: a column of one, an
/// exact constant, a negation, or a sum, difference or product within
/// `MAX_EXACT_PRECISION` digits; or a CAST to one.
fn exact_shape(e: &Expr, types: Types<'_>, depth: usize) -> Option<(u32, u32)> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    match e {
        Expr::Column(c) => {
            let DataType::Exact { precision, scale, .. } = types(*c)? else { return None };
            Some((precision, scale))
        }
        Expr::Const(Value::Exact(d)) => Some((d.digits().max(d.scale), d.scale)),
        Expr::Neg(x) => exact_shape(x, types, deeper),
        // A slot's value is of its type, cast to it as it is filled or
        // assigned (RULES.md D613).
        Expr::Cast(_, DataType::Exact { precision, scale, .. })
        | Expr::Dynamic(Slot { data_type: DataType::Exact { precision, scale, .. }, .. }) => Some((*precision, *scale)),
        Expr::Arith(a, op, b) => arith_shape((a, *op, b), types, deeper),
        Expr::Const(_)
        | Expr::Cast(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Warned(..) => None,
    }
}

/// The precision and scale of `a op b`'s values, if exact and within
/// `MAX_EXACT_PRECISION` digits: of a sum or difference, or a product.
fn arith_shape((a, op, b): (&Expr, ArithOp, &Expr), types: Types<'_>, depth: usize) -> Option<(u32, u32)> {
    let ((pa, sa), (pb, sb)) = (exact_shape(a, types, depth)?, exact_shape(b, types, depth)?);
    let (precision, scale) = match op {
        ArithOp::Add | ArithOp::Sub => {
            let scale = sa.max(sb);
            let whole = pa.saturating_sub(sa).max(pb.saturating_sub(sb)).saturating_add(1);
            (whole.saturating_add(scale), scale)
        }
        ArithOp::Mul => (pa.saturating_add(pb), sa.saturating_add(sb)),
        ArithOp::Div => return None,
    };
    (precision <= MAX_EXACT_PRECISION).then_some((precision, scale))
}

/// The declared type of column `column` of `plan`'s rows, if known: of a
/// base table's column, through filters, products, outer joins and
/// projections of columns, or of a table value constructor's.
pub fn plan_column_type(plan: &LogicalPlan, column: usize, catalog: &dyn Catalog) -> Option<DataType> {
    column_type(plan, column, catalog, 0)
}

/// `plan_column_type` at nesting `depth`.
fn column_type(plan: &LogicalPlan, column: usize, catalog: &dyn Catalog, depth: usize) -> Option<DataType> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Scan { table, .. } => catalog.table(table)?.columns.get(column).map(|c| c.data_type.clone()),
        LogicalPlan::Filter { input, .. } | LogicalPlan::SemiJoin { left: input, .. } => column_type(input, column, catalog, deeper),
        LogicalPlan::MarkJoin(m) => match column.checked_sub(m.left.width()) {
            Some(_) => Some(DataType::Boolean),
            None => column_type(&m.left, column, catalog, deeper),
        },
        LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
            Some(at) => column_type(right, at, catalog, deeper),
            None => column_type(left, column, catalog, deeper),
        },
        LogicalPlan::Project { exprs, input } => {
            let Some(Expr::Column(c)) = exprs.get(column) else { return None };
            column_type(input, *c, catalog, deeper)
        }
        LogicalPlan::Values { types, .. } => types.get(column).cloned(),
        LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..) => None,
    }
}

#[cfg(test)]
mod tests;
