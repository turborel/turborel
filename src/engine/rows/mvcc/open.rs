//! A transaction's bookkeeping in a table as its reads find it (RULES.md
//! D29): who it is as a reader, and what it read, if its reads are
//! checked. A scan of rows by id keeps the bookkeeping it found in its
//! `Near`, and the thread keeps what it last found (`found.rs`, D871), so
//! a read or a write after the first finds it without the table's map of
//! open transactions, which every reader of the table shares.

use std::any::Any;
use std::sync::Arc;

use super::opens::Opened;
use super::{Open, Read, Reader, found};
use crate::engine::rows::RowTable;
use crate::error::{Code, Result, SqlError};
use crate::storage::{Near, RowId};
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, TxId};

impl RowTable {
    /// Who `tx` is as a reader.
    pub(in crate::engine::rows) fn reader(&self, tx: TxId) -> Result<Reader> {
        if tx == NO_TRANSACTION {
            return Ok(Reader::Latest);
        }
        let open = self.open_of(tx)?;
        let snapshot = open.unchecked().unwrap_or_else(|| lock_builtin(&open).snapshot);
        Ok(Reader::At { tx, snapshot })
    }

    /// The bookkeeping of `tx` here, open. What this thread last found of
    /// it is given with no lock taken, else it is looked up in the table's
    /// map and kept for the thread's next call (RULES.md D871).
    pub(super) fn open_of(&self, tx: TxId) -> Result<Arc<Opened>> {
        let table = self.versions.open.token();
        if let Some(kept) = found::kept(tx, table) {
            return Ok(kept);
        }
        let begun = || SqlError::new(Code::Api, format!("[DESIGN] transaction {tx} has not begun in this table"));
        let opened = self.versions.open.get(tx).ok_or_else(begun)?;
        found::keep(tx, table, &opened);
        Ok(opened)
    }

    /// Records what `tx` read, as `read` gives it, if its reads are
    /// checked: given only then, so a read not checked copies nothing.
    pub(in crate::engine::rows) fn note_read(&self, tx: TxId, read: impl FnOnce() -> Read) -> Result<()> {
        if tx == NO_TRANSACTION {
            return Ok(());
        }
        let open = self.open_of(tx)?;
        // Not checked, nothing is recorded, and no lock taken to find so.
        if open.unchecked().is_none() {
            self.record(&mut lock_builtin(&open), read);
        }
        Ok(())
    }

    /// Who `tx` is as a reader of row `id`, the read recorded as
    /// `note_read` records it, both under one lock of its bookkeeping. That
    /// is found from what `near` keeps of the scan's last read while `tx`
    /// is open here, and kept there for the next. A transaction whose
    /// reads are not checked has nothing recorded, and its snapshot is
    /// read with no lock taken (RULES.md D954).
    pub(in crate::engine::rows) fn read_row(&self, tx: TxId, (id, columns): (RowId, u64), near: &mut Near) -> Result<Reader> {
        if tx == NO_TRANSACTION {
            return Ok(Reader::Latest);
        }
        let table = std::ptr::from_ref(self).addr();
        let kept = near.open.as_ref().filter(|&&(by, of, _)| by == tx && of == table);
        if let Some(opened) = kept.and_then(|(_, _, open)| open.downcast_ref::<Opened>()).filter(|opened| !opened.ended()) {
            return Ok(self.read_through(opened, tx, (id, columns)));
        }
        let open = self.open_of(tx)?;
        let reader = self.read_through(&open, tx, (id, columns));
        let held: Arc<dyn Any + Send + Sync> = open;
        near.open = Some((tx, table, held));
        Ok(reader)
    }

    /// `read_by`, under the lock of `opened` only if its reads are checked.
    fn read_through(&self, opened: &Opened, tx: TxId, read: (RowId, u64)) -> Reader {
        match opened.unchecked() {
            Some(snapshot) => Reader::At { tx, snapshot },
            None => self.read_by(&mut lock_builtin(opened), tx, read),
        }
    }

    fn read_by(&self, open: &mut Open, tx: TxId, (id, columns): (RowId, u64)) -> Reader {
        self.record(open, || Read::Row(id, columns));
        Reader::At { tx, snapshot: open.snapshot }
    }

    fn record(&self, open: &mut Open, read: impl FnOnce() -> Read) {
        if !open.checked {
            return;
        }
        let at = open.snapshot;
        open.reads_since.get_or_insert(at);
        open.reads.add(read(), self.versions.tracked());
    }
}
