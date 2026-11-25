//! \[DESIGN\] Morsel-driven parallel execution (RULES.md D290 to D299), as
//! Leis, Boncz, Kemper and Neumann describe it in "Morsel-Driven
//! Parallelism: A NUMA-Aware Query Evaluation Framework for the Many-Core
//! Age" (2014). A part of a plan the planner marked (`physical/parallel.rs`)
//! runs on the database's workers (`threads/pool.rs`) and the statement's
//! own thread: a pipeline's tables split into morsels, each morsel taken
//! through the pipeline's steps by whichever thread takes it
//! (`parallel/pipeline.rs`), and the operator ending the pipeline, if any,
//! made of each morsel's part, then of each partition's, on the threads
//! too: a grouping or DISTINCT (`parallel/grouped.rs`), a sort
//! (`parallel/sorted.rs`), window functions, EXCEPT and INTERSECT
//! (`parallel/partitioned.rs`), and a hash join's built operand
//! (`parallel/probe.rs`). No operator passes rows to another between
//! threads, as a morsel's rows go through its pipeline on one.
//!
//! Every part gives the rows the plan alone gives, in its order: a
//! pipeline's morsels in the order of their numbers, an operator's groups
//! or rows by their first row's or their keys' order, so a statement's
//! result does not depend on its degree. Every thread reads as the
//! statement's transaction does, in its snapshot and with its own writes
//! (RULES.md D29); each morsel looks at its cancellation (RULES.md D44);
//! the first error in the order of the rows is the one raised, and none
//! after it is read. A part holds no more than one work grant, split among
//! its partitions (RULES.md D38, D298). A sort that would hold more spills
//! its runs, and a window's or a set operation's partition its rows; a
//! grouping or a join's built operand that would hold more gives way to
//! the plan run alone, which spills, reading its rows again in the
//! statement's snapshot, as does a part of a table its engine cannot
//! split.

mod batched;
mod batches;
mod build;
mod charged;
mod each;
mod grouped;
mod indexed;
mod partitioned;
mod pipeline;
mod probe;
mod rows;
mod run;
mod sorted;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) use self::batches::streamed_batches;
use self::charged::{Ahead, Alone, Flow, whole_rows};
pub(super) use self::each::each_morsel;
pub(super) use self::grouped::grouped_told;
use self::pipeline::Pipeline;
pub(super) use self::probe::pipelined;
pub(super) use self::run::Morsels;
use super::{RowIter, Runner, broken, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::physical::{Parallel, Plan};
use crate::settings::{EXECUTOR_MORSEL_ROWS, MEMORY_WORK_BYTES, work_bytes};
use crate::spill::Grant;
use crate::storage::Row;
use crate::txn::{Stop, TxId};

/// What a part holds, or that it gave way to the plan run alone.
pub(in crate::exec) enum Held<T> {
    Kept(T),
    GaveWay,
}

/// \[DESIGN\] The bytes a part holds, charged to one work grant of the
/// statement's (RULES.md D38, D298), shared by the threads charging it:
/// a copy charges the same grant, so each thread reading a morsel holds
/// one (`parallel/charged.rs`).
#[derive(Clone)]
struct Limit {
    most: usize,
    held: Arc<AtomicUsize>,
    grant: Arc<Grant>,
}

impl Limit {
    /// A work grant of `db`'s budget, of up to MEMORY.WORK_BYTES.
    fn of(db: &State) -> Limit {
        let grant = db.work.grant(work_bytes(db.settings.get(MEMORY_WORK_BYTES)));
        Limit { most: grant.bytes(), held: Arc::new(AtomicUsize::new(0)), grant }
    }

    /// Its bytes.
    fn most(&self) -> usize {
        self.most
    }

    /// Charges `bytes`: whether they fit.
    fn take(&self, bytes: usize) -> bool {
        let held = self.held.fetch_add(bytes, Ordering::AcqRel).saturating_add(bytes);
        held <= self.most
    }

    /// Gives back `bytes` it was charged, no longer held.
    fn give(&self, bytes: usize) {
        let mut seen = self.held.load(Ordering::Acquire);
        while let Err(now) = self.held.compare_exchange_weak(seen, seen.saturating_sub(bytes), Ordering::AcqRel, Ordering::Acquire) {
            seen = now;
        }
    }

    /// What it holds.
    fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// Whether what it holds, the bytes taken since it held `from` after
    /// `done` of `count` morsels counted as much again for each morsel
    /// left, would fit: a part that will pass its grant gives way after
    /// its first morsels, not after its last.
    fn on_course(&self, from: usize, (done, count): (usize, usize)) -> bool {
        let grown = self.held().saturating_sub(from);
        from.saturating_add(grown.saturating_mul(count).checked_div(done).unwrap_or(grown)) <= self.most
    }

    /// `rows`, its grant held until they are dropped.
    fn keeping(&self, rows: RowIter) -> RowIter {
        let grant = Arc::clone(&self.grant);
        Box::new(rows.inspect(move |_| {
            let _held = &grant;
        }))
    }
}

/// A part run in parallel: of each, its rows, or that it gave way.
type Runs = fn(&Plan, (&Db, TxId), usize) -> Result<Held<RowIter>>;

/// The rows of `plan`, a part run in parallel, read as `tx` sees them; of
/// one that gave way, those of its plan run alone. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack
/// (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn parallel(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Parallel(p) = plan else { return failed(broken("a part run in parallel is not one")) };
    let Parallel { degree, input } = *p;
    // [EXTENSION] the operator at its top counted as the part's rows (RULES.md D378).
    if let Plan::Counted(c) = input {
        return super::counted::through(*c, db, |top| parallel(Plan::Parallel(Box::new(Parallel { degree, input: top })), db, tx));
    }
    let run: Runs = if grouped::is_one(&input) {
        grouped::grouped
    } else if matches!(input, Plan::Sort { .. }) {
        sorted::sorted
    } else if matches!(input, Plan::Window { .. }) {
        partitioned::windowed
    } else if matches!(input, Plan::SetOp { .. }) {
        partitioned::set_operation
    } else {
        streamed
    };
    match run(&input, (db, tx), degree) {
        Ok(Held::Kept(rows)) => rows,
        Ok(Held::GaveWay) => execute(input, db, tx),
        Err(e) => failed(e),
    }
}

