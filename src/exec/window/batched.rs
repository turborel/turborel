//! \[DESIGN\] RULES.md D560: window functions over batches [ISO/IEC
//! 9075-2:2023 §7.15, §6.10]. The window reads its input's batches in the
//! order of its partitioning and ordering: of a sort the planner placed
//! under it, each batch beside the normal keys the sort made of its rows
//! (`sort::sorted_keyed`), else keys made of each batch, or of rows made
//! batches. Where each row begins a partition or a peer group is found of
//! the keys' bytes (`boundaries.rs`); each call's arguments are evaluated
//! a vector at a time. A batch is held whole until a partition begins in
//! it, then cut there: the rows before are added to those held, every
//! partition of them is whole, and each call's values of them are
//! computed as vectors (`compute.rs`) and given beside the batches held,
//! one column a call. The rest begins the next rows held.
//!
//! The window gives way to its row-at-a-time form (`window.rs`) for the
//! rows held and all after them, which begin a partition, where that form
//! is what the standard's rules need of them: where a call's argument is
//! not evaluated by vectors without a warning or an error, where computing
//! a call raises an error, so the error is raised of the row and in the
//! order the rows' form raises it (RULES.md D15), and where the rows held
//! pass the work area, so they spill as that form spills them (D38).
//! Calls that form alone computes (nested window functions, row pattern
//! measures, filtered, DISTINCT or inverse distribution set functions)
//! leave the whole window to it (`admits`).

mod boundaries;
mod compute;
mod frames;
mod lanes;
mod picks;
mod reading;
mod sets;
mod states;

use std::collections::VecDeque;
use std::sync::Arc;

use self::boundaries::Last;
use self::compute::Prepared;
use super::frame::Bounds;
use super::value::Count;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, whole_batches, whole_width};
use crate::exec::sort::{KeyedBatches, sorted_keyed};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, execute};
use crate::kernels::Kernels;
use crate::logical::{Expr, SortKey, Window, WindowCall, WindowKind};
use crate::physical::Plan;
use crate::storage::{BATCH_ROWS, Batch, Column, Row, Sel};
use crate::txn::{TxId, checked};

/// \[DESIGN\] How many parts of the work area the rows held take at most,
/// the rest left to the trees each call holds.
const PARTS: usize = 2;

/// A call's argument of a batch: one of its columns, or a vector made.
pub(in crate::exec::window) enum Arg {
    /// The batch's column at this place.
    At(usize),
    /// A vector evaluated of the batch.
    Made(Column),
}

/// A batch held, its calls' arguments, and of each row whether it begins
/// a partition and whether a peer group.
pub(in crate::exec::window) struct Held {
    pub(in crate::exec::window) batch: Batch,
    pub(in crate::exec::window) args: Vec<Vec<Arg>>,
    pub(in crate::exec::window) parts: Vec<bool>,
    pub(in crate::exec::window) peers: Vec<bool>,
}

/// Whether the window `w` is computed over batches: unless a call is one
/// the rows' form alone computes.
pub(in crate::exec) fn admits<P>(w: &Window<P>) -> bool {
    w.calls.iter().all(admitted)
}

/// Whether `call` is computed over batches.
fn admitted(call: &WindowCall) -> bool {
    if !call.nested.is_empty() || call.frame.matching.is_some() {
        return false;
    }
    match &call.kind {
        WindowKind::Aggregate(a) => a.filter.is_none() && !a.distinct && a.distribution.is_none(),
        WindowKind::Measure(_) => false,
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_)
        | WindowKind::Offset(_)
        | WindowKind::Value(_) => true,
    }
}

/// The expressions of `call` evaluated of each row, in order.
fn arguments(call: &WindowCall) -> Vec<&Expr> {
    match &call.kind {
        WindowKind::Aggregate(a) => a.arg.iter().collect(),
        WindowKind::Offset(o) => vec![&o.extent, &o.default],
        WindowKind::Value(v) => vec![&v.arg],
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_)
        | WindowKind::Measure(_) => Vec::new(),
    }
}

/// [ISO/IEC 9075-2:2023 §7.15] The batches of the window `plan`, each of
/// its input's rows and a column of each call's values.
///
/// # Errors
///
/// As its input's batches fail.
pub(in crate::exec) fn window_batches(plan: Plan, db: &Db, tx: TxId) -> Result<Batches> {
    let Plan::Window { window, .. } = plan else { return Err(crate::exec::broken("a window is not one")) };
    let (w, input) = window.split();
    let partition: Vec<SortKey> = w.partition.iter().map(|&c| SortKey::ascending(c)).collect();
    let both: Vec<SortKey> = partition.iter().chain(&w.order).copied().collect();
    let keyed = input_of(input, (partition.len(), &both), db, tx)?;
    let limit = Work::of(db, tx).limit();
    let windowing = Windowing {
        limit,
        w,
        sorted: (partition, both),
        input: Some(keyed),
        prepared: None,
        runner: Runner::new(db, tx),
        db: db.clone(),
        tx,
        kernels: db.functions.kernels_shared(),
        held: Vec::new(),
        bytes: 0,
        last: None,
        out: VecDeque::new(),
        rows: None,
    };
    Ok(Box::new(checked(windowing, db.tx.stop_of(tx))))
}

