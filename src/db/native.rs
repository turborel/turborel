//! \[DESIGN\] Throwaway experiment, branch `storagebound`, never merged: a
//! session's transaction driven straight through the storage and
//! transaction API, with no parser, plan or executor between, so the
//! bench can tell what a TPROC-C transaction costs the storage engine.
//!
//! Kept: the transaction and its snapshots, renewed as each statement of
//! a procedure renews them; the table locks a statement takes; MVCC
//! reads through the index probes; each write's row lock, conflict
//! wait and recheck, undo, log record and index upkeep
//! (`State::rewrite_row`, `State::insert_routed`); the commit and its
//! sync (`Session::commit`). Skipped: everything of SQL.

#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction, missing_docs)]

use std::sync::Arc;

use super::{Session, State};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::index::Probe;
use crate::storage::{Row, RowId};
use crate::txn::{Again, Txn};
use crate::value::{DataType, Value};

/// A transaction's state and handle, as a native procedure sees them.
pub struct NativeTx<'a> {
    st: &'a Arc<State>,
    tx: &'a Txn,
}

impl Session {
    /// Runs `f` in the session's transaction, begun if none is, as one
    /// statement of it; the transaction is left open for `commit`.
    ///
    /// # Errors
    ///
    /// `f`'s, or of beginning the transaction.
    pub fn native<R>(&self, f: impl FnOnce(&NativeTx<'_>) -> Result<R>) -> Result<R> {
        let call = self.enter()?;
        let st = call.state();
        let tx = self.begun(st)?;
        let ran = f(&NativeTx { st, tx: &tx });
        self.replied(&tx, ran)
    }
}

impl NativeTx<'_> {
    /// The next statement begins: reading at a snapshot for each statement,
    /// a new one taken, as a procedure's statement takes it.
    pub fn statement(&self) -> Result<()> {
        self.tx.renew_snapshot(|| true)
    }

    /// The shared lock a statement reading `table` takes.
    pub fn read_lock(&self, table: &QualifiedName) -> Result<()> {
        self.st.read_lock(self.tx, table)
    }

    /// The update lock a statement reading `table` to write it takes.
    pub fn update_lock(&self, table: &QualifiedName) -> Result<()> {
        self.st.update_lock(self.tx, table)
    }

    /// The name of the index of `table` on exactly `columns`.
    pub fn index_on(&self, table: &QualifiedName, columns: &[usize]) -> Result<String> {
        self.st
            .catalog
            .indexes(table)
            .into_iter()
            .find(|ix| ix.columns == columns)
            .map(|ix| ix.name.clone())
            .ok_or_else(|| SqlError::new(Code::Api, format!("no index of {table} on {columns:?}")))
    }

    /// The type of column `column` of `table`.
    pub fn column_type(&self, table: &QualifiedName, column: usize) -> Result<DataType> {
        self.st
            .catalog
            .table(table)
            .and_then(|t| t.columns.get(column))
            .map(|c| c.data_type.clone())
            .ok_or_else(|| SqlError::new(Code::Api, format!("no column {column} of {table}")))
    }

    /// Each row the snapshot sees under `probe` of `index`, in key order or
    /// `reverse`d, given to `each` until it returns false.
    pub fn scan(
        &self,
        (table, index): (&QualifiedName, &str),
        probe: &Probe,
        reverse: bool,
        each: &mut dyn FnMut(RowId, Row) -> Result<bool>,
    ) -> Result<()> {
        crate::exec::native::probe_rows(self.st, self.tx.id, (table, index), (probe, reverse), each)
    }

    /// The one row the snapshot sees whose key in `index`, unique, is `key`.
    pub fn get(&self, (table, index): (&QualifiedName, &str), key: Vec<Value>) -> Result<Option<(RowId, Row)>> {
        let mut found = None;
        let probe = Probe::Prefix { equal: key, then: None };
        self.scan((table, index), &probe, false, &mut |id, row| {
            found = Some((id, row));
            Ok(false)
        })?;
        Ok(found)
    }

    /// Replaces row `id` of `table` with `row`; of a row another
    /// transaction changed since the snapshot, waits for it and writes what
    /// `recheck` makes of the version it committed instead, none leaving
    /// it alone. The row replaced, none if left alone.
    pub fn update(
        &self,
        (table, id): (&QualifiedName, RowId),
        row: &Row,
        recheck: &mut dyn FnMut(&Row) -> Result<Option<Row>>,
    ) -> Result<Option<Row>> {
        let mut again = |newest: &Row| Ok(recheck(newest)?.map_or(Again::Skip, Again::Update));
        self.st.rewrite_row(self.tx, (table, id), Some(row), &mut again)
    }

    /// Deletes row `id` of `table`, as `update` replaces it: the row
    /// deleted, none if another transaction deleted it first.
    pub fn delete(&self, (table, id): (&QualifiedName, RowId)) -> Result<Option<Row>> {
        let mut again = |_: &Row| Ok(Again::Delete);
        self.st.rewrite_row(self.tx, (table, id), None, &mut again)
    }

    /// Inserts `row` into `table`, with its index entries: its id.
    pub fn insert(&self, table: &QualifiedName, row: &Row) -> Result<RowId> {
        self.st.insert_routed(self.tx, table, row).map(|(_, id)| id)
    }
}
