//! \[EXTENSION\] What a read's conjuncts say of a partition key's parts
//! (RULES.md D156): each part a column of the table, restricted by the
//! conjuncts that compare it with a constant, test it for null, list its
//! values with IN, or OR such tests of it together. A `?` not yet filled
//! restricts nothing, so a plan a prepared statement keeps reads every
//! partition its constants admit, and one made with an execution's values
//! reads only those they admit.

use std::cmp::Ordering;

use super::Restriction;
use crate::logical::{Cond, Expr};
use crate::value::{CompOp, DataType, Value, assign, compare};

/// What `conds` say of each part of a key whose parts are the columns
/// `parts` (none of an expression part), of `types`: one restriction per
/// part, none where nothing restricts it, an empty `Equal` where the
/// conjuncts contradict each other.
pub(crate) fn restrictions(conds: &[Cond], parts: &[Option<usize>], types: &[DataType]) -> Vec<Option<Restriction>> {
    parts
        .iter()
        .zip(types)
        .map(|(part, data_type)| part.and_then(|column| conds.iter().fold(None, |r, c| narrowed(r, of_cond(c, column, data_type)))))
        .collect()
}

/// What the conjunct `cond` says of the column `column`, of `data_type`.
fn of_cond(cond: &Cond, column: usize, data_type: &DataType) -> Option<Restriction> {
    match cond {
        Cond::Compare(a, op, b) => compared((a, *op, b), column, data_type),
        Cond::IsNull(Expr::Column(c), not) => (*c == column && !*not).then_some(Restriction::Null),
        Cond::In(Expr::Column(c), set) => {
            (*c == column).then(|| Restriction::Equal(set.values().iter().filter_map(|v| typed(v, data_type)).collect()))
        }
        Cond::Or(a, b) => either(of_cond(a, column, data_type)?, of_cond(b, column, data_type)?),
        Cond::IsNull(..)
        | Cond::In(..)
        | Cond::Const(_)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Distinct(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Is(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::Value(_) => None,
    }
}

/// What a comparison says of the column `column`: of the column with a
/// constant, either way round.
fn compared((a, op, b): (&Expr, CompOp, &Expr), column: usize, data_type: &DataType) -> Option<Restriction> {
    let (op, value) = if let (Expr::Column(c), Expr::Const(v)) = (a, b) {
        (*c == column).then_some((op, v))?
    } else if let (Expr::Const(v), Expr::Column(c)) = (a, b) {
        (*c == column).then_some((op.flip(), v))?
    } else {
        return None;
    };
    // A comparison with the null value is never true.
    let Some(value) = typed(value, data_type) else { return Some(Restriction::Equal(Vec::new())) };
    match op {
        CompOp::Eq => Some(Restriction::Equal(vec![value])),
        CompOp::Lt => Some(Restriction::Range { low: None, high: Some((value, false)) }),
        CompOp::Le => Some(Restriction::Range { low: None, high: Some((value, true)) }),
        CompOp::Gt => Some(Restriction::Range { low: Some((value, false)), high: None }),
        CompOp::Ge => Some(Restriction::Range { low: Some((value, true)), high: None }),
        CompOp::Ne => None,
    }
}

/// `v` as a value of the key part's type `data_type`: none of the null
/// value, or of one that is not assigned exactly so.
fn typed(v: &Value, data_type: &DataType) -> Option<Value> {
    if v.is_null() {
        return None;
    }
    let assigned = assign(v.clone(), data_type, "EXTENSION").ok()?;
    (compare(&assigned, v) == Some(Ordering::Equal)).then_some(assigned)
}

/// What both `a` and `b` say of one part.
fn narrowed(a: Option<Restriction>, b: Option<Restriction>) -> Option<Restriction> {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (a, b) => return b.or(a),
    };
    Some(match (a, b) {
        (Restriction::Null, Restriction::Null) => Restriction::Null,
        (Restriction::Null, _) | (_, Restriction::Null) => Restriction::Equal(Vec::new()),
        (Restriction::Equal(values), other) | (other, Restriction::Equal(values)) => {
            Restriction::Equal(values.into_iter().filter(|v| within(v, &other)).collect())
        }
        (Restriction::Range { low, high }, Restriction::Range { low: low2, high: high2 }) => {
            Restriction::Range { low: tighter(low, low2, Ordering::Greater), high: tighter(high, high2, Ordering::Less) }
        }
    })
}

/// What `a` or `b` says of one part: the values of both lists, else
/// nothing.
fn either(a: Restriction, b: Restriction) -> Option<Restriction> {
    let (Restriction::Equal(mut values), Restriction::Equal(more)) = (a, b) else { return None };
    values.extend(more);
    Some(Restriction::Equal(values))
}

/// Whether `v` meets `r`.
fn within(v: &Value, r: &Restriction) -> bool {
    match r {
        Restriction::Equal(values) => values.iter().any(|w| compare(v, w) == Some(Ordering::Equal)),
        Restriction::Null => false,
        Restriction::Range { low, high } => {
            let above = low.as_ref().is_none_or(|(l, inclusive)| meets(compare(v, l), Ordering::Greater, *inclusive));
            let below = high.as_ref().is_none_or(|(h, inclusive)| meets(compare(v, h), Ordering::Less, *inclusive));
            above && below
        }
    }
}

/// Whether a comparison's result is `wanted`, or equal if `inclusive`.
fn meets(found: Option<Ordering>, wanted: Ordering, inclusive: bool) -> bool {
    found == Some(wanted) || inclusive && found == Some(Ordering::Equal)
}

/// Of two ends, the one further `toward` the other end: the greater of two
/// lower ends, the less of two upper.
fn tighter(a: Option<(Value, bool)>, b: Option<(Value, bool)>, toward: Ordering) -> Option<(Value, bool)> {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (a, b) => return a.or(b),
    };
    let order = compare(&a.0, &b.0);
    if order == Some(Ordering::Equal) {
        return Some((a.0, a.1 && b.1));
    }
    Some(if order == Some(toward) { a } else { b })
}
