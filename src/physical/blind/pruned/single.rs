//! A plan's operators of one input pruned (`blind/pruned.rs`, RULES.md
//! D1167): a scan, a filter, a projection and a grouping.

use super::super::super::{PhysicalPlanner, Plan};
use super::Live;
use crate::logical::{AggCall, Cond, Expr, expr_columns};

/// `plan`, a scan, of whose columns those above read `needed`, reading
/// those and the ones it is ordered by alone.
pub(super) fn scan_pruned(plan: Plan, needed: &[usize]) -> (Plan, Live) {
    let Plan::SeqScan { table, mut columns, rows, cost, order, backward, most, located } = plan else { return (plan, None) };
    // A row's id follows a located scan's columns (`LogicalPlan::Scan`),
    // which are left as they are.
    if !located {
        columns.retain(|c| needed.contains(c) || order.contains(c));
    }
    let live = (!located).then(|| columns.clone());
    (Plan::SeqScan { table, columns, rows, cost, order, backward, most, located }, live)
}

impl PhysicalPlanner<'_> {
    /// `plan`, a filter: its input pruned to `needed` and its condition's
    /// columns, or, if a subquery is in its condition, whole.
    pub(super) fn filter_pruned(&self, plan: Plan, needed: &[usize]) -> (Plan, Live) {
        let Plan::Filter { cond, detail, est, of_read, input } = plan else { return (plan, None) };
        if cond.has_subquery() {
            return self.whole(Plan::Filter { cond, detail, est, of_read, input });
        }
        let (input, live) = self.pruned_in(*input, &super::super::with_cond(needed, Some(&cond)));
        (Plan::Filter { cond, detail, est, of_read, input: Box::new(input) }, live)
    }

    /// `plan`, a projection: its input pruned to the columns it moves that
    /// are read above and those its other expressions read, or, if a
    /// subquery is in one, whole.
    pub(super) fn project_pruned(&self, plan: Plan, needed: &[usize]) -> (Plan, Live) {
        let Plan::Project { exprs, detail, input } = plan else { return (plan, None) };
        if !exprs.iter().all(plain) {
            return self.whole(Plan::Project { exprs, detail, input });
        }
        let (input, live) = self.pruned_in(*input, &project_reads(&exprs, needed));
        let live = live.map(|l| projected_live(&exprs, &l));
        (Plan::Project { exprs, detail, input: Box::new(input) }, live)
    }

    /// `plan`, a grouping: its input pruned to the columns its groups and
    /// its set functions read, or, if a subquery is in one, whole.
    pub(super) fn aggregate_pruned(&self, plan: Plan) -> (Plan, Live) {
        let Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed } = plan else { return (plan, None) };
        if !aggs.iter().all(plain_agg) {
            return self.whole(Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed });
        }
        let input = Box::new(self.pruned_in(*input, &super::super::super::grouping::reads(&groups, &aggs)).0);
        (Plan::Aggregate { groups, collations, aggs, seeds, detail, input, est, streamed }, None)
    }
}

/// Whether `e` reads nothing but the columns it names: no subquery in it.
pub(super) fn plain(e: &Expr) -> bool {
    !Cond::Value(e.clone()).has_subquery()
}

/// Whether the set function `a` reads nothing but the columns it names.
fn plain_agg(a: &AggCall) -> bool {
    a.arg.as_ref().is_none_or(plain)
        && a.filter.as_ref().is_none_or(|f| !f.has_subquery())
        && a.distribution.as_ref().is_none_or(|d| plain(&d.fraction))
}

/// The columns a projection of `exprs` reads of its input, those above it
/// reading its columns `needed`: a column moved to one of `needed`, and
/// those any other expression reads, whether read above or not, so no
/// error it raises is lost.
fn project_reads(exprs: &[Expr], needed: &[usize]) -> Vec<usize> {
    let mut reads = Vec::new();
    for (n, e) in exprs.iter().enumerate() {
        if let Expr::Column(c) = e {
            if needed.contains(&n) {
                reads.push(*c);
            }
        } else {
            expr_columns(e, &mut reads);
        }
    }
    reads
}

/// The columns of a projection of `exprs` that may hold a value, those of
/// its input `live`: a column moved of one of them, and any other
/// expression but the null value.
fn projected_live(exprs: &[Expr], live: &[usize]) -> Vec<usize> {
    let held = |e: &Expr| {
        if let Expr::Column(c) = e {
            live.contains(c)
        } else if let Expr::Const(v) = e {
            !v.is_null()
        } else {
            true
        }
    };
    exprs.iter().enumerate().filter(|(_, e)| held(e)).map(|(n, _)| n).collect()
}
