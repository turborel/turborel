//! A transaction's bookkeeping in a table as its reads find it (RULES.md
//! D29): who it is as a reader, and what it read, if its reads are
//! checked. A scan of rows by id keeps the bookkeeping it found in its
//! `Near`, so a read after the first finds it without the table's map of
//! open transactions, which every reader of the table shares.

use std::any::Any;
use std::sync::{Arc, Mutex};

use super::{Open, Read, Reader};
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
        let snapshot = lock_builtin(&open).snapshot;
        Ok(Reader::At { tx, snapshot })
    }

    pub(super) fn open_of(&self, tx: TxId) -> Result<Arc<Mutex<Open>>> {
        let open = self.versions.open.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        open.get(&tx)
            .map(Arc::clone)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] transaction {tx} has not begun in this table")))
    }

    /// Records what `tx` read, as `read` gives it, if its reads are
    /// checked: given only then, so a read not checked copies nothing.
    pub(in crate::engine::rows) fn note_read(&self, tx: TxId, read: impl FnOnce() -> Read) -> Result<()> {
        if tx == NO_TRANSACTION {
            return Ok(());
        }
        let open = self.open_of(tx)?;
        self.record(&mut lock_builtin(&open), read);
        Ok(())
    }

    /// Who `tx` is as a reader of row `id`, the read recorded as
    /// `note_read` records it, both under one lock of its bookkeeping. That
    /// is found from what `near` keeps of the scan's last read while `tx`
    /// is open here, and kept there for the next.
    pub(in crate::engine::rows) fn read_row(&self, tx: TxId, id: RowId, near: &mut Near) -> Result<Reader> {
        if tx == NO_TRANSACTION {
            return Ok(Reader::Latest);
        }
        let table = std::ptr::from_ref(self).addr();
        let kept = near.open.as_ref().filter(|&&(by, of, _)| by == tx && of == table);
        if let Some(open) = kept.and_then(|(_, _, open)| open.downcast_ref::<Mutex<Open>>()) {
            let mut open = lock_builtin(open);
            if !open.ended {
                return Ok(self.read_by(&mut open, tx, id));
            }
        }
        let open = self.open_of(tx)?;
        let reader = self.read_by(&mut lock_builtin(&open), tx, id);
        let held: Arc<dyn Any + Send + Sync> = open;
        near.open = Some((tx, table, held));
        Ok(reader)
    }

    fn read_by(&self, open: &mut Open, tx: TxId, id: RowId) -> Reader {
        self.record(open, || Read::Row(id));
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
