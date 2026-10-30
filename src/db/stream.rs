//! A read-only cursor's rows: read from its plan as the
//! cursor is fetched from, rather than all at OPEN, so a cursor over a
//! large table holds one row at a time. Split from db/cursor.rs (RULES.md
//! R-SIZE-2).
//!
//! OPEN creates the cursor's table [X3.135-1986 §8.8 GR3b]; reading it
//! later must give the same rows, and fail as OPEN would have. So:
//!
//! * a plan that can raise an error of a General Rule (an overflow, a
//!   division by zero) is read in full at OPEN, which raises it: whether it
//!   is raised cannot depend on how far the cursor is fetched (RULES.md
//!   D15);
//! * before the transaction changes a table the cursor reads, the cursor
//!   reads the rest of its rows (`settle`), so it never sees the change.
//!   Other transactions' changes it cannot see: they wait for its locks, or
//!   its engine reads as of its snapshot (RULES.md D29).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::{Completion, Database, State};
use crate::binder::OutColumn;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::error::{Code, fail};
use crate::exec::{RowIter, execute};
use crate::physical::Plan;
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::txn::{Stop, TxId};
use crate::value::datetime::show_in;

/// \[DESIGN\] The rows of a read-only cursor.
pub(super) struct Stream {
    /// Rows read and not yet fetched, an error where reading one failed.
    ready: VecDeque<Result<Row>>,
    /// The rows still to read from the plan; none once all are read.
    rest: Option<RowIter>,
    /// The base tables the plan reads, its subqueries' too.
    reads: Vec<QualifiedName>,
}

impl Stream {
    /// The rows of `physical`, which reads `reads`, for transaction `tx`:
    /// read now if the plan can raise an error, else as they are fetched.
    ///
    /// # Errors
    ///
    /// The plan, read now, raises one.
    pub(super) fn open(physical: Plan, reads: Vec<QualifiedName>, st: &Arc<State>, tx: TxId) -> Result<Stream> {
        let read_now = physical.can_raise();
        let rows = execute(physical, st, tx);
        if read_now {
            let all = rows.collect::<Result<Vec<Row>>>()?;
            return Ok(Stream { ready: all.into_iter().map(Ok).collect(), rest: None, reads });
        }
        Ok(Stream { ready: VecDeque::new(), rest: Some(rows), reads })
    }

    /// The next row, or none after the last.
    ///
    /// # Errors
    ///
    /// Reading it failed.
    pub(super) fn next(&mut self) -> Result<Option<Row>> {
        if let Some(read) = self.ready.pop_front() {
            return read.map(Some);
        }
        let read = self.rest.as_mut().and_then(Iterator::next);
        if read.is_none() {
            self.rest = None;
        }
        read.transpose()
    }

    /// [X3.135-1986 §8.8 GR3b] Reads the rest of the rows now if the cursor
    /// reads `table`, which its transaction is about to change.
    pub(super) fn settle(&mut self, table: &QualifiedName) {
        if !self.reads.contains(table) {
            return;
        }
        if let Some(rest) = self.rest.take() {
            self.ready.extend(rest);
        }
    }

    /// Whether it reads `table`, or did.
    pub(super) fn reads(&self, table: &QualifiedName) -> bool {
        self.reads.contains(table)
    }

    /// Whether rows are still read from the plan as they are fetched.
    #[cfg(test)]
    pub(super) fn streaming(&self) -> bool {
        self.rest.is_some()
    }
}

/// \[EXTENSION\] What a `<direct SQL statement>` gives (`Session::direct`):
/// the rows of a `<direct select statement: multiple rows>`, or the
/// completion of any other [ISO/IEC 9075:1992 §20.1, §20.2].
pub enum Direct {
    /// Its rows, as a cursor reads them.
    Rows(Cursor),
    /// Its completion.
    Done(Completion),
}

