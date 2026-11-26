//! \[DESIGN\] Bulk copy in (RULES.md D195): rows sent into a table with no
//! round trip but the copy's end. `copy_in` begins one, naming the table
//! and its columns; `Copying::send` sends rows, in chunks of about
//! `CHUNK_BYTES`, each with no reply; `finish` ends it, its reply the rows
//! inserted, or the copy's failure, every row of it undone. The server
//! assigns each value to its column as INSERT does, so a string too long
//! for its column is refused, not cut; a copy is all or nothing, within
//! the transaction, or its own under `set_autocommit`.
//!
//! A copy dropped before it ends is abandoned, so the connection takes
//! requests again.

use std::io::{Read, Write};
use std::mem;

use turborel::storage::Row;
use turborel::storage::lanes::Lanes;
use turborel_protocol::capability::{COLUMNS, COPY};
use turborel_protocol::{Request, row_size};

use crate::connection::{Connection, Result};

/// \[DESIGN\] The bytes of rows a CopyData carries at most, about: enough
/// that a message's cost is shared by many rows, few enough that neither
/// side holds much.
pub const CHUNK_BYTES: usize = 256 * 1024;

/// A copy under way, over its connection.
pub struct Copying<'c, S: Read + Write> {
    connection: &'c mut Connection<S>,
    /// The rows waiting to be sent, and their bytes.
    waiting: Vec<Row>,
    bytes: usize,
    /// Whether it ended, finished or abandoned.
    ended: bool,
}

impl<S: Read + Write> Connection<S> {
    /// \[DESIGN\] RULES.md D195: begins a copy into `table`, as SQL names
    /// it, of `columns`, in order, all of the table's if none: the copy
    /// the transaction's own under `set_autocommit`.
    ///
    /// # Errors
    ///
    /// A connection failure: `copy` not granted, or the server gone. The
    /// table not found, a column not of it, comes as `finish`'s failure.
    pub fn copy_in(&mut self, table: &str, columns: &[&str]) -> Result<Copying<'_, S>> {
        let columns = columns.iter().map(|&c| c.to_owned()).collect();
        let ends = self.autocommit;
        self.copy_begin(table, columns, ends)
    }

    /// Begins a copy into `table`'s `columns`, the transaction ended with
    /// it if `ends`.
    pub(crate) fn copy_begin(&mut self, table: &str, columns: Vec<String>, ends: bool) -> Result<Copying<'_, S>> {
        self.needs(false, COPY)?;
        self.send(&Request::CopyIn { table: table.to_owned(), columns, ends })?;
        Ok(Copying { connection: self, waiting: Vec::new(), bytes: 0, ended: false })
    }

    /// Copies `rows` into `table`'s `columns` (`copy_in`): the rows
    /// inserted.
    ///
    /// # Errors
    ///
    /// As `copy_in`'s and `Copying::finish`'s.
    pub fn copy_rows(&mut self, table: &str, columns: &[&str], rows: impl IntoIterator<Item = Row>) -> Result<u64> {
        let mut copying = self.copy_in(table, columns)?;
        for row in rows {
            copying.send(row)?;
        }
        copying.finish()
    }
}

impl<S: Read + Write> Copying<'_, S> {
    /// Sends `row`, with those before it once they are `CHUNK_BYTES`.
    ///
    /// # Errors
    ///
    /// A connection failure. A row the server refuses comes as `finish`'s
    /// failure.
    pub fn send(&mut self, row: Row) -> Result<()> {
        self.bytes = self.bytes.saturating_add(row_size(&row));
        self.waiting.push(row);
        if self.bytes >= CHUNK_BYTES { self.flush() } else { Ok(()) }
    }

    /// \[DESIGN\] RULES.md D1148: sends `lanes`, rows of the copy by
    /// column, in a CopyColumns, and gives its buffers back as they were,
    /// to be filled again.
    ///
    /// # Errors
    ///
    /// A connection failure: `columns` not granted, or the server gone. A
    /// row the server refuses comes as `finish`'s failure.
    pub(crate) fn send_columns(&mut self, lanes: &mut Lanes) -> Result<()> {
        self.connection.needs(false, COLUMNS)?;
        let request = Request::CopyColumns(mem::take(lanes));
        let sent = self.connection.send(&request);
        if let Request::CopyColumns(back) = request {
            *lanes = back;
        }
        sent
    }

    /// Sends the rows waiting.
    fn flush(&mut self) -> Result<()> {
        if self.waiting.is_empty() {
            return Ok(());
        }
        self.bytes = 0;
        let rows = mem::take(&mut self.waiting);
        self.connection.send(&Request::CopyData(rows))
    }

    /// Ends the copy: the rows inserted.
    ///
    /// # Errors
    ///
    /// The copy's failure, every row of it undone: the table or a column
    /// not found, a row of too many or too few values, a value its column
    /// refuses, its detail naming the row; a connection failure.
    pub fn finish(mut self) -> Result<u64> {
        self.flush()?;
        self.end(false)
    }

    /// Gives the copy up: every row of it undone.
    ///
    /// # Errors
    ///
    /// A connection failure.
    pub fn abandon(mut self) -> Result<()> {
        self.end(true).map(drop)
    }

    /// Sends CopyEnd, abandoned if `abandon`: the rows inserted.
    fn end(&mut self, abandon: bool) -> Result<u64> {
        self.ended = true;
        let completed = self.connection.completed(&Request::CopyEnd { abandon })?;
        Ok(completed.rows.unwrap_or(0))
    }
}

impl<S: Read + Write> Drop for Copying<'_, S> {
    fn drop(&mut self) {
        if !self.ended && !self.connection.is_lost() {
            // Abandoned so the connection takes requests again; a failure
            // has lost it, which says so at its next request.
            let _ = self.end(true);
        }
    }
}
