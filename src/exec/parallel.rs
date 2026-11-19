//! \[DESIGN\] Morsel-driven parallel execution (RULES.md D290 to D296), as
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
//! threads: a morsel's rows go through its pipeline on one.
//!
//! Every part gives the rows the plan alone gives, in its order: a
//! pipeline's morsels in the order of their numbers, an operator's groups
//! or rows by their first row's or their keys' order, so a statement's
//! result does not depend on its degree. Every thread reads as the
//! statement's transaction does, in its snapshot and with its own writes
//! (RULES.md D29); each morsel looks at its cancellation (RULES.md D44);
//! the first error in the order of the rows is the one raised, and none
//! after it is read. A part holds no more than one work area, split among
//! its partitions (RULES.md D38). One that would hold more gives way to
//! the plan run alone, which spills, reading its rows again in the
//! statement's snapshot, as does a part of a table its engine cannot
//! split.

mod grouped;
mod indexed;
mod partitioned;
mod pipeline;
mod probe;
mod run;
mod sorted;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use self::pipeline::Pipeline;
use self::run::Morsels;
use super::{RowIter, Runner, broken, execute, failed};
use crate::db::State;
use crate::error::Result;
use crate::physical::{Parallel, Plan};
use crate::settings::{MEMORY_WORK_BYTES, work_bytes};
use crate::spill::Grant;
use crate::storage::Row;
use crate::txn::{Stop, TxId};

/// What a part holds, or that it gave way to the plan run alone.
enum Held<T> {
    Kept(T),
    GaveWay,
}

/// \[DESIGN\] The bytes a part holds, charged to one work grant of the
/// statement's (RULES.md D38, D298), shared by the threads charging it.
struct Limit {
    most: usize,
    held: AtomicUsize,
    grant: Arc<Grant>,
}

impl Limit {
    /// A work grant of `db`'s budget, of up to MEMORY.WORK_BYTES.
    fn of(db: &State) -> Limit {
        let grant = db.work.grant(work_bytes(db.settings.get(MEMORY_WORK_BYTES)));
        Limit { most: grant.bytes(), held: AtomicUsize::new(0), grant }
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

    /// `rows`, its grant held until they are dropped.
    fn keeping(&self, rows: RowIter) -> RowIter {
        let grant = Arc::clone(&self.grant);
        Box::new(rows.inspect(move |_| {
            let _held = &grant;
        }))
    }
}

/// A part run in parallel: of each, its rows, or that it gave way.
type Runs = fn(&Plan, (&Arc<State>, TxId), usize) -> Result<Held<RowIter>>;

/// The rows of `plan`, a part run in parallel, read as `tx` sees them; of
/// one that gave way, those of its plan run alone. Never inlined into
/// `execute`, whose frame every level of a plan adds to the stack
/// (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn parallel(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Parallel(p) = plan else { return failed(broken("a part run in parallel is not one")) };
    let Parallel { degree, input } = *p;
    let run: Runs = if matches!(input, Plan::Aggregate { .. } | Plan::Distinct { .. }) {
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
/// threads, given in order.
fn streamed(plan: &Plan, (db, tx): (&Arc<State>, TxId), degree: usize) -> Result<Held<RowIter>> {
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(plan, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let (pipeline, db_for) = (Arc::new(pipeline), Arc::clone(db));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| pipeline.morsel(m, &Runner::new(&db_for, tx)));
    Ok(Held::Kept(Box::new(Streamed::of(db, tx, Morsels::start(db, tx, (count, degree), work)))))
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
