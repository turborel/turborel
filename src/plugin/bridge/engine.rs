//! \[EXTENSION\] A plug-in's storage engine (RULES.md D224, D225), a
//! `StorageEngine` whose tables keep their rows in the program's care,
//! outside its process, so a process stopped loses none.
//!
//! Transactions: the coordinator's table locks isolate them
//! (`Concurrency::Coordinator`), and each table keeps its own undo
//! (`Undo::Native`) by keeping a transaction's changes in the database,
//! in an overlay of the rows it wrote, with a journal its savepoints
//! return to. The program sees no change until the transaction commits:
//! at `validate` the overlay is applied, BATCH rows an APPLY, each call
//! all or none, the rows it changes read first (GET, BATCH ids a call),
//! so a refusal or a failed call puts every one back and the transaction
//! is rolled back. A process of the extension that stopped while the
//! transaction wrote fails it, as a serialization failure (SQLSTATE
//! 40001) to run again. Once validated, a rollback (another participant's
//! refusal) applies the rows read before; if that fails the table is
//! damaged (38T10).
//!
//! Reads: a transaction reads the program's rows, a page of BATCH rows a
//! SCAN, or one GET a row read by id, merged with its own overlay.
//!
//! Durability: its changes are logged (`Persistence::Logged`). A
//! checkpoint with no transaction writing the table is told the program
//! (CHECKPOINT), which makes what it applied durable and keeps the
//! position with its rows; OPEN gives it back, and recovery replays the
//! log from there, each change one APPLY of a row, idempotent. A table
//! damaged takes no checkpoint, so the next open recovers it.

mod scan;
mod table;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::catalog::TableDesc;
use crate::error::Result;
use crate::log::Lsn;
use crate::plugin::errors::broken;
use crate::plugin::payload::engine::{opened, put_changes};
use crate::plugin::payload::statistics::{self, TableAnswer};
use crate::plugin::payload::{ENGINE_APPLY, ENGINE_CHECKPOINT, ENGINE_CREATE, ENGINE_OPEN, ENGINE_STATISTICS, put_strings};
use crate::plugin::values::stored;
use crate::storage::{Row, RowId, StorageEngine, TableStorage};
use crate::sync::lock_builtin;
use crate::value::DataType;

use super::{Link, answer, leaked};

pub use table::PluginTable;

/// The rows a SCAN asks when the manifest says no BATCH.
pub const DEFAULT_BATCH: u32 = 1024;

/// A plug-in's storage engine.
#[derive(Debug)]
pub struct PluginEngine {
    link: Link,
    name: &'static str,
    batch: u32,
    /// Its tables, held weakly, for its checkpoints.
    tables: Mutex<Vec<Weak<Shared>>>,
}

/// What a table and its scans share.
#[derive(Debug)]
pub(super) struct Shared {
    link: Link,
    locator: Vec<u8>,
    columns: Vec<DataType>,
    batch: u32,
    /// The next id to give.
    next: AtomicU64,
    /// The rows committed.
    rows: Mutex<u64>,
    /// The position the program is durable through.
    durable: AtomicU64,
    /// The position a checkpoint captured, until it is finished.
    captured: Mutex<Option<u64>>,
    /// The transactions writing it, each its overlay.
    open: Mutex<std::collections::HashMap<crate::txn::TxId, table::Overlay>>,
    damaged: AtomicBool,
    /// What the program knows of its rows (RULES.md D278).
    known: super::known::Known<TableAnswer>,
}

impl PluginEngine {
    /// The engine `link` names, scanning `batch` rows a page.
    pub fn new(link: Link, batch: Option<u32>) -> PluginEngine {
        let name = leaked(&link.object);
        PluginEngine { link, name, batch: batch.unwrap_or(DEFAULT_BATCH).max(1), tables: Mutex::default() }
    }

    /// The table of `desc` whose program's state `opened` says, kept for
    /// checkpoints.
    fn table(&self, desc: &TableDesc, locator: Vec<u8>, o: crate::plugin::payload::engine::Opened) -> Box<dyn TableStorage> {
        let shared = Arc::new(Shared {
            link: self.link.clone(),
            locator,
            columns: desc.columns.iter().map(|c| c.data_type.clone()).collect(),
            batch: self.batch,
            next: AtomicU64::new(o.next.max(1)),
            rows: Mutex::new(o.rows),
            durable: AtomicU64::new(o.durable),
            captured: Mutex::new(None),
            open: Mutex::default(),
            damaged: AtomicBool::new(false),
            known: super::known::Known::default(),
        });
        let mut tables = lock_builtin(&self.tables);
        tables.retain(|t| t.strong_count() > 0);
        tables.push(Arc::downgrade(&shared));
        Box::new(PluginTable::of(shared))
    }

