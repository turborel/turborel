//! \[DESIGN\] A conjunction estimated on the table's sample of rows
//! (RULES.md D382). Of a table whose statistics keep a sample, a
//! conjunction of two or more conjuncts over two or more columns, or with
//! a conjunct no synopsis answers (a LIKE with no prefix or with more than
//! `%` after it, a function of a column, an OR across columns), is
//! evaluated on the sample's rows, at most `SAMPLE_ROWS` of them however
//! large the table: k of n kept give k/n when k is at least `SAMPLE_HITS`
//! or the sample is the whole table; fewer say only that the share is
//! small, and the estimate is that of the equalities' joint distinct
//! values in the sample (`estimate/joint.rs`), else the synopses', at
//! most the bound the sample allows: (k + 3(1 - f))/n of a sample of the
//! share f of the table, the rule of three (Hertzschuch et al., "Small
//! selectivities matter", SIGMOD 2021) narrowed as the sample nears the
//! whole table, where it is exact.
//!
//! Only conjuncts the sample can tell are evaluated: deterministic, of
//! no subquery, routine, sequence value or `?` not yet given, and of
//! the table's own columns. A row whose evaluation fails is taken as
//! kept, and nothing is raised: planning raises nothing of a condition
//! the statement evaluates (RULES.md D15).

use super::Known;
use super::fingerprint::key_of;
use crate::exec::{NoSubqueries, eval_cond};
use crate::logical::{Cond, Expr, Truth, compared_with_constant, cond_columns};
use crate::stats::RowSample;
use crate::storage::cost_of;

/// \[DESIGN\] The fewest sampled rows kept for their share to be the
/// estimate: below eight, a share is mostly chance.
pub(super) const SAMPLE_HITS: usize = 8;
/// \[DESIGN\] What bounds the share of a conjunction a sample keeps fewer
/// than `SAMPLE_HITS` rows of: (k + 3)/n, about the 95% bound of a share
/// none of n rows showed (the rule of three), of a sample of a large
/// table.
pub(super) const EMPTY_BOUND: usize = 3;

/// The sample `conds` are to be evaluated on, by what is `known`: if
/// the table keeps one, each conjunct can be evaluated on it, and they
/// go together or a synopsis cannot answer one.
pub(super) fn eligible<'a>(conds: &[&Cond], known: Known<'a>) -> Option<&'a RowSample> {
    let stats = known.stats?;
    let sample = stats.sample()?;
    let width = sample.rows().first()?.len();
    let told = !conds.is_empty() && conds.iter().all(|c| evaluable(c, width));
    let mut columns = Vec::new();
    for c in conds {
        cond_columns(c, &mut columns);
    }
    columns.sort_unstable();
    columns.dedup();
    let together = conds.len() > 1 && columns.len() > 1;
    (told && (together || conds.iter().any(|c| !answered(c)))).then_some(sample)
}

/// The share of rows `conds` keep, by `sample` of a table of `of` rows:
/// of too few kept rows to count, of a sample less than the table,
/// `rare`'s estimate if it gives one, else `synopses`, at most the bound
/// the sample allows.
pub(super) fn kept(conds: &[&Cond], (sample, of): (&RowSample, f64), synopses: f64, rare: impl FnOnce() -> Option<f64>) -> f64 {
    let rows = sample.rows();
    let n = cost_of(rows.len()).max(1.0_f64);
    let hits = sample.remembered(key_of("kept", conds, &|_| {}), || {
        cost_of(
            rows.iter()
                .filter(|row| conds.iter().all(|c| !matches!(eval_cond(c, row, &NoSubqueries), Ok(Truth::False | Truth::Unknown))))
                .count(),
        )
    });
    // The share of the table the sample is.
    let whole = (n / of.max(1.0_f64)).min(1.0_f64);
    if hits >= cost_of(SAMPLE_HITS) || whole >= 1.0_f64 {
        return hits / n;
    }
    let bound = (hits + cost_of(EMPTY_BOUND) * (1.0_f64 - whole)) / n;
    rare().unwrap_or(synopses).min(bound)
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

/// The answer `answer`, an option, as the sample remembers it: none as
/// less than nothing.
pub(super) fn remembered_as(answer: Option<f64>) -> f64 {
    answer.unwrap_or(-1.0_f64)
}

/// The answer `remembered` as it was given.
pub(super) fn as_given(remembered: f64) -> Option<f64> {
    (remembered >= 0.0_f64).then_some(remembered)
}
