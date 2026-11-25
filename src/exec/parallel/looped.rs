//! \[DESIGN\] (RULES.md D1114) An index nested-loop join whose left
//! operand holds a part run in parallel, so reads many rows, probes its
//! index on the part's threads: its left operand's rows are read by the
//! statement's thread, a wave of morsels at a time, each of a share of
//! the rows of a morsel of a scan (`PROBE_SHARE`), and
//! each morsel's rows are joined by a thread of up to the part's degree,
//! through the join and each index join it is the left operand of below
//! it, as one thread joins them (`exec/join/indexed.rs`). The morsels'
//! rows are given in the order of their numbers, so each left row's
//! matches come where they would alone, and the first error in that order
//! is the one raised, after the rows before it.
//!
//! A join takes part only if a worker evaluates its condition alone and
//! it gives each left row's rows as it reads that row: an inner, LEFT,
//! semi or antijoin, of no spatial join, nor of a probe by a function's
//! predicate, which a statement's prepared budget bounds.

use std::sync::{Arc, Mutex, PoisonError};

use super::run::Morsels;
use crate::error::{Result, SqlError};
use crate::exec::join::{Reads, projected_over};
use crate::exec::project::Columns;
use crate::exec::{Db, RowIter};
use crate::physical::{JoinProbe, Plan, Preserved, calm_cond};
use crate::settings::EXECUTOR_MORSEL_ROWS;
use crate::storage::Row;
use crate::txn::TxId;

/// \[DESIGN\] How deep a plan is walked for a part (RULES.md R-CF-6):
/// deeper than any plan the binder makes nests.
const MAX_DEPTH: usize = 1024;

/// \[DESIGN\] The rows of a morsel of a scan, EXECUTOR.MORSEL_ROWS, a
/// morsel of probes takes a share of: a probe costs a walk of the index
/// and a read of its row, as many rows scanned as this. Measured by a
/// sweep of 1, 4, 16 and 64, 16 the fastest and 4 near it.
const PROBE_SHARE: usize = 16;

/// \[DESIGN\] The morsels of a wave, by thread: enough that the threads
/// rarely wait on the last of a wave, few enough that a wave holds few rows.
const WAVE_PER_THREAD: usize = 4;

/// The left rows of each morsel of a wave, each taken by the thread that
/// joins them.
type Wave = Arc<Vec<Mutex<Vec<Row>>>>;

/// Whether the index join `plan` takes part: one a worker joins alone.
fn joins_alone(plan: &Plan) -> bool {
    let Plan::IndexJoin { outer, spatial: None, probe, cond, .. } = plan else { return false };
    let gives_as_read = !matches!(outer, Preserved::Both(_));
    gives_as_read && !matches!(probe, JoinProbe::Predicate { .. }) && cond.as_ref().is_none_or(calm_cond)
}

/// The degree the index join `plan` probes on, by the parts run in
/// parallel its left operand holds, below any index joins that take part:
/// the greatest of their degrees; 1 to probe on the statement's thread.
pub(in crate::exec) fn loop_degree(plan: &mut Plan) -> usize {
    if !joins_alone(plan) {
        return 1;
    }
    let Plan::IndexJoin { left, .. } = plan else { return 1 };
    if joins_alone(left) {
        return loop_degree(left);
    }
    part_degree(left, 0)
}

/// The greatest degree of a part run in parallel `plan` holds, `depth`
/// below the join; 1 of none.
fn part_degree(plan: &mut Plan, depth: usize) -> usize {
    if let Plan::Parallel(p) = plan {
        return p.degree;
    }
    let mut most = 1;
    if depth < MAX_DEPTH {
        plan.each_input(&mut |input| most = most.max(part_degree(input, depth.saturating_add(1))));
    }
    most
}

/// The rows of the index join `plan`, its columns `output` if some, its
/// probes made on up to `degree` threads.
pub(in crate::exec) fn looped(mut plan: Plan, output: Option<Columns>, (db, tx): (&Db, TxId), degree: usize) -> RowIter {
    let left = leaf(&mut plan, 0);
    Box::new(Looped {
        left: super::execute(left, db, tx),
        joins: Arc::new((plan, output)),
        run: (db.clone(), tx, degree),
        probes: db.settings.count(EXECUTOR_MORSEL_ROWS).checked_div(PROBE_SHARE).unwrap_or(1).max(1),
        morsels: None,
        rows: Vec::new().into_iter(),
        failed: None,
        ended: false,
    })
}

