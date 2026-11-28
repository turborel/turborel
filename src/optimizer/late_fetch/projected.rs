//! \[DESIGN\] A select list's expressions computed after FETCH FIRST
//! (RULES.md D1306). A query that sorts its rows and keeps the first of
//! them, `SELECT A, A - 1, COUNT(*) ... ORDER BY 3 DESC FETCH FIRST 10
//! ROWS ONLY`, computed every expression of its select list of every
//! row sorted, though ten rows show them. The rule
//! `compute_after_fetch_first` computes above the FETCH FIRST each
//! expression of the select list that gives the same value of the same
//! columns each time and can neither raise nor warn, judged by its
//! columns' declared types (`quiet`), so evaluating it of the rows kept
//! alone hides no error (RULES.md D15). The select list below keeps
//! every other expression where it was, one that may raise still
//! evaluated of every row, and hands on the columns the expressions
//! moved read.
//!
//! The rows kept and their order are the sort keys' doing alone, and
//! OFFSET, FETCH FIRST of rows or of a percentage and WITH TIES read
//! nothing else, so the same rows come out in the same order. The rule
//! takes a sort by column references of the select list, the set
//! functions of a grouping among them. A sort by an expression is left,
//! as a search by a distance is planned of that shape
//! (`physical/nearest.rs`).
//!
//! A select list of column references alone below a FETCH FIRST is what
//! a grouping run in parallel gives its first groups under (RULES.md
//! D1124), and what `fetch_dependents_late` fetches late through, so
//! the rule is a late one before that rule.

use crate::logical::{Expr, Fetch, Limit, LogicalPlan, SortKey, expr_columns, map_expr_columns};
use crate::optimizer::dependent_keys::quiet;
use crate::optimizer::{Rule, RuleContext};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const COMPUTE_AFTER_FETCH_FIRST: &str = "compute_after_fetch_first";

/// A select list's expressions computed after FETCH FIRST, as the module
/// says.
pub struct ComputeAfterFetchFirst;

impl Rule for ComputeAfterFetchFirst {
    fn name(&self) -> &'static str {
        COMPUTE_AFTER_FETCH_FIRST
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Limit { limit, input } = plan else { return None };
        let LogicalPlan::Sort { keys, input: sorted } = &**input else { return None };
        let LogicalPlan::Project { exprs, input: rows } = &**sorted else { return None };
        let ties = limit.fetch.as_ref().and_then(|f| f.ties.as_deref()).unwrap_or_default();
        if !keys.iter().chain(ties).all(|k| matches!(exprs.get(k.column), Some(Expr::Column(_)))) {
            return None;
        }
        let catalog = &*ctx.db.catalog;
        let later: Vec<bool> = exprs.iter().map(|e| !matches!(e, Expr::Column(_)) && quiet(e, rows, catalog)).collect();
        let is_later = |i: usize| later.get(i).is_some_and(|&l| l);
        let kept: Vec<usize> = (0..exprs.len()).filter(|&i| !is_later(i)).collect();
        if kept.len() == exprs.len() {
            return None;
        }
        let mut below: Vec<Expr> = kept.iter().filter_map(|&i| exprs.get(i).cloned()).collect();
        let mut read = Vec::new();
        exprs.iter().enumerate().filter(|(i, _)| is_later(*i)).for_each(|(_, e)| expr_columns(e, &mut read));
        for c in read {
            if place_of(&below, c).is_none() {
                below.push(Expr::Column(c));
            }
        }
        // Where each column read above is below, and each column kept.
        let handed = |c: usize| place_of(&below, c).unwrap_or(c);
        let moved = |i: usize| kept.iter().position(|&k| k == i).unwrap_or(i);
        let of = |(i, e): (usize, &Expr)| if is_later(i) { map_expr_columns(e.clone(), &handed) } else { Expr::Column(moved(i)) };
        let above: Vec<Expr> = exprs.iter().enumerate().map(of).collect();
        let key = |k: &SortKey| SortKey { column: moved(k.column), ..*k };
        let fetch = limit.fetch.clone().map(|f| Fetch { ties: f.ties.map(|t| t.iter().map(key).collect()), ..f });
        let first = Limit { offset: limit.offset.clone(), fetch };
        let listed = LogicalPlan::Project { exprs: below, input: rows.clone() };
        let ordered = LogicalPlan::Sort { keys: keys.iter().map(key).collect(), input: Box::new(listed) };
        let limited = LogicalPlan::Limit { limit: first, input: Box::new(ordered) };
        Some(LogicalPlan::Project { exprs: above, input: Box::new(limited) })
    }
}

