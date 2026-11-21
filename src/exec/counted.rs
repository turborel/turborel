//! \[EXTENSION\] Running an operator EXPLAIN ANALYZE counts (RULES.md D376,
//! D378): as its input, the rows it gives and the time it takes, from its
//! opening through each row pulled, tallied by the iterator and added to
//! its count once, as the iterator is dropped, so a thread adds what it
//! counted once and never contends a row at a time. A batch is counted as
//! one pull, its rows kept all at once (`exec/batch/counted.rs`). Time is
//! the database's clock's (`Clock`), so a clock that stands still, as a
//! test's does, takes none (RULES.md R-TEST-5). As it is opened and each
//! pull, its notes are the thread's, so what it spills and reads below the
//! pull is noted of it (`noted.rs`, RULES.md D620).

use std::sync::Arc;

use super::entries::Entries;
use super::{RowIter, broken, execute, failed};
use crate::clock::Clock;
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::noted::{self, Noted};
use crate::physical::{Counted, NodeCount, Plan};
use crate::storage::Row;
use crate::txn::TxId;

/// The rows of `plan`, a counted operator, counted.
#[inline(never)]
pub(super) fn counted(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Counted(c) = plan else { return failed(broken("a counted operator is not one")) };
    through(*c, db, |input| execute(input, db, tx))
}

/// The rows `run` gives of `c`'s operator, counted as `c`'s: its opening
/// timed, then each row pulled.
pub(super) fn through(c: Counted, db: &State, run: impl FnOnce(Plan) -> RowIter) -> RowIter {
    let Counted { count, input } = c;
    let mut tally = Tally::opened(count, db);
    let rows = tally.timed(|| run(input));
    Box::new(CountedRows { rows, tally })
}

/// The rows with their ids `run` gives of `c`'s operator, of the access
/// of a table whose rows a data change changes (`entries.rs`, RULES.md
/// D622), counted as `c`'s.
///
/// # Errors
///
/// As `run`.
pub(super) fn entries_through(c: Counted, db: &State, run: impl FnOnce(Plan) -> Result<Entries>) -> Result<Entries> {
    let Counted { count, input } = c;
    let mut tally = Tally::opened(count, db);
    let mut rows = tally.timed(|| run(input))?;
    Ok(Box::new(std::iter::from_fn(move || {
        let pulled = tally.timed(|| rows.next());
        if let Some(Ok(_)) = pulled {
            tally.rows(1);
        }
        pulled
    })))
}

/// \[DESIGN\] What one iterator of a counted operator counted, added to its
/// count when it is dropped.
pub(in crate::exec) struct Tally {
    count: Arc<NodeCount>,
    clock: Arc<dyn Clock>,
    rows: u64,
    micros: u64,
    /// The batches it gave.
    batches: u64,
}

impl Tally {
    /// The tally of an opening of the operator `count` counts, timed by
    /// `db`'s clock.
    pub(in crate::exec) fn opened(count: Arc<NodeCount>, db: &State) -> Tally {
        count.opened();
        Tally { count, clock: Arc::clone(&db.clock), rows: 0, micros: 0, batches: 0 }
    }

    /// `f`, its time added, run with the operator's notes the thread's.
    pub(in crate::exec) fn timed<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let began = self.clock.now();
        let made = noted::within(Some(Arc::clone(self.count.notes())), f);
        let took = u64::try_from(self.clock.now().saturating_sub(began)).unwrap_or_default();
        self.micros = self.micros.saturating_add(took);
        made
    }

    /// Adds `rows` rows.
    pub(in crate::exec) fn rows(&mut self, rows: u64) {
        self.rows = self.rows.saturating_add(rows);
    }

    /// Adds a batch of `rows` rows.
    pub(in crate::exec) fn batch(&mut self, rows: u64) {
        self.batches = self.batches.saturating_add(1);
        self.rows(rows);
    }
}

impl Drop for Tally {
    fn drop(&mut self) {
        self.count.add(self.rows, self.micros);
        if self.batches > 0 {
            self.count.notes().add(Noted::Batches, self.batches);
        }
    }
}

/// The rows of a counted operator, each counted and its pull timed.
struct CountedRows {
    rows: RowIter,
    tally: Tally,
}

impl Iterator for CountedRows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let rows = &mut self.rows;
        let pulled = self.tally.timed(|| rows.next());
        if let Some(Ok(_)) = pulled {
            self.tally.rows(1);
        }
        pulled
    }
}