    /// Its tables still held.
    fn live(&self) -> Vec<Arc<Shared>> {
        lock_builtin(&self.tables).iter().filter_map(Weak::upgrade).collect()
    }
}

impl StorageEngine for PluginEngine {
    fn name(&self) -> &'static str {
        self.name
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let mut payload = Vec::new();
        crate::bytes::Put::put_str(&mut payload, &desc.name.to_string());
        put_strings(&mut payload, &desc.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        let locator = answer(&self.link.plain(ENGINE_CREATE, payload)?, |r| r.bytes().map(<[u8]>::to_vec))?;
        Ok(self.table(desc, locator, crate::plugin::payload::engine::Opened { found: true, next: 1, rows: 0, durable: 0 }))
    }
    fn open_table(&self, desc: &TableDesc, locator: &[u8]) -> Result<Option<Box<dyn TableStorage>>> {
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, locator);
        let o = answer(&self.link.plain(ENGINE_OPEN, payload)?, opened)?;
        Ok(o.found.then(|| self.table(desc, locator.to_vec(), o)))
    }
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        for t in self.live() {
            let idle = lock_builtin(&t.open).values().all(table::Overlay::is_empty);
            *lock_builtin(&t.captured) = idle.then_some(at.0);
        }
        Ok(())
    }
    fn checkpoint_finish(&self) -> Result<()> {
        for t in self.live() {
            let Some(at) = lock_builtin(&t.captured).take() else { continue };
            if t.damaged.load(Ordering::Acquire) {
                return Err(broken("table", "a damaged table takes no checkpoint"));
            }
            let mut payload = Vec::new();
            crate::bytes::Put::put_bytes(&mut payload, &t.locator);
            crate::bytes::Put::put_u64(&mut payload, at);
            t.link.plain(ENGINE_CHECKPOINT, payload)?;
            t.durable.fetch_max(at, Ordering::AcqRel);
        }
        Ok(())
    }
}

impl Shared {
    /// `row`, as the program gave it, stored in the table's columns.
    fn stored(&self, row: Row) -> Result<Row> {
        if row.len() != self.columns.len() {
            return Err(crate::plugin::errors::violation(&format!("a row of {} values, of {} columns", row.len(), self.columns.len())));
        }
        row.into_iter().zip(&self.columns).map(|(v, t)| stored(v, t)).collect()
    }

    /// `changes` applied, BATCH rows a call, each call all or none: what
    /// each row held before. A failure leaves the calls before it made,
    /// which the caller puts back.
    fn apply(&self, changes: &[(RowId, Option<Row>)]) -> Result<Vec<Option<Row>>> {
        let mut before = Vec::with_capacity(changes.len());
        for chunk in changes.chunks(self.chunk()) {
            let mut payload = Vec::new();
            put_changes(&mut payload, &self.locator, chunk)?;
            let got = answer(&self.link.plain(ENGINE_APPLY, payload)?, crate::plugin::payload::engine::maybes)?;
            if let Some(top) = chunk.iter().map(|(id, _)| *id).max() {
                self.next.fetch_max(top.saturating_add(1), Ordering::AcqRel);
            }
            before.extend(super::counted(got, chunk.len())?);
        }
        Ok(before)
    }

    /// The rows of a call: BATCH.
    pub(super) fn chunk(&self) -> usize {
        usize::try_from(self.batch).unwrap_or(usize::MAX).max(1)
    }

    /// What the program knows of the table's rows, of as many columns as
    /// it has, and after how many rows written it is stale; none if it
    /// knows nothing (STATISTICS, RULES.md D278).
    ///
    /// # Errors
    ///
    /// The call's, or SQLSTATE 38T03 of an answer of other columns.
    fn statistics(&self) -> Result<Option<(TableAnswer, u64)>> {
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.locator);
        let Some(got) = answer(&self.link.plain(ENGINE_STATISTICS, payload)?, statistics::table)? else { return Ok(None) };
        if got.columns.len() != self.columns.len() && !got.columns.is_empty() {
            return Err(crate::plugin::errors::violation(&format!(
                "statistics of {} columns, of {}",
                got.columns.len(),
                self.columns.len()
            )));
        }
        let refresh = got.refresh;
        Ok(Some((got, refresh)))
    }

    /// Counts `before`, rows held, replaced by `changes`.
    fn count(&self, before: &[Option<Row>], changes: &[(RowId, Option<Row>)]) {
        self.known.written(u64::try_from(changes.len()).unwrap_or(u64::MAX));
        let mut rows = lock_builtin(&self.rows);
        for (b, (_, c)) in before.iter().zip(changes) {
            *rows = match (b.is_some(), c.is_some()) {
                (false, true) => rows.saturating_add(1),
                (true, false) => rows.saturating_sub(1),
                (true, true) | (false, false) => *rows,
            };
        }
    }
}
