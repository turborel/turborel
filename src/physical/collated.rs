//! \[DESIGN\] (RULES.md D60, D1086) A join on keys computed of each
//! operand's columns. A hash join's keys are columns, so a condition
//! equating an expression of one operand's columns with one of the
//! other's, as the sort keys of a collation of a column of each, or
//! `Q.K = X.TAG + X.K`, has them made columns of its operands, computed
//! once a row, which a projection takes away after, and the join is
//! planned as any. An expression lifted is deterministic and holds no
//! subquery, so a row's key is the value the condition would compare of
//! it with every row of the other operand; and a join's condition that
//! can raise an exception condition by its columns' declared types is
//! not hashed at all (`join.rs`, RULES.md D776), so computing the keys of
//! rows no pair holds raises nothing.

use crate::logical::{Cond, Expr, LogicalPlan, and_all, expr_columns, map_cond_columns, map_expr_columns, split_and};
use crate::optimizer::unnesting::expr_pure;
use crate::value::CompOp;

/// A join whose sort keys are made columns of its operands.
pub(super) struct Lifted {
    /// Its condition, over the operands below.
    pub(super) cond: Cond,
    /// Its left operand, its sort keys after its columns.
    pub(super) left: LogicalPlan,
    /// Its right operand, likewise.
    pub(super) right: LogicalPlan,
    /// What gives the join's own columns of its pairs.
    pub(super) kept: Vec<Expr>,
}

/// The keys `c` equates, an expression of the columns of each operand,
/// the left `width` columns wide, not both a column alone (`join.rs`
/// keys those), the left's first, and whether a null matches a null of
/// them, as `c` is IS NOT DISTINCT FROM [ISO/IEC 9075-2:2023 §8.15]. A
/// sort key of a null is null, so the keys match as the strings.
fn keys_of(c: &Cond, width: usize) -> Option<(Expr, Expr, bool)> {
    let (first, second, nulls) = match c {
        Cond::Compare(first, CompOp::Eq, second) => (first, second, false),
        Cond::Not(inner) => {
            let Cond::Distinct(first, second) = &**inner else { return None };
            (first, second, true)
        }
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Is(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => return None,
    };
    if matches!((first, second), (Expr::Column(_), Expr::Column(_))) {
        return None;
    }
    match (of_left(first, width)?, of_left(second, width)?) {
        (true, false) => Some((first.clone(), second.clone(), nulls)),
        (false, true) => Some((second.clone(), first.clone(), nulls)),
        (true, true) | (false, false) => None,
    }
}

/// Of `e`, a deterministic expression of no subquery that reads a column,
/// whether it reads the left operand's alone, `width` wide, or the
/// right's alone; none if neither, or if it is not one to compute once a
/// row.
fn of_left(e: &Expr, width: usize) -> Option<bool> {
    if !e.all_subqueries().is_empty() || !expr_pure(e) {
        return None;
    }
    let mut read = Vec::new();
    expr_columns(e, &mut read);
    let (&least, &most) = (read.iter().min()?, read.iter().max()?);
    if most < width { Some(true) } else { (least >= width).then_some(false) }
}

/// The condition matching left column `l` with right column `r`: equal,
/// or if `nulls`, not distinct.
fn matching(l: usize, r: usize, nulls: bool) -> Cond {
    if nulls {
        Cond::Not(Box::new(Cond::Distinct(Expr::Column(l), Expr::Column(r))))
    } else {
        Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(r))
    }
}

/// `e`, the sort key of a column of the right operand, of its own columns.
fn of_right(e: Expr, width: usize) -> Expr {
    map_expr_columns(e, &|c| c.saturating_sub(width))
}

/// `plan` with the columns `keys` after its own.
fn with_keys(plan: &LogicalPlan, keys: Vec<Expr>) -> LogicalPlan {
    let exprs = (0..plan.width()).map(Expr::Column).chain(keys).collect();
    LogicalPlan::Project { exprs, input: Box::new(plan.clone()) }
}

