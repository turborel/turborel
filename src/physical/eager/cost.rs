//! \[DESIGN\] (RULES.md D660) The work of a plan, as the estimates it
//! carries tell it (`physical/carried.rs`), so a grouping planned with a
//! relation grouped first is weighed against the grouping planned as
//! written, both plans made whole by the same planner: a scan's rows
//! read, an index's cost, each row a hash join builds or probes and each
//! a grouping hashes at the hash join's weight (`cost.rs`), each set
//! function's value taken at its own weight, and each row an operator
//! gives at `ROW_STEP`. What both plans share weighs alike in both, so
//! only where they differ decides.

use crate::functions::{MERGED, SCALED};
use crate::logical::AggCall;
use crate::physical::Plan;
use crate::physical::cost::HASH_ROW;

/// \[DESIGN\] The work of a row an operator gives, in rows of a scan: a
/// tenth, as a sort's comparison (`cost.rs`).
const ROW_STEP: f64 = 0.1;

/// \[DESIGN\] The work of a value a grouping keeps in a column of states
/// (RULES.md D356), in rows of a scan.
const COLUMN_VALUE: f64 = 0.05;

/// \[DESIGN\] The work of a value an accumulator of its group takes, a
/// part merged or a value scaled decoded from bytes or a row: six times a
/// value kept in a column, as an accumulator a group is reached through
/// its pointer and its state written and read.
const BOXED_VALUE: f64 = 0.3;

/// \[DESIGN\] How deep a plan is weighed (RULES.md R-CF-6); deeper, a
/// node weighs its estimated rows alone.
const MAX_DEPTH: usize = 1024;

/// The work of `plan`.
pub(super) fn work(plan: &Plan) -> f64 {
    work_at(plan, 0)
}

/// The rows `plan` gives as estimated, at least one.
fn rows(plan: &Plan) -> f64 {
    plan.est().unwrap_or(1.0_f64).max(1.0_f64)
}

/// `work` at nesting `depth`.
fn work_at(plan: &Plan, depth: usize) -> f64 {
    let own = rows(plan);
    if depth > MAX_DEPTH {
        return own;
    }
    let w = |p: &Plan| work_at(p, depth.saturating_add(1));
    match plan {
        Plan::IndexScan { cost, .. } => *cost,
        Plan::Filter { input, .. } | Plan::Project { input, .. } => w(input) + rows(input) * ROW_STEP,
        Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } => {
            w(left) + w(right) + (rows(left) + rows(right)) * HASH_ROW + own * ROW_STEP
        }
        Plan::Product { left, right, .. } => w(left) + w(right) + own * ROW_STEP,
        // A semi join hashes, marks or reads each row of both as a hash
        // join builds and probes them.
        Plan::SemiJoin(j) => w(&j.left) + w(&j.right) + (rows(&j.left) + rows(&j.right)) * HASH_ROW + own * ROW_STEP,
        Plan::IndexJoin { left, cost, .. } => w(left) + cost + own * ROW_STEP,
        Plan::MergeJoin(m) => w(&m.left) + w(&m.right) + (rows(&m.left) + rows(&m.right) + own) * ROW_STEP,
        Plan::RangeJoin(r) => w(&r.left) + w(&r.right) + (rows(&r.left) + rows(&r.right) + own) * ROW_STEP,
        Plan::Aggregate { aggs, input, .. } => {
            let taken: f64 = aggs.iter().map(value_work).sum();
            w(input) + rows(input) * (HASH_ROW + taken) + own * ROW_STEP
        }
        Plan::Parallel(p) => w(&p.input),
        Plan::Counted(c) => w(&c.input),
        Plan::Sort { input, .. } | Plan::Limit { input, .. } | Plan::Distinct { input, .. } => w(input) + own * ROW_STEP,
        Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(..) => own,
    }
}

/// The work of a value `a` takes: in a column of states, or by an
/// accumulator of its group.
fn value_work(a: &AggCall) -> f64 {
    let name = a.func.0.name();
    let boxed = name == MERGED || name == SCALED || (a.func.0.columnar().is_none() && a.func.0.parted().is_none());
    if boxed { BOXED_VALUE } else { COLUMN_VALUE }
}
