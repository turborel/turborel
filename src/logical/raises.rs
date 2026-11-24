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
//! dividend's digits before the point, and its scale leaves room for
//! them (RULES.md D697) [GR6b]. A CAST that widens
//! cannot raise either: to an exact type of no fewer digits before the
//! point [ISO/IEC 9075:1992 §6.10 GR3a ii], or to a character string type
//! at least as long as the longest literal of its operand's values [GR5a
//! iv, GR5b iii 4]. Anything else is judged as `Expr::can_raise` judges it,
//! and a column of no known type as of no exact type.
//!
//! A column a grouping computes has the bound its set function gives it
//! (`raises/totals.rs`, RULES.md D1060), and a subquery is judged by
//! whoever asks, who may know its plan better than `Subquery::can_raise`
//! does (`Judge`).

mod totals;

pub use totals::{set_function_type, total_fits};

use super::{Cond, Expr, LogicalPlan, Slot, Subquery};
use crate::catalog::Catalog;
use crate::value::{ArithOp, DataType, Decimal, MAX_EXACT_PRECISION, Value, bound_of, constant_prefix};

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

/// What a judgment reads beside the expression it judges.
#[derive(Clone, Copy)]
pub struct Judge<'a> {
    /// The declared types of the columns.
    pub types: Types<'a>,
    /// Whether a subquery can raise an exception condition as it runs for
    /// a row, its outer expressions apart, which are judged with the
    /// expression that holds them. Its second argument says the subquery
    /// stands for one row, which it raises of two [ISO/IEC 9075:1992 §7.11
    /// GR1].
    pub subquery: &'a dyn Fn(&Subquery, bool) -> bool,
}

/// How a subquery is judged where no more is known of it, of one row
/// taken as raising, of any as its plan raises (`Subquery::can_raise`).
fn subquery_raises(s: &Subquery, one: bool) -> bool {
    one || s.can_raise()
}

/// Whether evaluating `e` can raise an exception condition, its columns of
/// the declared types `types` gives, as the module says.
pub fn can_raise_typed(e: &Expr, types: Types<'_>) -> bool {
    expr_raises(e, &Judge { types, subquery: &subquery_raises }, 0)
}

/// `can_raise_typed`, each subquery judged as `judge` says.
pub fn can_raise_judged(e: &Expr, judge: &Judge<'_>) -> bool {
    expr_raises(e, judge, 0)
}

/// `cond_can_raise_typed`, each subquery judged as `judge` says.
pub fn cond_can_raise_judged(c: &Cond, judge: &Judge<'_>) -> bool {
    cond_raises(c, judge, 0)
}

/// Whether evaluating `c` can raise an exception condition, its columns of
/// the declared types `types` gives: as `Cond::can_raise` judges it, each
/// expression judged by `can_raise_typed`.
pub fn cond_can_raise_typed(c: &Cond, types: Types<'_>) -> bool {
    cond_raises(c, &Judge { types, subquery: &subquery_raises }, 0)
}

/// `cond_can_raise_typed` at nesting `depth`.
fn cond_raises(c: &Cond, judge: &Judge<'_>, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return true;
    }
    let deeper = depth.saturating_add(1);
    let (own, one) = match c {
        Cond::Like(_, Expr::Const(pattern), None) => (constant_prefix(pattern, None).is_none(), false),
        Cond::Like(_, Expr::Const(pattern), Some(Expr::Const(escape))) => (constant_prefix(pattern, Some(escape)).is_none(), false),
        Cond::Like(..) => (true, false),
        // A comparison with one row, which a subquery of two raises.
        Cond::Quantified(_, _, super::Quantifier::One, _) => (false, true),
        Cond::Or(a, b) | Cond::And(a, b) => (cond_raises(a, judge, deeper) || cond_raises(b, judge, deeper), false),
        Cond::Not(a) | Cond::Is(a, ..) => (cond_raises(a, judge, deeper), false),
        Cond::Const(_)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => (false, false),
    };
    let mut raises = own || c.subqueries().iter().any(|s| (judge.subquery)(s, one));
    own_exprs(c, &mut |e| raises = raises || expr_raises(e, judge, deeper));
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
fn expr_raises(e: &Expr, judge: &Judge<'_>, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return true;
    }
    let (deeper, types) = (depth.saturating_add(1), judge.types);
    let mut parts = false;
    e.for_each_child(&mut |child| parts = parts || expr_raises(child, judge, deeper));
    match e {
        Expr::Arith(a, op, b) => parts || !arith_safe((a, *op, b), types, deeper),
        Expr::Cast(x, to) => parts || !widens(x, to, types, deeper),
        // A `?` cut by its cast warns as it is evaluated (RULES.md D391), as
        // `Expr::can_raise` says.
        Expr::Domain(..) | Expr::NextValue(_) | Expr::Invoked(_) | Expr::Warned(..) => true,
        Expr::Subquery(s, _) => parts || (judge.subquery)(s, true),
        Expr::Call(f, _) => parts || (!f.0.raise_free() && (!f.0.index_predicate() || !f.0.deterministic())),
        Expr::Truth(c) => cond_raises(c, judge, deeper),
        Expr::Case(whens, _) => parts || whens.iter().any(|(c, _)| cond_raises(c, judge, deeper)),
        Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Neg(_) | Expr::SortKey(..) => parts,
    }
}

