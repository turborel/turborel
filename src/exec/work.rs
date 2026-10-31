//! \[DESIGN\] An operator's work area (RULES.md D38): what it may hold in
//! memory, the setting MEMORY.WORK_BYTES's bytes, and once it holds more, a spill
//! area of as many bytes, reserved of the memory budget, made when first
//! needed (`spill.rs`). And the keys an operator counts, kept in memory
//! until they outgrow it, then in a table of its area. Split from exec.rs
//! (RULES.md R-SIZE-2).
//!
//! The runs of an operator's area, of the statement that runs it, look for
//! a cancel of it as each record, or each batch of records, is read back
//! (RULES.md D44): the passes of a spilled operator read no table's rows,
//! which look so, yet may read many records.

use crate::value::types::TypeRegistry;
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::settings::{MEMORY_WORK_BYTES, SPILL_BATCH_BYTES, SPILL_PARTITION_DEPTH, SPILL_PARTITIONS, batch_bytes, work_bytes};
use crate::spill::{Area, Batches, Records, SpillArea, SpillRun, SpillTable, WorkAreas};
use crate::txn::{Stop, TxId, checked};

/// The bytes a key held in memory takes beyond its own: its map entry.
pub(super) const ENTRY_BYTES: usize = 48;

/// One operator's work area.
pub(super) struct Work {
    limit: usize,
    /// The bytes of rows a run gathers before it writes them.
    batch: usize,
    /// The most partitions an operator writes at once, and the most times
    /// it partitions one again.
    partitions: (usize, usize),
    areas: Arc<WorkAreas>,
    area: Option<Box<dyn SpillArea>>,
    /// The types beyond the standard the rows it spills may hold values of,
    /// read back by them (`spill::record_row`).
    types: TypeRegistry,
    /// What cancels the statement it works for, looked at as its runs are
    /// read back; none if it is not known.
    stop: Option<Arc<Stop>>,
}

impl Work {
    /// The work area of an operator of `db`.
    pub(super) fn new(db: &State) -> Work {
        let (limit, batch) = (work_bytes(db.settings.get(MEMORY_WORK_BYTES)), batch_bytes(db.settings.get(SPILL_BATCH_BYTES)));
        let partitions = (db.settings.count(SPILL_PARTITIONS), db.settings.count(SPILL_PARTITION_DEPTH));
        Work { limit, batch, partitions, areas: Arc::clone(&db.work), area: None, types: db.types.clone(), stop: None }
    }

    /// The work area of an operator of `db` run by `tx`'s statement: its
    /// runs read back only while the statement is not cancelled.
    pub(super) fn of(db: &State, tx: TxId) -> Work {
        Work { stop: db.tx.stop_of(tx), ..Work::new(db) }
    }

    /// A work area of the same settings, of its own: for an operator's
    /// part that holds rows apart from the rest.
    pub(super) fn another(&self) -> Work {
        let (limit, batch, partitions, stop) = (self.limit, self.batch, self.partitions, self.stop.clone());
        Work { limit, batch, partitions, areas: Arc::clone(&self.areas), area: None, types: self.types.clone(), stop }
    }

    /// Half of it, of its own, the other half left to it, an area of each
    /// half made of each when needed: for a part of an operator that holds
    /// rows beside it, so the two hold no more than one work area does.
    /// Made before it makes its area.
    pub(super) fn split(&mut self) -> Work {
        let half = self.limit.div_ceil(2);
        self.limit = self.limit.saturating_sub(half);
        Work { limit: half, ..self.another() }
    }

    /// The types beyond the standard the rows it spills may hold values of.
    pub(super) fn types(&self) -> &TypeRegistry {
        &self.types
    }

    /// The bytes it may hold in memory.
    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    /// How many runs it reads or writes at a time, each holding a batch
    /// being written or read and the rows of one: at least two.
    pub(super) fn width(&self) -> usize {
        self.limit.checked_div(self.batch.saturating_mul(2)).unwrap_or_default().max(2)
    }

    /// The most partitions an operator writes at once (SPILL.PARTITIONS).
    pub(super) fn partitions(&self) -> usize {
        self.partitions.0
    }

    /// The most times an operator partitions a partition again
    /// (SPILL.PARTITION_DEPTH).
    pub(super) fn depth(&self) -> usize {
        self.partitions.1
    }

    /// Its spill area, made the first time.
    fn area(&mut self) -> Result<&dyn SpillArea> {
        if self.area.is_none() {
            self.area = Some(self.areas.area(Area::new(self.limit, self.batch))?);
        }
        self.area.as_deref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a spill area was not made"))
    }

