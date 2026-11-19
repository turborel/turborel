//! \[EXTENSION\] A plug-in engine's table (RULES.md D224): its rows the
//! program's, each transaction's changes an overlay kept here until it
//! commits.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::Shared;
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::plugin::errors::{broken, restarted};
use crate::plugin::payload::ENGINE_GET;
use crate::plugin::payload::engine::{maybes, put_ids};
use crate::stats::Statistics;
use crate::storage::{Row, RowId, Rows, TableStorage};
use crate::sync::lock_builtin;
use crate::txn::{Concurrency, Persistence, Timestamp, TxCapabilities, TxId, Undo};

use super::super::{answer, counted};

/// A transaction's changes to a table, not yet the program's.
#[derive(Debug, Default)]
pub(in crate::plugin::bridge) struct Overlay {
    /// Each row it wrote, as it is now: none if deleted.
    pub(super) rows: BTreeMap<RowId, Option<Row>>,
    /// Each write, what its row was in the overlay before, oldest first:
    /// none if the overlay did not hold it.
    journal: Vec<(RowId, Held)>,
    /// The pool's processes stopped after a failure, as the transaction
    /// began here.
    stopped: u64,
    /// Once validated, what each row changed held before.
    applied: Option<Vec<(RowId, Option<Row>)>>,
}

/// A row as an overlay holds it: not at all, or written, none if deleted.
#[derive(Debug, Clone)]
pub(in crate::plugin::bridge) enum Held {
    /// The overlay holds it not.
    Not,
    /// The overlay holds it as this.
    Written(Option<Row>),
}

impl Overlay {
    /// Whether it holds no change.
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Writes `row` to `id`, journalled.
    fn write(&mut self, id: RowId, row: Option<Row>) {
        let was = self.rows.insert(id, row).map_or(Held::Not, Held::Written);
        self.journal.push((id, was));
    }
}

/// A plug-in engine's table.
#[derive(Debug)]
pub struct PluginTable(pub(super) Arc<Shared>);

impl PluginTable {
    /// The table of `shared`.
    pub(super) fn of(shared: Arc<Shared>) -> PluginTable {
        PluginTable(shared)
    }

    /// The pool's failures so far.
    fn stopped(&self) -> u64 {
        self.0.link.pool.info().stopped
    }

    /// Writes `row` to `id` in `tx`'s overlay.
    fn write(&self, tx: TxId, id: RowId, row: Option<Row>) {
        let stopped = self.stopped();
        let mut open = lock_builtin(&self.0.open);
        open.entry(tx).or_insert_with(|| Overlay { stopped, ..Overlay::default() }).write(id, row);
    }

    /// `tx`'s own version of `id`, if it wrote it.
    fn own(&self, tx: TxId, id: RowId) -> Held {
        lock_builtin(&self.0.open).get(&tx).and_then(|o| o.rows.get(&id).cloned()).map_or(Held::Not, Held::Written)
    }

    /// The program's rows `ids`, committed.
    pub(super) fn fetched(&self, ids: &[RowId]) -> Result<Vec<Option<Row>>> {
        let mut all = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(self.0.chunk()) {
            let mut payload = Vec::new();
            put_ids(&mut payload, &self.0.locator, chunk);
            let got = counted(answer(&self.0.link.plain(ENGINE_GET, payload)?, maybes)?, chunk.len())?;
            for row in got {
                all.push(row.map(|r| self.0.stored(r)).transpose()?);
            }
        }
        Ok(all)
    }

    /// That `id` is live to `tx`.
    fn live(&self, tx: TxId, id: RowId) -> Result<()> {
        if self.get(tx, id)?.is_some() {
            return Ok(());
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] no live row has id {id}")))
    }

    /// `tx`'s overlay applied, all or none, its rows read first so a failed
    /// call is put back; the table damaged if it cannot be.
    fn flush(&self, tx: TxId, changes: &[(RowId, Option<Row>)]) -> Result<()> {
        let ids: Vec<RowId> = changes.iter().map(|(id, _)| *id).collect();
        let before = self.fetched(&ids)?;
        if let Err(e) = self.0.apply(changes) {
            let put_back: Vec<(RowId, Option<Row>)> = ids.into_iter().zip(before).collect();
            if self.0.apply(&put_back).is_err() {
                self.0.damaged.store(true, Ordering::Release);
                return Err(broken("table", &e.msg));
            }
            return Err(e);
        }
        self.0.count(&before, changes);
        if let Some(o) = lock_builtin(&self.0.open).get_mut(&tx) {
            o.applied = Some(ids.into_iter().zip(before).collect());
        }
        Ok(())
    }
}

