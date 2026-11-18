//! Narrow type-based proofs for expressions skipped by distance pruning.
//! Ordinary calls keep their D15 error behavior; a stored fixed-reference
//! geometry is a valid input to the declared emptiness observer.

use crate::catalog::TableDesc;
use crate::geometry::{SpatialOperation, fixed_srid};
use crate::logical::{Cond, Expr};
use crate::physical::estimate::{self, Known};
use crate::value::{CompOp, Value};
use crate::vector::{Element, Layout, fixed_shape, is_built_in, shape_of};

pub(super) fn projection(exprs: &[Expr], measure: usize, table: &TableDesc) -> bool {
    exprs.iter().enumerate().all(|(at, e)| at == measure || expression(e, table))
}

fn expression(expr: &Expr, table: &TableDesc) -> bool {
    if !expr.can_raise() {
        return true;
    }
    let Expr::Call(function, args) = expr else { return false };
    if rechecked(function, args) && vectors_rechecked(expr, table) {
        return true;
    }
    let [Expr::Column(column)] = args.as_slice() else { return false };
    let geometry = table.columns.get(*column).is_some_and(|c| fixed_srid(&c.data_type).is_some());
    geometry && function.0.deterministic() && function.0.spatial_operation() == Some(SpatialOperation::IsEmpty)
}

/// A call that raises only of its arguments other than the column, alike
/// for every row of a value, and of none of a null (D87): a row the
/// pruning skips would have raised as the rows it reads do.
fn rechecked(function: &crate::functions::ScalarRef, args: &[Expr]) -> bool {
    let f = &function.0;
    let plain = args.iter().all(|a| matches!(a, Expr::Column(_) | Expr::Const(_)));
    f.rechecked_errors() && f.deterministic() && plain
}

/// \[EXTENSION\] Of a call measuring vectors, whether it raises alike for
/// every row, as `rechecked_errors` promises: a vector distance of a
/// built-in metric, which never fails of finite vectors of one shape, its
/// metric a constant, and each vector column of a fixed shape, dense or
/// sparse, and no FLOAT64, whose squares could overflow; then only a constant operand
/// of another shape raises, and for every row (D-APPROX). A call of no
/// vector column is not one.
fn vectors_rechecked(call: &Expr, table: &TableDesc) -> bool {
    let Expr::Call(_, args) = call else { return true };
    let column_type = |e: &Expr| if let Expr::Column(c) = e { table.columns.get(*c).map(|c| &c.data_type) } else { None };
    let vectors: Vec<&crate::value::DataType> = args.iter().filter_map(column_type).filter(|t| shape_of(t).is_some()).collect();
    if vectors.is_empty() {
        return true;
    }
    let fixed = vectors.iter().all(|t| fixed_shape(t).or_else(|| sparse_shape(t)).is_some_and(|(_, element)| element != Element::Float64));
    fixed && super::vector_measure(call).is_some_and(|(metric, _)| is_built_in(&metric))
}

/// \[EXTENSION\] Of a SPARSE type of fixed dimensions and format, those:
/// a built-in metric measures two such vectors as it does dense ones, of
/// the entries either holds (§10.3, R6.4).
fn sparse_shape(t: &crate::value::DataType) -> Option<(u32, Element)> {
    let s = shape_of(t)?;
    if s.layout? == Layout::Sparse { Some((s.dims?, s.element?)) } else { None }
}

pub(super) fn condition(cond: &Cond, table: &TableDesc) -> bool {
    if !cond.can_raise() {
        return true;
    }
    match cond {
        Cond::Compare(a, _, b) | Cond::Distinct(a, b) => expression(a, table) && expression(b, table),
        Cond::IsNull(e, _) => expression(e, table),
        Cond::And(a, b) | Cond::Or(a, b) => condition(a, table) && condition(b, table),
        Cond::Not(c) | Cond::Is(c, ..) => condition(c, table),
        Cond::Const(_)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => false,
    }
}

/// No empty-geometry statistic exists yet. Guess emptiness as a range
/// predicate, its negation as the complementary share, excluding NULLs;
/// generic equality's one-in-a-hundred estimate is unsuitable here.
pub(super) fn selectivity(cond: &Cond, known: Known<'_>) -> f64 {
    match cond {
        Cond::And(a, b) => selectivity(a, known) * selectivity(b, known),
        Cond::Or(a, b) => {
            let (a, b) = (selectivity(a, known), selectivity(b, known));
            a + b - a * b
        }
        Cond::Compare(a, CompOp::Eq, b) => {
            emptiness(a, b, known).or_else(|| emptiness(b, a, known)).unwrap_or_else(|| estimate::selectivity(cond, known))
        }
        Cond::Const(_)
        | Cond::Not(_)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::Distinct(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Is(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::Value(_)
        | Cond::In(..) => estimate::selectivity(cond, known),
    }
}

fn emptiness(call: &Expr, value: &Expr, known: Known<'_>) -> Option<f64> {
    let (Expr::Call(function, args), Expr::Const(Value::Exact(value))) = (call, value) else { return None };
    let [Expr::Column(column)] = args.as_slice() else { return None };
    if function.0.spatial_operation() != Some(SpatialOperation::IsEmpty) || !function.0.deterministic() || value.scale != 0 {
        return None;
    }
    let fraction = match value.units {
        0 => 1.0_f64 - known.cost.range_selectivity,
        1 => known.cost.range_selectivity,
        _ => return Some(0.0_f64),
    };
    let nulls = known.stats.and_then(|s| s.nulls(*column)).unwrap_or(known.cost.null_fraction);
    Some(fraction * (1.0_f64 - nulls))
}
