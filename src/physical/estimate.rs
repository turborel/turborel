//! \[DESIGN\] The fraction of rows a condition keeps, its selectivity
//! (RULES.md D31): from the table's statistics, which its engine gives as
//! far as its layout tells it (`stats.rs`), and from defaults for what
//! they do not answer. A conjunction is estimated as one
//! (`estimate/conjunction.rs`, RULES.md D382): its conjuncts on one column
//! as a range, equalities an index's key answers by its distinct keys,
//! conjuncts that go together on the table's sample of rows
//! (`estimate/sampled.rs`), the rest by exponential backoff
//! (`estimate/backoff.rs`). A LIKE of a prefix is a range
//! (`estimate/like.rs`). Disjuncts are taken as independent.

mod backoff;
mod conjunction;
mod fingerprint;
mod joint;
mod like;
mod sampled;
mod valued;

pub use conjunction::conjunction;
pub(super) use valued::{estimated, filled, valued};

use crate::logical::{Cond, Expr, Truth, compared_with_constant, split_and};
use crate::stats::Statistics;
use crate::value::{CompOp, Value};

use super::CostModel;

/// What an estimate knows: a table's statistics, if any, and the cost
/// model's guesses for what they do not tell.
#[derive(Clone, Copy)]
pub struct Known<'a> {
    /// The table's statistics.
    pub stats: Option<&'a dyn Statistics>,
    /// The guesses.
    pub cost: &'a CostModel,
}

/// The fraction of rows `cond`, over a table's columns, keeps, by what is
/// `known`.
pub fn selectivity(cond: &Cond, known: Known<'_>) -> f64 {
    // A slot read as its value, of a plan made with it (`valued.rs`).
    if let Some(filled) = filled(cond) {
        return of_cond(&filled, known);
    }
    of_cond(cond, known)
}

/// `selectivity`, of `cond` as it is.
fn of_cond(cond: &Cond, known: Known<'_>) -> f64 {
    let (stats, guess) = (known.stats, known.cost);
    let fraction = match cond {
        Cond::Const(Truth::True) => 1.0_f64,
        Cond::Const(Truth::False | Truth::Unknown) => 0.0_f64,
        Cond::And(..) => {
            let mut conjuncts = Vec::new();
            split_and(cond.clone(), &mut conjuncts);
            conjunction(&conjuncts, known)
        }
        Cond::Or(a, b) => {
            let (a, b) = (selectivity(a, known), selectivity(b, known));
            a + b - a * b
        }
        Cond::Not(c) => 1.0_f64 - selectivity(c, known),
        // A pattern of a prefix restricts to its range; any other as an
        // order comparison does, by default.
        Cond::Like(..) => like::selectivity(cond, known).unwrap_or(guess.range_selectivity),
        // A subquery's result is not known before it runs.
        Cond::Exists(_) | Cond::Unique(..) | Cond::Match { .. } | Cond::Is(..) | Cond::Value(_) => guess.range_selectivity,
        Cond::IsNull(e, not) => {
            let column = if let Expr::Column(c) = e { Some(*c) } else { None };
            let nulls = column.and_then(|c| stats?.nulls(c)).unwrap_or(guess.null_fraction);
            if *not { 1.0_f64 - nulls } else { nulls }
        }
        Cond::Quantified(_, op, _, _) => default_compare(*op, guess),
        // Distinct where an equality would not hold, so IS NOT DISTINCT
        // FROM, its negation, keeps what an equality does.
        Cond::Distinct(..) => 1.0_f64 - guess.equal_selectivity,
        // An OR of equalities, taken as each other's exclusion.
        Cond::In(e, set) => set
            .values()
            .iter()
            .map(|v| if let Expr::Column(c) = e { compare((*c, CompOp::Eq, v), known) } else { guess.equal_selectivity })
            .sum(),
        // The candidate's operator has the column on its left.
        Cond::Compare(_, op, _) => match compared_with_constant(cond) {
            Some((column, flipped, value)) => compare((column, flipped, value), known),
            None => default_compare(*op, guess),
        },
    };
    fraction.clamp(0.0, 1.0)
}

/// The fraction of rows for which "`column` `op` `value`" is true, by what
/// is `known`.
pub fn compare((column, op, value): (usize, CompOp, &Value), known: Known<'_>) -> f64 {
    known.stats.and_then(|s| s.selectivity(column, op, value)).unwrap_or_else(|| default_compare(op, known.cost)).clamp(0.0, 1.0)
}

/// The fraction a comparison keeps when nothing is known of its values.
fn default_compare(op: CompOp, guess: &CostModel) -> f64 {
    match op {
        CompOp::Eq => guess.equal_selectivity,
        CompOp::Ne => 1.0_f64 - guess.equal_selectivity,
        CompOp::Lt | CompOp::Le | CompOp::Gt | CompOp::Ge => guess.range_selectivity,
    }
}

// Unit tests: a conjunction on the sample, by ranges and by backoff
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
