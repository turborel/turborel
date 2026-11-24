//! \[DESIGN\] An export's run (RULES.md D975): the query prepared, its rows
//! read and written by its layout. A query none of whose columns may hold
//! a large object is executed once, all its rows sent, each Rows reply
//! written as it comes. Else its cursor is read a batch at a time, a
//! batch written, its large objects read by their locators, before the
//! next is fetched, so no request goes out while a reply is coming. The
//! transaction its locators are valid in is kept until the last is read:
//! under autocommit, one the export began it commits at its end, or rolls
//! back if the export failed.

use std::io::{self, Read, Write};

use turborel::value::lob::Lob;
use turborel_protocol::{Failure, Reply, Request};

use super::{Layout, Lobs, Reading, may_hold_lob};
use crate::connection::{Connection, Result, failed};
use crate::statements::{Statement, execute_request};

/// \[DESIGN\] The rows an export of a query that may hold large objects
/// fetches at a time: as many as there are large objects sent whole at
/// their longest (`LOB_INLINE_OCTETS`, 64 KiB) in a reply's longest body
/// (`MAX_BODY`, 16 MiB), so a batch held while its large objects are read
/// is no more than a reply's longest body for each such column.
pub(crate) const EXPORT_ROWS: u32 = 256;

/// Ends a transaction the export began [ISO/IEC 9075:1992 §14.3].
const COMMIT: &str = "COMMIT";
/// Undoes one, if the export failed [ISO/IEC 9075:1992 §14.4].
const ROLLBACK: &str = "ROLLBACK";

/// What reads no large object: a query whose columns hold none sends none
/// by a locator, so one that came is the server's fault.
struct NoLobs;

impl Lobs for NoLobs {
    fn chunks(&mut self, lob: &Lob, each: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        let Some(bytes) = lob.inline() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a large object came by its locator in a column of no such type"));
        };
        each(bytes)
    }
}

/// The failure of an export whose write failed with `e`: a read's that
/// failed under it, if one did.
fn unwritten(e: &io::Error, read: Option<Failure>) -> Failure {
    read.unwrap_or_else(|| failed(&format!("[DESIGN] cannot write the export: {e}")))
}

impl<S: Read + Write> Connection<S> {
    /// \[DESIGN\] RULES.md D975: runs the query `sql` and writes its rows to
    /// `out` by `layout`, as they come: how many. Each large object sent by
    /// its locator is read as its batch is written.
    ///
    /// # Errors
    ///
    /// The query's, as `prepare` and `open_cursor` give them; a statement
    /// that is no query; a large object's read (SQLSTATE 0F001 if its
    /// locator is no longer valid); a write to `out` that failed (SQLSTATE
    /// 08006 is not the connection's then, but the export's, which ends).
    pub fn export_with(&mut self, sql: &str, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64> {
        let statement = self.prepare(sql)?;
        if statement.columns().is_empty() {
            self.deallocate(statement)?;
            return Err(failed("[DESIGN] an export runs a query"));
        }
        let lobs = statement.columns().iter().any(|c| may_hold_lob(&c.data_type));
        // Under autocommit, the transaction the cursor is read in is the
        // export's to end once its last large object is read.
        let begun = lobs && self.autocommit && !self.in_transaction();
        let run = match layout.columns(out, statement.columns()) {
            Err(e) => Err(unwritten(&e, None)),
            Ok(()) if lobs => self.batched(&statement, out, layout),
            Ok(()) => self.streamed(&statement, out, layout),
        };
        let ended = self.ended(statement, &run, begun);
        let count = run?;
        ended?;
        layout.end(out).and_then(|()| out.flush()).map_err(|e| unwritten(&e, None))?;
        Ok(count)
    }

    /// `statement`'s rows, all sent at once, each Rows reply written by
    /// `layout` as it comes; the replies read to the end if a write fails,
    /// so the connection takes more.
    fn streamed(&mut self, statement: &Statement, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64> {
        self.send(&execute_request(statement, (0, &[]), 0, (self.autocommit, self.told.attributes())))?;
        let (mut count, mut unwrote) = (0_u64, None);
        loop {
            match self.receive()? {
                Reply::Rows(batch) => {
                    count = count.saturating_add(u64::try_from(batch.len()).unwrap_or(u64::MAX));
                    if unwrote.is_none() {
                        unwrote = layout.rows(out, &batch, &mut NoLobs).err();
                    }
                }
                Reply::Complete(_) => return unwrote.map_or(Ok(count), |e| Err(unwritten(&e, None))),
                Reply::Error(failure) => return Err(self.failed(failure)),
                Reply::Lob(_)
                | Reply::Challenge(_)
                | Reply::Ready { .. }
                | Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Suspended
                | Reply::Notice(_)
                | Reply::Status { .. }
                | Reply::Pong
                | Reply::Debug(_)
                | Reply::Event(_) => return Err(self.lose("[DESIGN] the server broke off its rows")),
            }
        }
    }

    /// `statement`'s rows, its cursor read `EXPORT_ROWS` at a time within
    /// the transaction, each batch written by `layout` before the next is
    /// fetched, its large objects read meanwhile.
    fn batched(&mut self, statement: &Statement, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64> {
        self.send(&execute_request(statement, (0, &[]), EXPORT_ROWS, (false, self.told.attributes())))?;
        let mut count = 0_u64;
        loop {
            let (rows, more) = self.rows()?;
            count = count.saturating_add(u64::try_from(rows.len()).unwrap_or(u64::MAX));
            let mut reading = Reading { connection: self, failure: None };
            if let Err(e) = layout.rows(out, &rows, &mut reading) {
                return Err(unwritten(&e, reading.failure));
            }
            if !more {
                return Ok(count);
            }
            self.send(&Request::Fetch { statement: statement.number, cursor: 0, rows: EXPORT_ROWS })?;
        }
    }

    /// Ends the export's use of `statement`: its cursor closed if the
    /// export failed with it open, the statement destroyed, and the
    /// transaction the export `begun` committed if its `run` succeeded,
    /// else rolled back.
    fn ended(&mut self, statement: Statement, run: &Result<u64>, begun: bool) -> Result<()> {
        let succeeded = run.is_ok();
        if self.is_lost() {
            return Ok(());
        }
        if !succeeded {
            // Its cursor may be closed already, as the failure closed it:
            // 24000 then, which changes nothing.
            drop(self.close(&statement));
        }
        let freed = self.deallocate(statement);
        if begun && self.in_transaction() {
            self.query(if succeeded { COMMIT } else { ROLLBACK })?;
        }
        freed
    }
}
