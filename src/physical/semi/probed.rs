//! \[DESIGN\] A semi join probing the left's table from the right's rows
//! (RULES.md D774). A semi join whose left operand reads a large table and
//! whose right gives a few rows, as `(NO_W_ID, NO_D_ID, NO_O_ID) IN
//! (SELECT ... FROM UNNEST(...))` does, read the table whole to hash it
//! or probe it. Its rows are those of the left a right row matches, each
//! once. So they are the left's rows of the inner join of the right's
//! rows with the left on the condition, when the right gives one row at
//! most (`LogicalPlan::one_row_at_most`): a left row then matches once at
//! most. Of a condition whose conjuncts reading the right are each an
//! equality of a left and a right column, they are the left's rows of the
//! inner join of the distinct values of the right's columns compared with
//! the left: a left row's values then make one row of those that it
//! matches. That join is planned as any inner join (`physical/join.rs`),
//! and taken when it probes an index of the left's table from each of its
//! rows, the probes priced against the read of the table they replace
//! (`probed_instead`, D386); else the semi join is hashed as before. Its
//! rows come in no order of the left's.

use super::super::built::Built;
use super::super::indexed::reads_table;
use super::super::join::{Flip, JoinKey, equated};
use super::super::{PhysicalPlanner, Plan};
use crate::logical::{Cond, Expr, LogicalPlan, and_all, cond_columns, map_cond_columns, split_and};
use crate::value::CompOp;

impl PhysicalPlanner<'_> {
    /// The semi join of `left` and `right` on `cond` as the join of the
    /// right's rows, or of their distinct values compared, probing an index
    /// of the table `left` reads, if `left` reads one and the probes are
    /// chosen; none else.
    pub(super) fn probed_from_right(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan)) -> Option<Built> {
        if !reads_table(left) {
            return None;
        }
        let width = left.width();
        let (driver, on) = if right.one_row_at_most() {
            let flip = Flip { width, right_width: right.width() };
            (right.clone(), flip.cond(cond.clone()))
        } else {
            distinct_keys(cond, width, right)?
        };
        let shift = driver.width();
        let product = LogicalPlan::Product { left: Box::new(driver), right: Box::new(left.clone()) };
        let joined = LogicalPlan::Filter { cond: on, input: Box::new(product) };
        let exprs = (0..width).map(|c| Expr::Column(c.saturating_add(shift))).collect();
        let plan = LogicalPlan::Project { exprs, input: Box::new(joined) };
        let built = self.build_node(&plan, &[]);
        probes(&built.plan).then(|| Built::unordered(built.plan))
    }
}

/// Of a semi join on `cond` of a left operand `width` columns wide and
/// `right`, the distinct values of the right's columns its equalities
/// compare with the left's, and the condition over those then the left's
/// columns, if each conjunct reading the right is such an equality.
fn distinct_keys(cond: &Cond, width: usize, right: &LogicalPlan) -> Option<(LogicalPlan, Cond)> {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut columns, mut pairs, mut own) = (Vec::new(), Vec::new(), Vec::new());
    for c in conjuncts {
        if let Some(JoinKey { left: a, right: b, nulls: false }) = equated(&c, width) {
            let at = columns.iter().position(|&k| k == b).unwrap_or_else(|| {
                columns.push(b);
                columns.len().saturating_sub(1)
            });
            pairs.push((a, at));
        } else if read_by(&c).iter().all(|&i| i < width) {
            own.push(c);
        } else {
            return None;
        }
    }
    let shift = columns.len();
    let equal = pairs.into_iter().map(|(a, at)| Cond::Compare(Expr::Column(at), CompOp::Eq, Expr::Column(a.saturating_add(shift))));
    let own = own.into_iter().map(|c| map_cond_columns(c, &|i| i.saturating_add(shift)));
    let on = and_all(equal.chain(own).collect())?;
    let exprs = columns.into_iter().map(Expr::Column).collect();
    let keys = LogicalPlan::Project { exprs, input: Box::new(right.clone()) };
    Some((LogicalPlan::Distinct { input: Box::new(keys) }, on))
}

/// Whether `plan`, a projection of a join, probes an index nested-loop
/// join's table, through the projections and filters over it.
fn probes(mut plan: &Plan) -> bool {
    while let Plan::Project { input, .. } | Plan::Filter { input, .. } = plan {
        plan = input;
    }
    matches!(plan, Plan::IndexJoin { .. })
}

/// The columns `c` reads.
fn read_by(c: &Cond) -> Vec<usize> {
    let mut out = Vec::new();
    cond_columns(c, &mut out);
    out
}
