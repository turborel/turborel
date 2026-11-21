//! [ISO/IEC 9075-2:2023 §8.20] A `<period predicate>` of two periods each
//! a `<period reference>`, or of one and a point in time, as the
//! comparisons of their bounds it is made of (RULES.md D581). Such a
//! predicate is True exactly when each comparison is: a comparison of a
//! null bound is Unknown, not True, and the predicate is then False, not
//! True [GR4], so as a conjunct of a join's condition, whose pairs are kept
//! only when it is True, it may be replaced by them. Its bounds are
//! columns of its operands by then (`optimizer/hoisting.rs`), so its
//! comparisons are a range join's (`physical/range.rs`) or a hash join's
//! equalities.

use crate::ast::PeriodOperator;
use crate::logical::{Cond, Expr};
use crate::value::CompOp;

/// `conjuncts`, each `<period predicate>` of them that a join may judge as
/// comparisons replaced by them.
pub(in crate::physical) fn expanded_periods(conjuncts: Vec<Cond>) -> Vec<Cond> {
    let mut out = Vec::with_capacity(conjuncts.len());
    for c in conjuncts {
        match expanded(&c) {
            Some(parts) => out.extend(parts),
            None => out.push(c),
        }
    }
    out
}

/// "`a op b`".
fn compared(a: &Expr, op: CompOp, b: &Expr) -> Cond {
    Cond::Compare(a.clone(), op, b.clone())
}

/// The comparisons `c` is made of, if it is a `<period predicate>` whose
/// function judges it so (`ScalarFunction::period_operator`): of periods
/// from S1 to E1 and from S2 to E2, [GR4b iv to ix]; of a point in time P
/// after CONTAINS, [GR4a v 2].
fn expanded(c: &Cond) -> Option<Vec<Cond>> {
    use CompOp::{Eq, Ge, Gt, Le, Lt};
    let Cond::Value(Expr::Call(function, args)) = c else { return None };
    let op = function.0.period_operator()?;
    if let (PeriodOperator::Contains, [s1, e1, p]) = (op, args.as_slice()) {
        return Some(vec![compared(s1, Le, p), compared(e1, Gt, p)]);
    }
    let [s1, e1, s2, e2] = args.as_slice() else { return None };
    Some(match op {
        PeriodOperator::Overlaps => vec![compared(s1, Lt, e2), compared(e1, Gt, s2)],
        PeriodOperator::Equals => vec![compared(s1, Eq, s2), compared(e1, Eq, e2)],
        PeriodOperator::Contains => vec![compared(s1, Le, s2), compared(e1, Ge, e2)],
        PeriodOperator::Precedes => vec![compared(e1, Le, s2)],
        PeriodOperator::Succeeds => vec![compared(s1, Ge, e2)],
        PeriodOperator::ImmediatelyPrecedes => vec![compared(e1, Eq, s2)],
        PeriodOperator::ImmediatelySucceeds => vec![compared(s1, Eq, e2)],
    })
}