/// Whether `a op b`, its operands raising nothing, cannot raise.
fn arith_safe((a, op, b): (&Expr, ArithOp, &Expr), types: Types<'_>, depth: usize) -> bool {
    match op {
        ArithOp::Add | ArithOp::Sub | ArithOp::Mul => arith_shape((a, op, b), types, depth).is_some(),
        ArithOp::Div => {
            let (Some(_), Expr::Const(Value::Exact(divisor))) = (exact_shape(a, types, depth), b) else { return false };
            quotient_fits(*divisor)
        }
    }
}

/// Whether a quotient of an exact dividend by the constant `divisor`
/// always fits, as the module says: of a divisor at least 1 in magnitude
/// its integer digits are at most the dividend's, and its scale is as
/// many fewer as they need (`Decimal::checked_div`).
fn quotient_fits(divisor: Decimal) -> bool {
    divisor.units.unsigned_abs() >= magnitude(divisor.scale)
}

/// 10 to the power `n`, as the magnitude of `Decimal::units`, or the
/// largest it holds.
fn magnitude(n: u32) -> u128 {
    10_u128.checked_pow(n).unwrap_or(u128::MAX)
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
/// `MAX_EXACT_PRECISION` digits; a CAST to one; or a CASE of such results.
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
        Expr::Case(whens, otherwise) => case_shape(whens.iter().map(|(_, result)| result).chain([&**otherwise]), types, deeper),
        Expr::Const(_)
        | Expr::Cast(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_)
        | Expr::Warned(..) => None,
    }
}

/// The precision and scale the values of a CASE of `results` are within,
/// one of which it gives [ISO/IEC 9075:1992 §6.9 GR2]: the most digits
/// before the point and the most after it of any, a null constant apart,
/// which bounds nothing; none if a result is not bounded, or all are null.
fn case_shape<'a>(results: impl Iterator<Item = &'a Expr>, types: Types<'_>, depth: usize) -> Option<(u32, u32)> {
    let mut widest = None;
    for result in results.filter(|r| !matches!(r, Expr::Const(Value::Null))) {
        let (p, s) = exact_shape(result, types, depth)?;
        let (whole, scale) = widest.map_or((p.saturating_sub(s), s), |(w, held): (u32, u32)| (w.max(p.saturating_sub(s)), held.max(s)));
        widest = Some((whole, scale));
    }
    widest.map(|(whole, scale)| (whole.saturating_add(scale), scale)).filter(|(p, _)| *p <= MAX_EXACT_PRECISION)
}

/// The precision and scale of `a op b`'s values, if exact and within
/// `MAX_EXACT_PRECISION` digits: of a sum or difference, or a product.
fn arith_shape((a, op, b): (&Expr, ArithOp, &Expr), types: Types<'_>, depth: usize) -> Option<(u32, u32)> {
    let ((pa, sa), (pb, sb)) = (exact_shape(a, types, depth)?, exact_shape(b, types, depth)?);
    let (precision, scale) = bound_of(op, (pa, sa), (pb, sb))?;
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
        // A grouping column is its input's, and a set function's values
        // are within what its argument's bound them to.
        LogicalPlan::Aggregate { groups, aggs, seeds, input, .. } => {
            if !seeds.is_empty() {
                return None;
            }
            match column.checked_sub(groups.len()) {
                Some(n) => totals::result_type(aggs.get(n)?, &|c| column_type(input, c, catalog, deeper), deeper),
                None => column_type(input, *groups.get(column)?, catalog, deeper),
            }
        }
        LogicalPlan::Distinct { .. }
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

/// Whether a CAST of `x` to `to` can raise an exception or completion
/// condition: of a constant, if casting it does [ISO/IEC 9075:1992 §6.10
/// GR2-GR5]; of any other value, as its value is not known, it can.
pub(super) fn cast_raises(x: &Expr, to: &DataType) -> bool {
    let Expr::Const(v) = x else { return true };
    !matches!(crate::value::cast_warned(v.clone(), to), Ok((_, None)))
}

#[cfg(test)]
mod tests;
