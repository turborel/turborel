//! \[DESIGN\] The fraction of rows a condition keeps, its selectivity
//! (RULES.md D31): from the table's statistics, which its engine gives as
//! far as its layout tells it (`stats.rs`), and from defaults for what
//! they do not answer. Conjuncts and disjuncts are taken as independent.

use crate::logical::{Cond, Expr, Truth};
use crate::stats::Statistics;
use crate::value::{CompOp, Value};

use super::CostModel;
use super::access::probe_candidate;

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
    let (stats, guess) = (known.stats, known.cost);
    let fraction = match cond {
        Cond::Const(Truth::True) => 1.0_f64,
        Cond::Const(Truth::False | Truth::Unknown) => 0.0_f64,
        Cond::And(a, b) => selectivity(a, known) * selectivity(b, known),
        Cond::Or(a, b) => {
            let (a, b) = (selectivity(a, known), selectivity(b, known));
            a + b - a * b
        }
        Cond::Not(c) => 1.0_f64 - selectivity(c, known),
        // A pattern restricts as an order comparison does, by default; a
        // subquery's result is not known before it runs.
        Cond::Like(..) | Cond::Exists(_) | Cond::Unique(_) | Cond::Match { .. } | Cond::Is(..) => guess.range_selectivity,
        Cond::IsNull(e, not) => {
            let column = if let Expr::Column(c) = e { Some(*c) } else { None };
            let nulls = column.and_then(|c| stats?.nulls(c)).unwrap_or(guess.null_fraction);
            if *not { 1.0_f64 - nulls } else { nulls }
        }
        Cond::Quantified(_, op, _, _) => default_compare(*op, guess),
        // The candidate's operator has the column on its left.
        Cond::Compare(_, op, _) => match probe_candidate(cond) {
            Some((column, flipped, value)) => compare((column, flipped, &value), known),
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