/// \[DESIGN\] The rows of an unnamed cursor opened with `Session::open_cursor`.
/// `fetch` returns `Ok(None)` after the last row: SQLCODE 100 [X3.135-1986 §8.6 GR2].
///
/// Its rows are read in its session's transaction as it is fetched from
/// (`db/stream.rs`); the session's statements and the cursor's fetches may
/// run on different threads.
pub struct Cursor {
    /// The columns of the rows, with their names and types.
    pub columns: Vec<OutColumn>,
    /// Its rows, shared with the session; none once its transaction ended.
    pub(super) rows: Arc<Mutex<Option<Stream>>>,
    /// Its database, told of a failure reading a row (RULES.md D40).
    pub(super) db: Database,
    /// Its session's, which cancels a fetch as a statement (RULES.md D44).
    pub(super) stop: Arc<Stop>,
    /// Its work, timed, to tell if it is slow (`db/slow.rs`).
    pub(super) timed: Option<super::slow::Timed<String>>,
    /// Its session's time zone displacement as it was opened, its times
    /// and timestamps without time zone shown in [ISO/IEC 9075:1992 §6.1
    /// GR5].
    pub(super) zone: i16,
}

impl Drop for Cursor {
    fn drop(&mut self) {
        if let Some(timed) = self.timed.take() {
            timed.done(&self.db);
        }
    }
}

impl Cursor {
    /// The next row, or `None` after the last.
    ///
    /// # Errors
    ///
    /// SQLCODE -501 if the cursor's transaction has ended; reading the row
    /// failed.
    pub fn fetch(&mut self) -> Result<Option<Row>> {
        let _window = self.stop.window();
        if let Some(timed) = self.timed.as_mut() {
            timed.resumed();
        }
        let next = match lock_builtin(&self.rows).as_mut() {
            Some(rows) => rows.next(),
            None => fail(Code::Cursor, "[X3.135-1986 §8.2 GR2, §8.9 GR2] the cursor was closed when its transaction ended"),
        };
        if let Some(timed) = self.timed.as_mut() {
            timed.paused();
        }
        let mut next = self.db.noted(next)?;
        if let Some(row) = next.as_mut() {
            show_in(row, self.zone);
        }
        Ok(next)
    }
}

// Unit tests: which cursors read their rows as they are fetched from
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::super::{CursorRows, Database, OpenCursor, Session};
    use crate::params::Params;

    /// Whether the open cursor `name` of `s` still reads from its plan.
    fn streaming(s: &Session, name: &str) -> bool {
        match s.inner.borrow().cursors.get(name).and_then(|c| c.open.as_ref()) {
            Some(OpenCursor { rows: CursorRows::Table(rows), .. }) => rows.streaming(),
            _ => panic!("{name} is not an open read-only cursor"),
        }
    }

    /// Verifies: [DESIGN] read as fetched from, [X3.135-1986 §8.8 GR3b] a cursor
    /// whose plan raises no error reads as it is fetched from, until its
    /// transaction changes a table it reads, its subqueries' too; changing
    /// another leaves it so; a plan that can raise an error is read at OPEN.
    #[test]
    fn cursors_read_as_they_are_fetched_from() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INT) CREATE TABLE U (K INT) CREATE TABLE V (K INT)").unwrap();
        let s = db.session("A");
        let mut p = Params::new();
        s.declare_cursor("DECLARE C CURSOR FOR SELECT DISTINCT K FROM T WHERE K IN (SELECT K FROM V)").unwrap();
        s.declare_cursor("DECLARE D CURSOR FOR SELECT K + 1 FROM T").unwrap();
        s.execute("OPEN C", &mut p).unwrap();
        s.execute("OPEN D", &mut p).unwrap();
        assert!(streaming(&s, "C"));
        assert!(!streaming(&s, "D"), "K + 1 can overflow");
        s.run("INSERT INTO U VALUES (1)").unwrap();
        assert!(streaming(&s, "C"), "U is not read");
        s.run("INSERT INTO V VALUES (1)").unwrap();
        assert!(!streaming(&s, "C"), "V is read by the subquery");
    }
}
