//! \[DESIGN\] (RULES.md D651) What an operator holding rows costs past its
//! work grant: a hash join's built rows, a hashed grouping's groups and a
//! sort's rows. The grant is what `spill/grant.rs` would give the
//! operator as the statement is planned (RULES.md D298): half of what the
//! grants' half of the budget has free, at most MEMORY.WORK_BYTES, at
//! least the floor every operator holds. Rows are held at their width,
//! the bytes `Statistics::width` says the columns read take, and an
//! entry's bytes more of the table holding them.
//!
//! An operator whose rows outgrow the grant writes them to the spill area
//! and reads them back once more. A hash join past its grant partitions
//! both its operands, as a Grace hash join does (`exec/join/partitioned.rs`),
//! so each row of either is written and read once more; a grouping writes
//! its partial groups and its rows past the grant; a sort writes runs and
//! merges them. Each such row costs `SPILLED_ROW`, a hash join's
//! `PARTITIONED_ROW`, and each byte of it `SPILLED_BYTE`, in rows of a
//! scan, as `bench/analytics/costs.rs` measures them. So a merge join of inputs already in order, or a
//! grouping streamed in its keys' order, wins where holding the rows
//! would spill, and a wide build fills its grant sooner than a narrow one.

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::physical::{PhysicalPlanner, Plan};
use crate::settings::MEMORY_WORK_BYTES;
use crate::storage::cost_of;

/// \[DESIGN\] The cost of a row written to the spill area and read back
/// once more, in rows of a scan: a sort's run written and merged
/// (`bench/analytics/costs.rs`, its sort spilled, less its rows' bytes).
pub(in crate::physical) const SPILLED_ROW: f64 = 4.5;
/// \[DESIGN\] The cost of a row of a hash join's operand partitioned past
/// its grant, in rows of a scan: written, read back, hashed again and
/// probed by its partition (`bench/analytics/costs.rs`, its hash join
/// spilled, less its rows' bytes), dearer than a sort's.
pub(in crate::physical) const PARTITIONED_ROW: f64 = 15.0;
/// \[DESIGN\] The cost more of each byte of such a row, in rows of a scan:
/// a spilled row of 20 bytes costs a scanned row more than one of none.
pub(in crate::physical) const SPILLED_BYTE: f64 = 0.05;
/// \[DESIGN\] The bytes an operator's table holds for each row besides its
/// values: its hash, its link and its slot.
pub(in crate::physical) const ENTRY_BYTES: f64 = 16.0;
/// \[DESIGN\] The bytes of a column's value held when its width is not
/// known: an integer's.
pub(in crate::physical) const COLUMN_BYTES: f64 = 8.0;

/// The bytes an operator of a statement running alone would be granted,
/// as its grant takes them (`spill/grant.rs`, RULES.md D683, D1159): half
/// of the grants' share, of the standing reservations at their bounds and
/// no grant held, at most MEMORY.WORK_BYTES, at least the floor, or the
/// ceiling itself below the floor.
pub(crate) fn grant_bytes(db: &State) -> f64 {
    cost_of(db.work.would_grant(crate::settings::work_bytes(db.settings.get(MEMORY_WORK_BYTES))))
}

/// The bytes a row of `columns` columns held by an operator takes, of
/// `width` bytes of values if known.
pub(in crate::physical) fn row_bytes(columns: usize, width: Option<f64>) -> f64 {
    width.unwrap_or_else(|| cost_of(columns) * COLUMN_BYTES) + ENTRY_BYTES
}

/// The cost more of an operator holding `held` rows of `bytes` bytes
/// each, past a grant of `grant` bytes, which writes and reads back
/// `moved` rows once more if they outgrow it: none if they fit.
pub(in crate::physical) fn spilled((held, bytes): (f64, f64), moved: f64, grant: f64) -> f64 {
    moved_past((held, bytes), (moved, SPILLED_ROW), grant)
}

/// `spilled`, each row moved costing `per_row` besides its bytes.
fn moved_past((held, bytes): (f64, f64), (moved, per_row): (f64, f64), grant: f64) -> f64 {
    if held * bytes <= grant {
        return 0.0_f64;
    }
    moved * (per_row + SPILLED_BYTE * bytes)
}

/// The cost more of a hash join building `built` rows of `bytes` bytes
/// and probing with `probing` rows, past a grant of `grant` bytes: both
/// operands partitioned, each row written and read once more.
pub(in crate::physical) fn hash_spilled((built, bytes): (f64, f64), probing: f64, grant: f64) -> f64 {
    moved_past((built, bytes), (built + probing, PARTITIONED_ROW), grant)
}

/// \[DESIGN\] (RULES.md D1240) The share of a grant a pass of a hash
/// join built in parts past it holds: a third, as the first pass's table
/// is held to the end beside the pass being probed and the next, made
/// while the last morsels of that one end (`exec/parallel/passes.rs`).
pub(crate) const PASS_SHARE: f64 = 3.0;