/// The rows of the stream `plan`, its morsels taken by up to `degree`
/// threads, given in order: each morsel's gathered within the part's
/// grant, or read by the reader as it is given if it outgrows it
/// (`parallel/charged.rs`, RULES.md D1150).
fn streamed(plan: &Plan, (db, tx): (&Db, TxId), degree: usize) -> Result<Held<RowIter>> {
    let (limit, ahead) = (Limit::of(db), Ahead::of(degree, db.settings.count(EXECUTOR_MORSEL_ROWS)));
    let Held::Kept(pipeline) = Pipeline::of(plan, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let (pipeline, db_for, within) = (Arc::new(pipeline), db.clone(), (limit.clone(), ahead.clone()));
    let (count, read, db_alone) = (pipeline.count(), Arc::clone(&pipeline), db.clone());
    let work = Box::new(move |m: usize| {
        whole_rows(pipeline.morsel_rows(m, (&db_for, tx), Runner::new(&db_for, tx))?, (&within.0, &within.1))
    });
    let alone: Alone<Row> = Box::new(move |m| read.morsel_rows(m, (&db_alone, tx), Runner::new(&db_alone, tx)));
    let morsels = Morsels::start(db, tx, (count, degree), work);
    Ok(Held::Kept(Box::new(Flow::of_rows(morsels, alone, (&limit, &ahead), db.tx.stop_of(tx)))))
}

/// Rows given a morsel's at a time, in order, the statement's
/// cancellation looked at as each morsel's are taken up (RULES.md D44).
struct Streamed {
    morsels: Morsels<Vec<Row>>,
    rows: std::vec::IntoIter<Row>,
    stop: Option<Arc<Stop>>,
    ended: bool,
}

impl Streamed {
    /// The rows of `morsels`, for the statement of `tx`.
    fn of(db: &State, tx: TxId, morsels: Morsels<Vec<Row>>) -> Streamed {
        Streamed { morsels, rows: Vec::new().into_iter(), stop: db.tx.stop_of(tx), ended: false }
    }
}

impl Iterator for Streamed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(row) = self.rows.next() {
                return Some(Ok(row));
            }
            if self.ended {
                return None;
            }
            let taken = self.stop.as_ref().map_or(Ok(()), |s| s.check()).and_then(|()| self.morsels.next().transpose());
            match taken {
                Ok(Some(rows)) => self.rows = rows.into_iter(),
                Ok(None) => return None,
                Err(e) => {
                    // A failed morsel ends its run: a cursor cut short stays failed.
                    self.ended = true;
                    return Some(Err(e));
                }
            }
        }
    }
}