/// The input of a window, `input`, as batches in order of `both`, its
/// partitioning's and ordering's keys: of a sort by those keys whose input
/// gives whole batches, with the keys it made; of a stream giving whole
/// batches, as they come; else its rows made batches.
fn input_of(input: Plan, (parted, both): (usize, &[SortKey]), db: &Db, tx: TxId) -> Result<KeyedBatches> {
    if let Plan::Sort { keys, input: inner, most: None, .. } = &input
        && sorts_as(keys, (parted, both))
        && let Some(width) = whole_width(inner, db)
    {
        let Plan::Sort { keys: by, input: sorted, .. } = input else { return Err(crate::exec::broken("a sort is not one")) };
        return sorted_keyed(&by, whole_batches(*sorted, db, tx)?, width, db, tx);
    }
    if whole_width(&input, db).is_some() {
        let batches = whole_batches(input, db, tx)?;
        return Ok(Box::new(batches.map(|kept| kept.map(|(batch, sel)| (compact(batch, sel), None)))));
    }
    Ok(Box::new(rows_made_batches(execute(input, db, tx)).map(|b| b.map(|b| (b, None)))))
}

/// Whether a sort by `keys` gives rows in the order of `both`, a window's
/// partitioning's keys then its ordering's: of its columns, each
/// partitioning key in either direction, as a partition's rows are
/// together either way, then the ordering's keys as they are.
fn sorts_as(keys: &[SortKey], (parted, both): (usize, &[SortKey])) -> bool {
    let alike = |(n, (k, b)): (usize, (&SortKey, &SortKey))| if n < parted { k.column == b.column } else { k == b };
    keys.len() == both.len() && keys.iter().zip(both).enumerate().all(alike)
}

/// `batch`, of the rows `sel` keeps alone if given.
fn compact(batch: Batch, sel: Option<Sel>) -> Batch {
    let Some(sel) = sel else { return batch };
    let picks: Vec<Option<usize>> = sel.places().map(Some).collect();
    let columns = batch.columns.iter().map(|c| c.gathered(&picks)).collect();
    Batch { columns, rows: picks.len() }
}

/// \[DESIGN\] The rows of `rows` as batches of up to `BATCH_ROWS`, of every
/// column of the first: an error given after a batch of the rows before
/// it, so they come first (RULES.md D15).
fn rows_made_batches(rows: RowIter) -> impl Iterator<Item = Result<Batch>> + Send {
    let (mut rows, mut failed) = (rows, None);
    std::iter::from_fn(move || {
        if let Some(e) = failed.take() {
            return Some(Err(e));
        }
        let mut made: Option<Batch> = None;
        while made.as_ref().is_none_or(|b| b.rows < BATCH_ROWS) {
            match rows.next() {
                Some(Ok(row)) => push_row(&mut made, &row),
                Some(Err(e)) => {
                    if made.is_none() {
                        return Some(Err(e));
                    }
                    failed = Some(e);
                    break;
                }
                None => break,
            }
        }
        made.map(Ok)
    })
}

/// Adds `row` to `made`, a batch of its width begun if none.
fn push_row(made: &mut Option<Batch>, row: &Row) {
    let columns: Vec<usize> = (0..row.len()).collect();
    made.get_or_insert_with(|| Batch::empty(row.len())).push(row, &columns);
}

/// A window over batches, as it reads its input.
struct Windowing {
    w: Window<()>,
    /// The partitioning's keys, then those and the ordering's.
    sorted: (Vec<SortKey>, Vec<SortKey>),
    /// Its input, none once read to its end.
    input: Option<KeyedBatches>,
    /// Its calls' frames and counts, prepared as the first row is read.
    prepared: Option<Prepared>,
    runner: Runner,
    db: Db,
    tx: TxId,
    kernels: Arc<dyn Kernels>,
    /// The batches held, the first beginning a partition.
    held: Vec<Held>,
    /// The bytes they hold.
    bytes: usize,
    /// The bytes of the work area.
    limit: usize,
    /// The last row read.
    last: Option<Last>,
    /// Batches computed, to give.
    out: VecDeque<Batch>,
    /// Once given way, the rows' form's batches.
    rows: Option<Box<dyn Iterator<Item = Result<Batch>> + Send>>,
}

impl Iterator for Windowing {
    type Item = Result<(Batch, Option<Sel>)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(batch) = self.out.pop_front() {
                return Some(Ok((batch, None)));
            }
            if let Some(rows) = &mut self.rows {
                return rows.next().map(|b| b.map(|b| (b, None)));
            }
            let read = self.input.as_mut()?.next();
            if let Err(e) = self.take(read) {
                self.input = None;
                return Some(Err(e));
            }
        }
    }
}

/// Of `call`'s frame bounds and count, prepared: unused of a call with no
/// frame read.
fn prepared(w: &Window<()>, runner: &Runner) -> Result<Prepared> {
    let order = w.order.first().map(SortKey::order);
    w.calls.iter().map(|c| Ok((Bounds::of(&c.frame, order, runner)?, Count::of(&c.kind, runner)?))).collect()
}