/// \[DESIGN\] (RULES.md D1240) The most passes a hash join run in
/// parallel and built past its grant probes in, each probing row costing
/// `probe_row` in rows of a scan to make again and holding `bytes` bytes:
/// as many as cost no more than its rows partitioned once. A pass past the
/// first reads the probing side again, where the join partitioned writes
/// and reads each of its rows once more (`PARTITIONED_ROW` and its bytes);
/// the built rows are written once either way, so they weigh nothing in
/// the choice. Passes win while (P - 1) x `probe_row` is at most that.
pub(crate) fn pass_limit(probe_row: f64, bytes: f64) -> f64 {
    1.0_f64 + ((PARTITIONED_ROW + SPILLED_BYTE * bytes) / probe_row.max(1.0_f64)).floor()
}

/// \[DESIGN\] (RULES.md D1240) The cost more of a hash join run in
/// parallel building `built` rows of `bytes` bytes past a grant of
/// `grant` bytes in passes, its probing side costing `probing` to read
/// and giving `rows` rows: the built rows written and read back once, and
/// the probing side read again for each pass past the first; none if they
/// fit, without bound past the passes `pass_limit` allows, as the join
/// then gives way. A probing row's bytes are an entry's, as the run, which
/// knows no estimate of its width, takes them (`passes_of`), so the plan
/// never counts on more passes than the run allows.
pub(in crate::physical) fn passed_spilled((built, bytes): (f64, f64), (probing, rows): (f64, f64), grant: f64) -> f64 {
    if built * bytes <= grant {
        return 0.0_f64;
    }
    let passes = (built * bytes / (grant / PASS_SHARE)).ceil();
    if passes > pass_limit(probing / rows.max(1.0_f64), ENTRY_BYTES) {
        return f64::INFINITY;
    }
    built * (SPILLED_ROW + SPILLED_BYTE * bytes) + (passes - 1.0_f64) * probing
}

/// \[DESIGN\] (RULES.md D1240) What reading `plan`, the probing side of
/// a hash join run in parallel, once more costs, in rows of a scan, and
/// the rows it gives, as the planner costed them: its scans' costs, each
/// probe of a join below a row hashed (`HASH_ROW`), and the rows its top
/// estimates. None if it is not a pipeline of scans in no order, filters,
/// projections and hash joins' probes, as an index scan's or an ordered
/// scan's rows come in an order a plan above may rely on, which passes
/// would lose.
pub(crate) fn rescanned(plan: &Plan) -> Option<(f64, f64)> {
    match plan {
        Plan::SeqScan { cost, rows, order, .. } => order.is_empty().then(|| (*cost, cost_of(*rows))),
        Plan::Filter { input, est, .. } => rescanned(input).map(|(cost, _)| (cost, *est)),
        Plan::Project { input, .. } => rescanned(input),
        Plan::Counted(c) => rescanned(&c.input),
        Plan::HashJoin { left, est, .. } | Plan::OuterJoin { left, est, .. } => probed(left, *est),
        Plan::SemiJoin(s) => probed(&s.left, s.est),
        Plan::Append { left, right, .. } => {
            let ((a, m), (b, n)) = (rescanned(left)?, rescanned(right)?);
            Some((a + b, m + n))
        }
        Plan::IndexScan { .. }
        | Plan::Transfer(_)
        | Plan::Nearest(_)
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Product { .. }
        | Plan::Aggregate { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Shared(_)
        | Plan::Parallel(..) => None,
    }
}

/// \[DESIGN\] (RULES.md D1240) The most passes a hash join run in
/// parallel whose probing side is `plan` may probe in (`pass_limit`), as
/// the planner costed that side; none if it cannot be read again
/// (`rescanned`). A probing row's bytes are taken at the least, an
/// entry's, as a plan does not carry its rows' width, which allows fewer
/// passes, not more.
pub(crate) fn passes_of(plan: &Plan) -> Option<f64> {
    rescanned(plan).map(|(cost, rows)| pass_limit(cost / rows.max(1.0_f64), ENTRY_BYTES))
}

/// `rescanned` of a join's probing side `left`, giving `est` rows.
fn probed(left: &Plan, est: f64) -> Option<(f64, f64)> {
    rescanned(left).map(|(cost, rows)| (cost + super::HASH_ROW * rows, est))
}

/// The cost more of a hashed grouping holding `groups` groups of `bytes`
/// bytes each and given `rows` rows, past a grant of `grant` bytes: its
/// rows partitioned, written and read back once more, as a hash join's
/// operands are (`bench/analytics/costs.rs`, its grouping spilled).
pub(in crate::physical) fn grouped_spilled((groups, bytes): (f64, f64), rows: f64, grant: f64) -> f64 {
    moved_past((groups, bytes), (rows, PARTITIONED_ROW), grant)
}

/// The cost more of a sort of `rows` rows of `bytes` bytes past a grant
/// of `grant` bytes: its runs written and merged once.
pub(in crate::physical) fn sort_spilled((rows, bytes): (f64, f64), grant: f64) -> f64 {
    spilled((rows, bytes), rows, grant)
}

impl PhysicalPlanner<'_> {
    /// The bytes a row of `columns` columns of `table` holds: its share of
    /// the table's average row, as the statistics say, else each column's
    /// an integer's.
    pub(in crate::physical) fn row_bytes_of(&self, table: &QualifiedName, columns: usize) -> f64 {
        let all = self.db.catalog.table(table).map_or(columns, |t| t.columns.len());
        let share = (cost_of(columns) / cost_of(all).max(1.0_f64)).min(1.0_f64);
        row_bytes(columns, self.statistics(table).and_then(|s| s.width(&[])).map(|w| w * share))
    }
}