/// The place among `exprs` of the reference to the column `c`, if one
/// of them is it.
fn place_of(exprs: &[Expr], c: usize) -> Option<usize> {
    exprs.iter().position(|e| if let Expr::Column(own) = e { *own == c } else { false })
}

// Unit tests: an expression not deterministic that raises nothing, which
// no query gives the rule (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::Database;
    use crate::functions::{ScalarFunction, ScalarRef};
    use crate::value::{DataType, Value};

    /// A function that raises nothing, deterministic if `.0`.
    struct Quiet(bool);
    impl ScalarFunction for Quiet {
        fn name(&self) -> &'static str {
            "QUIET"
        }
        fn return_type(&self, args: &[DataType]) -> crate::error::Result<DataType> {
            Ok(args.first().cloned().unwrap_or(DataType::Boolean))
        }
        fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
            Ok(args.first().cloned().unwrap_or(Value::Null))
        }
        fn raise_free(&self) -> bool {
            true
        }
        fn deterministic(&self) -> bool {
            self.0
        }
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §4.35.1]; [DESIGN] RULES.md D1306:
    /// a call in the select list that raises nothing is computed after
    /// the FETCH FIRST if the function is deterministic, the select list
    /// below then of the column alone, and stays below if it is not.
    #[test]
    fn a_call_not_deterministic_stays_below_the_fetch_first() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE P (K INTEGER NOT NULL, N VARCHAR(10))").unwrap();
        let s = db.session("O");
        let (st, bound) = s.bound_plan("SELECT K, N FROM P ORDER BY K FETCH FIRST 2 ROWS ONLY").unwrap();
        let ctx = RuleContext { db: &st, seen: &crate::physical::Seen::default() };
        let LogicalPlan::Limit { limit, input } = bound else { panic!("{bound:?}") };
        let LogicalPlan::Sort { keys, input: listed } = *input else { panic!("no sort") };
        let LogicalPlan::Project { input: rows, .. } = *listed else { panic!("no select list") };
        let applied = |deterministic: bool| {
            let call = Expr::Call(ScalarRef(Arc::new(Quiet(deterministic))), vec![Expr::Column(0)]);
            let listed = LogicalPlan::Project { exprs: vec![Expr::Column(0), call], input: rows.clone() };
            let sorted = LogicalPlan::Sort { keys: keys.clone(), input: Box::new(listed) };
            ComputeAfterFetchFirst.apply(&LogicalPlan::Limit { limit: limit.clone(), input: Box::new(sorted) }, &ctx)
        };
        let Some(LogicalPlan::Project { exprs, input }) = applied(true) else { panic!("left") };
        assert!(matches!(exprs.get(1), Some(Expr::Call(..))), "{exprs:?}");
        let LogicalPlan::Limit { input: sorted, .. } = *input else { panic!("no FETCH FIRST below") };
        let LogicalPlan::Sort { input: below, .. } = *sorted else { panic!("no sort below") };
        let LogicalPlan::Project { exprs: columns, .. } = *below else { panic!("no select list below") };
        assert!(matches!(columns.as_slice(), [Expr::Column(0)]), "{columns:?}");
        assert!(applied(false).is_none());
    }
}
