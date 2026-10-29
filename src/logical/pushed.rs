//! \[DESIGN\] The conditions of a filter a scan is told (RULES.md D31,
//! `TableStorage::scan_where`), as the executor tells them and the planner
//! prices them (`TableStorage::scan_share`). Split from exec.rs (RULES.md
//! R-SIZE-2).

use super::{Cond, Expr};
use crate::index::Probe;
use crate::storage::{Condition, Conjunct, Conjunction};
use crate::value::{CompOp, Value};

/// The conjuncts of `cond` comparing a column with a constant, testing it
/// IS NULL or IS NOT NULL, or IN a list of constants, if any, unless `cond`
/// can raise an error: every row `cond` keeps satisfies each, so a scan
/// tells its engine (`TableStorage::scan_where`), which may then leave out
/// the rows they do not match, skip the frozen chunks one cannot match, and
/// count only the rows they all match as read.
pub fn pushed_conjuncts(cond: &Cond) -> Option<Conjunction> {
    // A condition that can raise an error must see every row: a row the
    // conjunct rejects may still raise it, and whether it does may not
    // depend on the plan, nor on which rows another transaction changed.
    if cond.can_raise() {
        return None;
    }
    let mut found = Vec::new();
    conjuncts(cond, &mut found);
    if found.is_empty() {
        return None;
    }
    Some(Conjunction(found))
}

/// The conjuncts of `cond` a scan is told (`pushed_conjuncts`).
fn conjuncts(cond: &Cond, found: &mut Vec<Conjunct>) {
    let compared = |column: usize, op: CompOp, value: &Value| {
        Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value: value.clone() } })
    };
    match cond {
        Cond::And(a, b) => {
            conjuncts(a, found);
            conjuncts(b, found);
        }
        Cond::Compare(Expr::Column(c), op, Expr::Const(v)) => found.push(compared(*c, *op, v)),
        // "constant op column" is "column op' constant", the operator turned around.
        Cond::Compare(Expr::Const(v), op, Expr::Column(c)) => found.push(compared(*c, op.flip(), v)),
        Cond::IsNull(Expr::Column(c), not) => found.push(Conjunct::Null { column: *c, not: *not }),
        // An `<in predicate>` of constants, as the binder writes it.
        Cond::Or(..) => found.extend(in_list(cond).map(|(column, values)| Conjunct::In { column, values })),
        Cond::Compare(..)
        | Cond::Not(_)
        | Cond::Const(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => {}
    }
}

/// The column and constant `cond` compares by `op`, either way round, the
/// operator turned around if the constant is first.
pub fn compared_with_constant(cond: &Cond) -> Option<(usize, CompOp, &Value)> {
    match cond {
        Cond::Compare(Expr::Column(c), op, Expr::Const(v)) => Some((*c, *op, v)),
        // "constant op column" is "column op' constant".
        Cond::Compare(Expr::Const(v), op, Expr::Column(c)) => Some((*c, op.flip(), v)),
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => None,
    }
}

/// The column and constants of `cond` if it is an OR of equalities of one
/// column with constants, as an `<in predicate>` of constants is
/// (`binder/predicates.rs`).
pub fn in_list(cond: &Cond) -> Option<(usize, Vec<Value>)> {
    if let Cond::Or(a, b) = cond {
        let (c, mut list) = in_list(a)?;
        let (d, more) = in_list(b)?;
        list.extend(more);
        return (c == d).then_some((c, list));
    }
    let (c, op, v) = compared_with_constant(cond)?;
    (op == CompOp::Eq).then(|| (c, vec![v.clone()]))
}