impl TableStorage for PluginTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        let id = self.0.next.fetch_add(1, Ordering::AcqRel);
        self.write(tx, id, Some(row));
        Ok(id)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.live(tx, id)?;
        self.write(tx, id, Some(row));
        Ok(())
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.live(tx, id)?;
        self.write(tx, id, None);
        Ok(())
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.write(tx, id, Some(row));
        Ok(())
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        if let Held::Written(own) = self.own(tx, id) {
            return Ok(own);
        }
        Ok(self.fetched(&[id])?.pop().flatten())
    }
    fn scan(self: Arc<Self>, tx: TxId, _columns: &[usize]) -> Rows {
        let overlay = lock_builtin(&self.0.open).get(&tx).map(|o| o.rows.clone()).unwrap_or_default();
        Box::new(super::scan::Scan::new(Arc::clone(&self.0), overlay))
    }
    fn row_count(&self) -> usize {
        usize::try_from(*lock_builtin(&self.0.rows)).unwrap_or(usize::MAX)
    }
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        let known: Arc<dyn Statistics> = self.0.known.get(|| self.0.statistics())?;
        Some(known)
    }
    fn analyze(&self) -> Result<()> {
        self.0.known.refresh(|| self.0.statistics());
        Ok(())
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Coordinator, persistence: Persistence::Logged }
    }
    fn begin(&self, tx: TxId, _snapshot: Timestamp) -> Result<()> {
        let stopped = self.stopped();
        lock_builtin(&self.0.open).entry(tx).or_insert_with(|| Overlay { stopped, ..Overlay::default() });
        Ok(())
    }
    fn savepoint(&self, tx: TxId) -> u64 {
        let open = lock_builtin(&self.0.open);
        open.get(&tx).map_or(0, |o| u64::try_from(o.journal.len()).unwrap_or(u64::MAX))
    }
    fn rollback_to(&self, tx: TxId, savepoint: u64) -> Result<()> {
        let mut open = lock_builtin(&self.0.open);
        let Some(o) = open.get_mut(&tx) else { return Ok(()) };
        let keep = usize::try_from(savepoint).unwrap_or(usize::MAX);
        while o.journal.len() > keep {
            let Some((id, was)) = o.journal.pop() else { break };
            match was {
                Held::Written(row) => drop(o.rows.insert(id, row)),
                Held::Not => drop(o.rows.remove(&id)),
            }
        }
        Ok(())
    }
    fn validate(&self, tx: TxId, _at: Timestamp) -> Result<()> {
        let (changes, stopped) = {
            let open = lock_builtin(&self.0.open);
            let Some(o) = open.get(&tx) else { return Ok(()) };
            (o.rows.iter().map(|(id, row)| (*id, row.clone())).collect::<Vec<_>>(), o.stopped)
        };
        if changes.is_empty() {
            return Ok(());
        }
        if self.stopped() != stopped {
            return Err(restarted(self.0.link.pool.extension(), "a table"));
        }
        self.flush(tx, &changes)
    }
    fn commit(&self, tx: TxId, _at: Timestamp) {
        lock_builtin(&self.0.open).remove(&tx);
    }
    fn rollback(&self, tx: TxId) -> Result<()> {
        let Some(o) = lock_builtin(&self.0.open).remove(&tx) else { return Ok(()) };
        let Some(before) = o.applied else { return Ok(()) };
        let changes: Vec<(RowId, Option<Row>)> = o.rows.into_iter().collect();
        match self.0.apply(&before) {
            Ok(_) => {
                let after: Vec<Option<Row>> = changes.into_iter().map(|(_, r)| r).collect();
                self.0.count(&after, &before);
                Ok(())
            }
            Err(e) => {
                self.0.damaged.store(true, Ordering::Release);
                Err(broken("table", &e.msg))
            }
        }
    }
    fn durable_through(&self) -> Lsn {
        Lsn(self.0.durable.load(Ordering::Acquire))
    }
    fn locator(&self) -> Vec<u8> {
        self.0.locator.clone()
    }
    fn discard(&self) {
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.0.locator);
        // A table dropped whose program fails to drop it leaves its rows in
        // the program's care, read by nothing.
        drop(self.0.link.plain(crate::plugin::payload::ENGINE_DROP, payload));
    }
    fn apply(&self, id: RowId, row: Option<Row>) -> Result<()> {
        let changes = [(id, row)];
        let before = self.0.apply(&changes)?;
        self.0.count(&before, &changes);
        Ok(())
    }
}
