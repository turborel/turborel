//! \[DESIGN\] A conjunction estimated on the table's sample of rows
//! (RULES.md D382). Of a table of at least `SAMPLED_FROM` rows whose
//! statistics keep a sample, a conjunction of two or more conjuncts over
//! two or more columns, or with a conjunct no synopsis answers (a LIKE
//! with no prefix, a function of a column, an OR across columns), is
//! evaluated on the sample's rows: k of n kept give k/n when k is at
//! least `SAMPLE_HITS`; fewer say only that the share is small, and the
//! estimate is the synopses', at most (k + `EMPTY_BOUND`)/n, the bound an
//! empty sample allows (Hertzschuch et al., "Small selectivities
//! matter", SIGMOD 2021).
//!
//! Only conjuncts the sample can tell are evaluated: deterministic, of
//! no subquery, routine, sequence value or `?` not yet given, and of
//! the table's own columns. A row whose evaluation fails is taken as
//! kept, and nothing is raised: planning raises nothing of a condition
//! the statement evaluates (RULES.md D15). Smaller tables are not
//! sampled, as their scans cost little more than evaluating the sample.

use super::Known;
use crate::exec::{NoSubqueries, eval_cond};
use crate::logical::{Cond, Expr, Truth, compared_with_constant, cond_columns};
use crate::stats::RowSample;
use crate::storage::cost_of;

/// \[DESIGN\] The rows from which a table's conjunctions are evaluated on
/// its sample.
pub(super) const SAMPLED_FROM: f64 = 10_000.0;
/// \[DESIGN\] The fewest sampled rows kept for their share to be the
/// estimate: below eight, a share is mostly chance.
pub(super) const SAMPLE_HITS: usize = 8;
/// \[DESIGN\] What bounds the share of a conjunction a sample keeps fewer
/// than `SAMPLE_HITS` rows of: (k + 3)/n, about the 95% bound of a share
/// none of n rows showed (the rule of three).
pub(super) const EMPTY_BOUND: usize = 3;

/// The sample `conds` are to be evaluated on, by what is `known`: if
/// the table is large enough and keeps one, each conjunct can be
/// evaluated on it, and they go together or a synopsis cannot answer
/// one.
pub(super) fn eligible<'a>(conds: &[&Cond], known: Known<'a>) -> Option<&'a RowSample> {
    let stats = known.stats?;
    let sample = stats.sample()?;
    let width = sample.rows().first()?.len();
    let large = stats.rows()? >= SAMPLED_FROM;
    let told = !conds.is_empty() && conds.iter().all(|c| evaluable(c, width));
    let mut columns = Vec::new();
    for c in conds {
        cond_columns(c, &mut columns);
    }
    columns.sort_unstable();
    columns.dedup();
    let together = conds.len() > 1 && columns.len() > 1;
    (large && told && (together || conds.iter().any(|c| !answered(c)))).then_some(sample)
}

/// The share of rows `conds` keep, by `sample`, `synopses` their
/// estimate otherwise.
pub(super) fn kept(conds: &[&Cond], sample: &RowSample, synopses: f64) -> f64 {
    let rows = sample.rows();
    let n = cost_of(rows.len()).max(1.0_f64);
    let hits = rows
        .iter()
        .filter(|row| conds.iter().all(|c| !matches!(eval_cond(c, row, &NoSubqueries), Ok(Truth::False | Truth::Unknown))))
        .count();
    if hits >= SAMPLE_HITS {
        return cost_of(hits) / n;
    }
    synopses.min(cost_of(hits.saturating_add(EMPTY_BOUND)) / n)
}

/// Whether a column's synopsis answers `c`: a comparison, an IN or a
/// null test of a column with constants, a LIKE of a constant prefix and
/// `%` after it, an OR of such of one column, a negation of one.
pub(super) fn answered(c: &Cond) -> bool {
    match c {
        Cond::Const(_) => true,
        Cond::Compare(..) => compared_with_constant(c).is_some(),
        Cond::In(e, _) | Cond::IsNull(e, _) => matches!(e, Expr::Column(_)),
        Cond::Like(..) => super::like::whole(c),
        Cond::Not(a) => answered(a),
        Cond::Or(a, b) => {
            let mut columns = Vec::new();
            cond_columns(c, &mut columns);
            columns.sort_unstable();
            columns.dedup();
            answered(a) && answered(b) && columns.len() <= 1
        }
        Cond::And(a, b) => answered(a) && answered(b),
        Cond::Distinct(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Is(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::Value(_) => false,
    }
}

/// Whether `c` can be evaluated on a sampled row `width` columns wide:
/// of no subquery, and each of its expressions so.
pub(super) fn evaluable(c: &Cond, width: usize) -> bool {
    if c.has_subquery() {
        return false;
    }
    let mut told = true;
    c.for_each_expr(&mut |e| told = told && evaluable_expr(e, width));
    told
}

/// Whether `e` can be evaluated on a sampled row `width` columns wide:
/// deterministic, of none of the row's columns past it, and of no outer
/// reference, `?`, subquery, sequence value or routine.
fn evaluable_expr(e: &Expr, width: usize) -> bool {
    let own = match e {
        Expr::Column(c) => *c < width,
        Expr::Call(f, _) => f.0.deterministic(),
        Expr::Truth(c) => evaluable(c, width),
        Expr::Outer(..) | Expr::Dynamic(_) | Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_) => false,
        Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Warned(..) => true,
    };
    let mut children = own;
    e.for_each_child(&mut |child| children = children && evaluable_expr(child, width));
    children
}
