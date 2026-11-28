//! \[DESIGN\] Predicate transitivity (RULES.md D35, D404).

mod joined;

use super::{Rule, RuleContext};
use crate::logical::{Cond, Expr, LogicalPlan, and_all, split_and};
use crate::value::{CompOp, NormKey};

/// See [`super::CONSTANT_FOLDING`].
pub const PREDICATE_TRANSITIVITY: &str = "predicate_transitivity";

/// Where a filter's conjuncts equate two columns and compare one of them
/// with a constant or a `?`, adds the same comparison of the other: if
/// `a = b` and `a < 5` are true, so is `b < 5`, as equal values compare
/// alike with any value [X3.135-1986 §5.11 GR4, GR5]. The filter keeps the
/// same rows, and the added conjunct, a comparison of a column with a
/// constant, raises no error; but it can be pushed to `b`'s table, or
/// answered by its index. A `?` is one value in an execution, so it is
/// added as it is, the same place filled once, and a plan made with slots
/// gains what one made with the execution's values gains (RULES.md D1323).
/// Where two columns of different tables of a product are each equal to
/// one exact number or one `?`, it adds their equality, which a join may
/// probe an index by (`transitivity/joined.rs`). Runs before the
/// conjuncts are pushed into a product's operands, while they are still
/// together.
pub struct PredicateTransitivity;

/// What a comparison compares a column with, as two comparisons are found
/// to be of one value: a constant's form under which identical values are
/// equal (`Value::norm`), or a `?`'s place, which has one value in an
/// execution.
#[derive(Clone, PartialEq)]
enum Held {
    Norm(NormKey),
    Place(usize),
}

/// A comparison of a column with a constant or a `?`, the value, and what
/// it is held by.
type Compared = (usize, CompOp, Expr, Held);

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
        let mut added = joined::equated_by_values(&conjuncts, &equated, input);
        for (column, op, value, held) in known.clone() {
            for &(a, b) in &equated {
                let other = if a == column { b } else { a };
                if a != column && b != column {
                    continue;
                }
                if !known.iter().any(|(c, o, _, h)| *c == other && *o == op && *h == held) {
                    known.push((other, op, value.clone(), held.clone()));
                    added.push(Cond::Compare(Expr::Column(other), op, value.clone()));
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
pub(super) fn equated(c: &Cond) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    (a != b).then_some((*a, *b))
}

/// "column `<op>` constant" or "column `<op>` ?", or either turned around.
fn compared(c: &Cond) -> Option<Compared> {
    let Cond::Compare(left, op, right) = c else { return None };
    let (column, op, value, held) = match (left, right) {
        (Expr::Column(i), Expr::Const(v)) => (*i, *op, right, Held::Norm(v.norm())),
        (Expr::Const(v), Expr::Column(i)) => (*i, op.flip(), left, Held::Norm(v.norm())),
        (Expr::Column(i), Expr::Dynamic(slot)) => (*i, *op, right, Held::Place(slot.index)),
        (Expr::Dynamic(slot), Expr::Column(i)) => (*i, op.flip(), left, Held::Place(slot.index)),
        (
            Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::SortKey(..)
            | Expr::Domain(..)
            | Expr::Warned(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_),
            _,
        ) => return None,
    };
    Some((column, op, value.clone(), held))
}