/// The left operand of the lowest index join of `plan` that takes part,
/// taken out, an empty set of rows left in its place.
fn leaf(plan: &mut Plan, depth: usize) -> Plan {
    let empty = || Plan::Values { rows: Vec::new(), detail: String::new() };
    let Plan::IndexJoin { left, .. } = plan else { return empty() };
    if depth < MAX_DEPTH && joins_alone(left) {
        return leaf(left, depth.saturating_add(1));
    }
    std::mem::replace(&mut **left, empty())
}

/// The rows of the index joins `plan`, its columns `output` if some, of
/// the left rows `rows` of the lowest: each join's left operand the rows
/// of the one below.
fn joined(plan: Plan, output: Option<Columns>, rows: Vec<Row>, (db, tx): (&Db, TxId)) -> RowIter {
    let deeper = if let Plan::IndexJoin { left, .. } = &plan { matches!(**left, Plan::IndexJoin { .. }) } else { false };
    let db_for = db.clone();
    let read: Reads = if deeper {
        Box::new(move |left| joined(left, None, rows, (&db_for, tx)))
    } else {
        Box::new(move |_| Box::new(rows.into_iter().map(Ok)))
    };
    projected_over(plan, (output, read), db, tx)
}

/// An index join's rows as its waves of probes give them.
struct Looped {
    left: RowIter,
    /// The joins, their lowest's left operand taken out, and the columns
    /// given.
    joins: Arc<(Plan, Option<Columns>)>,
    run: (Db, TxId, usize),
    /// The left rows of a morsel.
    probes: usize,
    morsels: Option<Morsels<Vec<Row>>>,
    rows: std::vec::IntoIter<Row>,
    /// The error the left operand raised, raised after the rows before it.
    failed: Option<SqlError>,
    ended: bool,
}

impl Looped {
    /// The next wave's morsels of left rows, none once the left operand
    /// is read or failed.
    fn wave(&mut self) -> Vec<Mutex<Vec<Row>>> {
        let (most, mut wave) = (self.run.2.saturating_mul(WAVE_PER_THREAD), Vec::new());
        while wave.len() < most && self.failed.is_none() {
            let mut rows = Vec::with_capacity(self.probes);
            while rows.len() < self.probes {
                match self.left.next() {
                    Some(Ok(row)) => rows.push(row),
                    Some(Err(e)) => {
                        self.failed = Some(e);
                        break;
                    }
                    None => break,
                }
            }
            let short = rows.len() < self.probes;
            if !rows.is_empty() {
                wave.push(Mutex::new(rows));
            }
            if short {
                break;
            }
        }
        wave
    }

    /// Starts the next wave's morsels: whether it holds any.
    fn started(&mut self) -> bool {
        let wave: Wave = Arc::new(self.wave());
        if wave.is_empty() {
            return false;
        }
        let (db, tx, degree) = (self.run.0.clone(), self.run.1, self.run.2);
        let (joins, taken, db_for) = (Arc::clone(&self.joins), Arc::clone(&wave), db.clone());
        let work = Box::new(move |m: usize| -> Result<Vec<Row>> {
            let rows = taken.get(m).map(|r| std::mem::take(&mut *r.lock().unwrap_or_else(PoisonError::into_inner))).unwrap_or_default();
            let (plan, output) = &*joins;
            joined(plan.clone(), output.clone(), rows, (&db_for, tx)).collect()
        });
        self.morsels = Some(Morsels::start(&db, tx, (wave.len(), degree), work));
        true
    }
}

impl Iterator for Looped {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(row) = self.rows.next() {
                return Some(Ok(row));
            }
            if self.ended {
                return None;
            }
            match self.morsels.as_mut().and_then(Iterator::next) {
                Some(Ok(rows)) => self.rows = rows.into_iter(),
                Some(Err(e)) => {
                    self.ended = true;
                    return Some(Err(e));
                }
                None => {
                    self.morsels = None;
                    if !self.started() {
                        self.ended = true;
                        return self.failed.take().map(Err);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
