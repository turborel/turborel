//! \[DESIGN\] Predicate transitivity (RULES.md D35). Split from
//! optimizer.rs (RULES.md R-SIZE-2).

use super::{Rule, RuleContext};
use crate::logical::{Cond, Expr, LogicalPlan, and_all, split_and};
use crate::value::{CompOp, NormKey, Value};

/// See [`super::CONSTANT_FOLDING`].
pub const PREDICATE_TRANSITIVITY: &str = "predicate_transitivity";

/// Where a filter's conjuncts equate two columns and compare one of them
/// with a constant, adds the same comparison of the other: if `a = b` and
/// `a < 5` are true, so is `b < 5`, as equal values compare alike with any
/// value [X3.135-1986 §5.11 GR4, GR5]. The filter keeps the same rows, and
/// the added conjunct, a comparison of a column with a constant, raises no
/// error; but it can be pushed to `b`'s table, or answered by its index.
/// Runs before the conjuncts are pushed into a product's operands, while
/// they are still together.
pub struct PredicateTransitivity;

/// A comparison of a column with a constant, and the constant's form under
/// which identical values are equal (`Value::norm`).
type Compared = (usize, CompOp, Value, NormKey);

impl Rule for PredicateTransitivity {
    fn name(&self) -> &'static str {
        PREDICATE_TRANSITIVITY
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let equated: Vec<(usize, usize)> = conjuncts.iter().filter_map(equated).collect();
        let mut known: Vec<Compared> = conjuncts.iter().filter_map(compared).collect();
        let mut added = Vec::new();
        for (column, op, value, _) in known.clone() {
            for &(a, b) in &equated {
                let other = if a == column { b } else { a };
                if a != column && b != column {
                    continue;
                }
                let norm = value.norm();
                if !known.iter().any(|(c, o, _, n)| *c == other && *o == op && *n == norm) {
                    known.push((other, op, value.clone(), norm));
                    added.push(Cond::Compare(Expr::Column(other), op, Expr::Const(value.clone())));
                }
            }
        }
        if added.is_empty() {
            return None;
        }
        conjuncts.extend(added);
        and_all(conjuncts).map(|all| LogicalPlan::Filter { cond: all, input: input.clone() })
    }
}

/// The columns "column = column" equates, if two.
fn equated(c: &Cond) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    (a != b).then_some((*a, *b))
}

/// "column `<op>` constant", or "constant `<op>` column" turned around.
fn compared(c: &Cond) -> Option<Compared> {
    let Cond::Compare(left, op, right) = c else { return None };
    let (column, op, value) = match (left, right) {
        (Expr::Column(i), Expr::Const(v)) => (*i, *op, v),
        (Expr::Const(v), Expr::Column(i)) => (*i, op.flip(), v),
        (
            Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::Subquery(..),
            _,
        ) => return None,
    };
    Some((column, op, value.clone(), value.norm()))
}
