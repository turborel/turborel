//! \[DESIGN\] (RULES.md D650) The order a filter judges its conjuncts in:
//! ascending by rank, a conjunct's cost a row over the share of rows it
//! drops, as Hellerstein and Stonebraker, "Predicate Migration", SIGMOD
//! 1993, §2, order the predicates of a selection. The batch filter passes
//! each conjunct only the rows the one before kept (RULES.md D354), so a
//! cheap conjunct dropping many rows first spares the rest of them. A
//! filter that can raise an error keeps its order: each of its conjuncts
//! is judged of every row anyway (RULES.md D15), and the error it raises
//! first stays the one its query's text gives first.

#[cfg(test)]
mod tests;

use crate::logical::{Cond, compared_with_constant};
use crate::physical::estimate::{Known, selectivity};

/// \[DESIGN\] The cost a row of a column compared with a constant, or
/// tested for null: one kernel's pass over a vector (`bench/analytics/costs.rs`,
/// its conjunct judged).
const SIMPLE: f64 = 1.0;
/// \[DESIGN\] The cost a row of a set of constants searched, a hash and a
/// probe of its table.
const SEARCHED: f64 = 2.0;
/// \[DESIGN\] The cost a row of a comparison of expressions, evaluated a
/// vector at a time by a program (RULES.md D362).
const EVALUATED: f64 = 3.0;
/// \[DESIGN\] The cost a row of a pattern matched, the text walked.
const MATCHED: f64 = 4.0;
/// \[DESIGN\] The cost a row of a conjunct with a subquery, which may run
/// it for each row: past every other, so it is judged last.
const SUBQUERY: f64 = 1_000.0;
/// \[DESIGN\] The least share of rows a conjunct is taken to drop, so a
/// conjunct estimated to keep every row ranks by its cost, not at infinity.
const LEAST_DROPPED: f64 = 1e-6;

/// `conds`, the conjuncts of one filter, in the order it judges them best:
/// ascending by rank, ties as given; as given if any can raise an error.
pub(in crate::physical) fn ranked(conds: Vec<Cond>, known: Known<'_>) -> Vec<Cond> {
    if conds.len() < 2 || conds.iter().any(Cond::can_raise) {
        return conds;
    }
    let mut ranked: Vec<(f64, Cond)> = conds.into_iter().map(|c| (rank(&c, known), c)).collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    ranked.into_iter().map(|(_, c)| c).collect()
}

/// The rank of `cond`: its cost a row over the share of rows it drops.
pub(in crate::physical) fn rank(cond: &Cond, known: Known<'_>) -> f64 {
    let dropped = (1.0_f64 - selectivity(cond, known)).max(LEAST_DROPPED);
    cost(cond) / dropped
}

/// The cost a row of judging `cond`, in its simplest conjunct's.
pub(in crate::physical) fn cost(cond: &Cond) -> f64 {
    if cond.has_subquery() {
        return SUBQUERY;
    }
    match cond {
        Cond::Const(_) => 0.0_f64,
        Cond::And(a, b) | Cond::Or(a, b) => cost(a) + cost(b),
        Cond::Not(c) | Cond::Is(c, ..) => cost(c),
        Cond::Compare(..) => {
            if compared_with_constant(cond).is_some() {
                SIMPLE
            } else {
                EVALUATED
            }
        }
        Cond::IsNull(e, _) | Cond::Value(e) => {
            if matches!(e, crate::logical::Expr::Column(_)) {
                SIMPLE
            } else {
                EVALUATED
            }
        }
        Cond::Distinct(..) => EVALUATED,
        Cond::In(..) => SEARCHED,
        Cond::Like(..) => MATCHED,
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(..) | Cond::Match { .. } => SUBQUERY,
    }
}
