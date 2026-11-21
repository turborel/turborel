//! \[DESIGN\] (RULES.md D660, D664) The work of a plan, as the estimates
//! it carries tell it (`physical/carried.rs`), so a grouping planned with
//! a relation grouped first is weighed against the grouping planned as
//! written, both plans made whole by the same planner: a scan's rows
//! read, an index's cost, each row a hash join builds or probes and each
//! a grouping hashes at a weight that grows with the table it is hashed
//! into, each set function's value taken at its own weight, each conjunct
//! a filter judges of a row, each row a product makes at `PRODUCT_ROW`,
//! and each row another operator gives at `ROW_STEP`. What both plans
//! share weighs alike in both, so only where they differ decides.
//!
//! The weights are those `bench/analytics/costs.rs` measures (D650), in
//! rows of the row store's scan by batch, the unit of a scan's cost.

use crate::functions::{MERGED, SCALED};
use crate::logical::{AggCall, Cond, split_and};
use crate::physical::Plan;
use crate::physical::cost::runtime::probed;

/// \[DESIGN\] The work of a row an operator gives: a sort's comparison,
/// as measured (D650).
const ROW_STEP: f64 = 0.105;

/// \[DESIGN\] The work of a row a product makes of a row of each side,
/// as EXPLAIN ANALYZE measures a product of a thousand rows by a
/// thousand: about two rows of the scan, an upper figure, as the count
/// of each row adds to it. A product priced as an operator's row alone
/// looked cheaper than the joins it stands for.
const PRODUCT_ROW: f64 = 2.0;

/// \[DESIGN\] The work of a value a grouping keeps in a column of states
/// (RULES.md D356): a conjunct judged by batch, as measured (D650), one
/// expression of a row by a kernel of its column.
const COLUMN_VALUE: f64 = 0.457;

/// \[DESIGN\] The work of a value an accumulator of its group takes, a
/// part merged or a value scaled decoded from bytes or a row: a conjunct
/// judged a row at a time, as measured (D650), as an accumulator is
/// reached through its group's pointer and its state written and read.
const BOXED_VALUE: f64 = 1.61;

/// \[DESIGN\] The work of a row a grouping hashes into few groups (a
/// thousand), its value's taken apart: a grouping's row of 1.03 measured
/// (D650), less `COLUMN_VALUE`.
const GROUP_FEW: f64 = 0.573;

/// \[DESIGN\] The work of a row a grouping hashes into a group of its
/// own, of a million: 6.96 measured (D650), less `COLUMN_VALUE`.
const GROUP_EACH: f64 = 6.503;

/// \[DESIGN\] The work of a row a hash join builds or probes of a table
/// of a million rows, as measured (D650).
const JOIN_LARGE: f64 = 3.705;

/// \[DESIGN\] The work of a row a hash join builds or probes of a table
/// of a thousand rows, less `ROW_STEP`: as EXPLAIN ANALYZE measures the
/// probes of a thousand suppliers by line items. The grouping's ratio of
/// the large table's weights gave less than half of it, and plans that
/// trade a join's rows for a grouping's were weighed the wrong way.
const JOIN_FEW: f64 = 0.73;

/// \[DESIGN\] The tables, in rows, the few and the large weights are
/// measured of; between, a weight is taken by the logarithm of its table.
const FEW: f64 = 1_000.0;
const LARGE: f64 = 1_000_000.0;

/// \[DESIGN\] How deep a plan is weighed (RULES.md R-CF-6); deeper, a
/// node weighs its estimated rows alone.
const MAX_DEPTH: usize = 1024;

/// The work of `plan`, its hash joins making runtime filters if
/// `filters` (EXECUTOR.RUNTIME_FILTERS).
pub(super) fn work(plan: &Plan, filters: bool) -> f64 {
    work_at(plan, (0, filters))
}

/// The rows `plan` gives as estimated, at least one.
fn rows(plan: &Plan) -> f64 {
    plan.est().unwrap_or(1.0_f64).max(1.0_f64)
}