    /// A new table of its spill area.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the area cannot be reserved; the area's errors.
    pub(super) fn table(&mut self) -> Result<Box<dyn SpillTable>> {
        self.area()?.table()
    }

    /// A new run of its spill area.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the area cannot be reserved; the area's errors.
    pub(super) fn run(&mut self) -> Result<Box<dyn SpillRun>> {
        let run = self.area()?.run()?;
        Ok(match &self.stop {
            Some(stop) => Box::new(Stopped { run, stop: Arc::clone(stop) }),
            None => run,
        })
    }
}

/// A run whose records are read back only while the statement that wrote
/// them is not cancelled (`txn::checked`).
struct Stopped {
    run: Box<dyn SpillRun>,
    stop: Arc<Stop>,
}

impl SpillRun for Stopped {
    fn push(&mut self, record: &[u8]) -> Result<()> {
        self.run.push(record)
    }

    fn records(&mut self) -> Result<Records> {
        Ok(Box::new(checked(self.run.records()?, Some(Arc::clone(&self.stop)))))
    }

    fn into_records(self: Box<Self>) -> Result<Records> {
        let Stopped { run, stop } = *self;
        Ok(Box::new(checked(run.into_records()?, Some(stop))))
    }

    /// Looked for a cancel as each batch is read: a sort's merge reads
    /// its runs so.
    fn into_batches(self: Box<Self>) -> Result<Batches> {
        let Stopped { run, stop } = *self;
        Ok(Box::new(checked(run.into_batches()?, Some(stop))))
    }
}

/// Keys, each with a count: in memory until they hold more than the work
/// area's bytes, then all in a table of its spill area, a key's value its
/// count.
pub(super) struct Bag {
    work: Work,
    counts: HashMap<Vec<u8>, u64>,
    bytes: usize,
    table: Option<Box<dyn SpillTable>>,
}

impl Bag {
    /// An empty bag in `work`.
    pub(super) fn new(work: Work) -> Bag {
        Bag { work, counts: HashMap::new(), bytes: 0, table: None }
    }

    /// The count of `key`: 0 if it is not in the bag.
    pub(super) fn count(&self, key: &[u8]) -> Result<u64> {
        match &self.table {
            Some(table) => Ok(table.get(key)?.first().map_or(0, |v| count_of(v))),
            None => Ok(self.counts.get(key).copied().unwrap_or(0)),
        }
    }

    /// Adds `more` to the count of `key`.
    pub(super) fn add_count(&mut self, key: &[u8], more: u64) -> Result<()> {
        let n = self.count(key)?;
        self.set(key, (n, n.saturating_add(more)))
    }

    /// Adds `key` if it is not in the bag; whether it was not.
    pub(super) fn add_new(&mut self, key: &[u8]) -> Result<bool> {
        if self.count(key)? > 0 {
            return Ok(false);
        }
        self.set(key, (0, 1))?;
        Ok(true)
    }

    /// Takes one from the count of `key`, if it is above 0; whether it was.
    pub(super) fn take(&mut self, key: &[u8]) -> Result<bool> {
        let n = self.count(key)?;
        if n == 0 {
            return Ok(false);
        }
        self.set(key, (n, n.saturating_sub(1)))?;
        Ok(true)
    }

    /// Changes the count of `key` from `was` to `now`.
    fn set(&mut self, key: &[u8], (was, now): (u64, u64)) -> Result<()> {
        if let Some(table) = &mut self.table {
            if was > 0 {
                table.remove(key, &was.to_le_bytes())?;
            }
            return if now > 0 { table.insert(key, &now.to_le_bytes()) } else { Ok(()) };
        }
        if was == 0 {
            self.bytes = self.bytes.saturating_add(key.len()).saturating_add(ENTRY_BYTES);
        }
        self.counts.insert(key.to_vec(), now);
        if self.bytes > self.work.limit() { self.spill() } else { Ok(()) }
    }

    /// Moves its keys to a table of the spill area.
    fn spill(&mut self) -> Result<()> {
        let mut table = self.work.table()?;
        for (key, n) in self.counts.drain() {
            if n > 0 {
                table.insert(&key, &n.to_le_bytes())?;
            }
        }
        self.table = Some(table);
        Ok(())
    }
}

/// The count a table's value holds.
fn count_of(value: &[u8]) -> u64 {
    <[u8; 8]>::try_from(value).map_or(0, u64::from_le_bytes)
}