/// The join of `left` and `right` on `cond` as `Lifted`, if `cond` equates
/// keys computed of the columns of each (`keys_of`).
pub(super) fn lifted(cond: &Cond, left: &LogicalPlan, right: &LogicalPlan) -> Option<Lifted> {
    let (width, right_width) = (left.width(), right.width());
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut lefts, mut rights, mut nulls, mut rest) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for c in conjuncts {
        match keys_of(&c, width) {
            Some((l, r, matches_null)) => {
                lefts.push(l);
                rights.push(of_right(r, width));
                nulls.push(matches_null);
            }
            None => rest.push(c),
        }
    }
    if lefts.is_empty() {
        return None;
    }
    let count = lefts.len();
    let wider = width.saturating_add(count);
    let moved = move |c: usize| if c < width { c } else { c.saturating_add(count) };
    let equal = nulls
        .into_iter()
        .enumerate()
        .map(|(k, matches_null)| matching(width.saturating_add(k), wider.saturating_add(right_width).saturating_add(k), matches_null));
    let conds: Vec<Cond> = equal.chain(rest.into_iter().map(|c| map_cond_columns(c, &moved))).collect();
    let kept = (0..width).chain(wider..wider.saturating_add(right_width)).map(Expr::Column).collect();
    Some(Lifted { cond: and_all(conds)?, left: with_keys(left, lefts), right: with_keys(right, rights), kept })
}

// Unit tests: the keys a join's condition equates that are computed of
// each operand's columns (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::keys_of;
    use crate::catalog::{QualifiedName, SequenceDesc};
    use crate::logical::{Cond, Expr};
    use crate::value::{ArithOp, CompOp, DataType, Spelled};

    /// `a = b`.
    fn equal(a: Expr, b: Expr) -> Cond {
        Cond::Compare(a, CompOp::Eq, b)
    }

    /// The sum of columns `a` and `b`.
    fn sum(a: usize, b: Expr) -> Expr {
        Expr::Arith(Box::new(Expr::Column(a)), ArithOp::Add, Box::new(b))
    }

    /// The keys `c` equates, the left operand two columns wide, as text.
    fn keys(c: &Cond) -> String {
        format!("{:?}", keys_of(c, 2))
    }

    /// Verifies: [DESIGN] RULES.md D1086: an equality of an expression of
    /// the left's columns with one of the right's gives its keys, the
    /// left's first, whichever side the condition writes it; not of two
    /// columns alone, which the join keys as they are, nor of an
    /// expression of both operands' columns, nor of one that is not
    /// deterministic, as NEXT VALUE FOR, whose value a pair may not share.
    #[test]
    fn keys_computed_of_each_operands_columns_are_equated() {
        let computed = || sum(2, Expr::Column(3));
        let expected = format!("{:?}", Some((Expr::Column(0), computed(), false)));
        assert_eq!(keys(&equal(Expr::Column(0), computed())), expected);
        assert_eq!(keys(&equal(computed(), Expr::Column(0))), expected, "written right first");
        assert_eq!(keys(&equal(Expr::Column(0), Expr::Column(2))), "None", "columns alone");
        assert_eq!(keys(&equal(sum(0, Expr::Column(2)), Expr::Column(3))), "None", "of both operands");
        let name = QualifiedName { auth: "A".into(), ident: "S".into() };
        let data_type = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
        let sequence = SequenceDesc {
            name,
            data_type,
            start: 1,
            minimum: 1,
            maximum: 9,
            increment: 1,
            cycle: false,
            epoch: 0,
            origin: 1,
            usage: Vec::new(),
        };
        let drawn = sum(2, Expr::NextValue(Arc::new(sequence)));
        assert_eq!(keys(&equal(Expr::Column(0), drawn)), "None", "not deterministic");
    }
}