/// The weight of a row hashed into a table of `size` rows or groups,
/// `few` at `FEW` and less, `large` at `LARGE` and more, and between by
/// the logarithm of `size`, as a table outgrows each cache in turn.
fn hashed(size: f64, (few, large): (f64, f64)) -> f64 {
    let at = ((size.max(1.0_f64).ln() - FEW.ln()) / (LARGE.ln() - FEW.ln())).clamp(0.0_f64, 1.0_f64);
    few + (large - few) * at
}

/// The work of a hash join's rows: each of `build` and of `probe` hashed,
/// into a table of the build's rows. The planner's own weight of a hash
/// join's row by its build's size (planner I, D650) is to take this
/// curve's place once it lands, so the two are one function.
fn joined(build: f64, probe: f64) -> f64 {
    (build + probe) * hashed(build, (JOIN_FEW, JOIN_LARGE))
}

/// The work of an inner hash join of `left` and `right` rows giving
/// `own`, its runtime filter made if `filters`: the filter judges each
/// probing row and keeps for hashing only those a built key matches, as
/// the planner weighs it (`cost/runtime.rs`, D653), the share it keeps
/// that of the rows the join gives, at least its build's.
fn filtered((left, right, own): (f64, f64, f64), filters: bool) -> f64 {
    let (build, probe) = (left.min(right), left.max(right));
    if !filters {
        return joined(build, probe);
    }
    let per_row = hashed(build, (JOIN_FEW, JOIN_LARGE));
    let kept = (own.max(build) / probe).min(1.0_f64);
    build * per_row + probed(probe, per_row, kept)
}

/// `work` at nesting `depth`.
fn work_at(plan: &Plan, (depth, filters): (usize, bool)) -> f64 {
    let own = rows(plan);
    if depth > MAX_DEPTH {
        return own;
    }
    let w = |p: &Plan| work_at(p, (depth.saturating_add(1), filters));
    match plan {
        Plan::IndexScan { cost, .. } => *cost,
        Plan::Filter { cond, input, .. } => w(input) + rows(input) * (ROW_STEP + conjuncts(cond) * COLUMN_VALUE),
        Plan::Project { input, .. } => w(input) + rows(input) * ROW_STEP,
        Plan::HashJoin { left, right, .. } => w(left) + w(right) + filtered((rows(left), rows(right), own), filters) + own * ROW_STEP,
        Plan::OuterJoin { left, right, .. } => {
            w(left) + w(right) + joined(rows(left).min(rows(right)), rows(left).max(rows(right))) + own * ROW_STEP
        }
        // A semi join hashes, marks or reads each row of both as a hash
        // join builds and probes them.
        Plan::SemiJoin(j) => {
            let (l, r) = (rows(&j.left), rows(&j.right));
            w(&j.left) + w(&j.right) + joined(l.min(r), l.max(r)) + own * ROW_STEP
        }
        Plan::Product { left, right, .. } => w(left) + w(right) + own * PRODUCT_ROW,
        Plan::IndexJoin { left, cost, .. } => w(left) + cost + own * ROW_STEP,
        Plan::MergeJoin(m) => w(&m.left) + w(&m.right) + (rows(&m.left) + rows(&m.right) + own) * ROW_STEP,
        Plan::RangeJoin(r) => w(&r.left) + w(&r.right) + (rows(&r.left) + rows(&r.right) + own) * ROW_STEP,
        Plan::Aggregate { aggs, input, .. } => {
            let taken: f64 = aggs.iter().map(value_work).sum();
            w(input) + rows(input) * (hashed(own, (GROUP_FEW, GROUP_EACH)) + taken) + own * ROW_STEP
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

/// The conjuncts of `cond`, each judged of every row a filter reads.
fn conjuncts(cond: &Cond) -> f64 {
    let mut each = Vec::new();
    split_and(cond.clone(), &mut each);
    f64::from(u32::try_from(each.len()).unwrap_or(u32::MAX))
}

/// The work of a value `a` takes: in a column of states, or by an
/// accumulator of its group; none of COUNT(*), or its part, which counts
/// the rows a grouping already hashes.
fn value_work(a: &AggCall) -> f64 {
    let name = a.func.0.name();
    if a.arg.is_none() {
        return 0.0_f64;
    }
    let boxed = name == MERGED || name == SCALED || (a.func.0.columnar().is_none() && a.func.0.parted().is_none());
    if boxed { BOXED_VALUE } else { COLUMN_VALUE }
}
