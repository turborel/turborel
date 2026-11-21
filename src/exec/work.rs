//! \[DESIGN\] An operator's work area (RULES.md D38): what it may hold in
//! memory, its grant of the memory budget of up to MEMORY.WORK_BYTES
//! (`spill/grant.rs`, RULES.md D298), and once it holds more, a spill area
//! of its bytes up to `WORK_FLOOR`, reserved of the budget, made when first
//! needed (`spill.rs`). And the keys an operator counts, kept in memory until
//! they outgrow it, then in a table of its area.
//!
//! The runs of an operator's area, of the statement that runs it, look for
//! a cancel of it as each record, or each batch of records, is read back
//! (RULES.md D44): the passes of a spilled operator read no table's rows,
//! which look so, yet may read many records.

use crate::value::types::TypeRegistry;
use std::collections::HashMap;
use std::cell::Cell;
use std::sync::Arc;

use crate::db::State;
use crate::exec::Seed;
use crate::error::{Code, Result, SqlError};
use crate::settings::{MEMORY_WORK_BYTES, SPILL_BATCH_BYTES, SPILL_PARTITION_DEPTH, SPILL_PARTITIONS, batch_bytes, work_bytes};
use crate::spill::{Area, Batches, Grant, Records, SpillArea, SpillRun, SpillTable, SpilledLobs, WORK_FLOOR, WorkAreas};
use crate::txn::{Stop, TxId, checked};

/// The bytes a key held in memory takes beyond its own: its map entry.
pub(super) const ENTRY_BYTES: usize = 48;

/// One operator's work area.
pub(super) struct Work {
    limit: usize,
    /// Its grant, shared with its parts, which hold shares of it.
    grant: Arc<Grant>,
    /// The bytes of rows a run gathers before it writes them.
    batch: usize,
    /// The most partitions an operator writes at once, and the most times
    /// it partitions one again.
    partitions: (usize, usize),
    areas: Arc<WorkAreas>,
    area: Option<Box<dyn SpillArea>>,
    /// The types beyond the standard the rows it spills may hold values of,
    /// read back by them (`SpilledLobs::record_row`).
    types: TypeRegistry,
    /// The sources of the large objects and XML values the rows it spills
    /// name, held while it, or a part of it, is: shared with its parts, as
    /// rows pass between them (`spill/named.rs`).
    lobs: SpilledLobs,
    /// What cancels the statement it works for, looked at as its runs are
    /// read back; none if it is not known.
    stop: Option<Arc<Stop>>,
    /// The database's seed, and the word and count of the seeds it draws
    /// for its partitioners (`drawn`), boxed, as an operator holds its
    /// work area beside its state.
    seed: Box<(Seed, u64, Cell<u64>)>,
}

impl Work {
    /// The work area of an operator of `db`.
    pub(super) fn new(db: &State) -> Work {
        let (ceiling, batch) = (work_bytes(db.settings.get(MEMORY_WORK_BYTES)), batch_bytes(db.settings.get(SPILL_BATCH_BYTES)));
        let partitions = (db.settings.count(SPILL_PARTITIONS), db.settings.count(SPILL_PARTITION_DEPTH));
        let (areas, types) = (Arc::clone(&db.work), db.types.clone());
        let grant = areas.grant(ceiling);
        let limit = grant.bytes();
        let seed = Box::new((db.functions.seed().clone(), db.functions.seed().xxh3(b"partitions"), Cell::new(0)));
        Work { limit, grant, batch, partitions, areas, area: None, types, lobs: SpilledLobs::new(), stop: None, seed }
    }

    /// The most an operator of `db` is granted, MEMORY.WORK_BYTES, for
    /// what reserves what it holds itself.
    pub(super) fn ceiling_of(db: &State) -> usize {
        work_bytes(db.settings.get(MEMORY_WORK_BYTES))
    }

    /// Whether the statement it works for goes on: an error once it was
    /// cancelled or ran past its time (RULES.md D380).
    ///
    /// # Errors
    ///
    /// SQLCODE -920 if it was cancelled or timed out.
    pub(super) fn look(&self) -> Result<()> {
        self.stop.as_deref().map_or(Ok(()), Stop::check)
    }

    /// What cancels the statement it works for, if known.
    pub(super) fn stop(&self) -> Option<Arc<Stop>> {
        self.stop.clone()
    }

    /// The work area of an operator of `db` run by `tx`'s statement: its
    /// runs read back only while the statement is not cancelled.
    pub(super) fn of(db: &State, tx: TxId) -> Work {
        Work { stop: db.tx.stop_of(tx), ..Work::new(db) }
    }

    /// A work area of the same settings, of a grant of its own of up to
    /// its bytes: for an operator's part that holds rows apart from the rest.
    pub(super) fn another(&self) -> Work {
        let grant = self.areas.grant(self.limit);
        Work { limit: grant.bytes(), grant, ..self.shared(self.limit) }
    }

    /// A work area of the same settings, of `limit` bytes of its grant: for
    /// a part of an operator held beside its others in a part of its area.
    pub(super) fn part(&self, limit: usize) -> Work {
        self.shared(limit)
    }

    /// A work area of the same settings and grant, of `limit` bytes.
    fn shared(&self, limit: usize) -> Work {
        let (batch, partitions, stop) = (self.batch, self.partitions, self.stop.clone());
        let (areas, types, lobs, grant) = (Arc::clone(&self.areas), self.types.clone(), self.lobs.clone(), Arc::clone(&self.grant));
        let seed = Box::new((self.seed.0.clone(), self.drawn(), Cell::new(0)));
        Work { limit, grant, batch, partitions, areas, area: None, types, lobs, stop, seed }
    }

    /// \[DESIGN\] (RULES.md D357) The seed of the next partitioner it
    /// makes: the XXH3-64 of its count, seeded by a word of the database's
    /// seed, drawn at random as the database opens, so no client knows
    /// which keys share a partition. Each partitioner of an operator, and
    /// of each level it partitions again, hashes by a seed of its own, and
    /// two runs of a statement alike draw the same, so a spilled operator
    /// gives its rows in the same order each time. A part of the work area
    /// draws its word from the area's.
    pub(super) fn drawn(&self) -> u64 {
        let (_, word, count) = &*self.seed;
        let n = count.get();
        count.set(n.wrapping_add(1));
        crate::hash::xxhash::xxh3_64_with_seed(&n.to_le_bytes(), *word)
    }

    /// The database's seed of its hash tables (RULES.md D357).
    pub(super) fn seed(&self) -> &Seed {
        &self.seed.0
    }

    /// Half of it, of its own, the other half left to it, an area of each
    /// half made of each when needed: for a part of an operator that holds
    /// rows beside it, so the two hold no more than one work area does.
    /// Made before it makes its area.
    pub(super) fn split(&mut self) -> Work {
        let half = self.limit.div_ceil(2);
        self.limit = self.limit.saturating_sub(half);
        self.shared(half)
    }

    /// The types beyond the standard the rows it spills may hold values of.
    pub(super) fn types(&self) -> &TypeRegistry {
        &self.types
    }

    /// What holds the sources of the large objects and XML values the rows
    /// it spills name, to write and read them by.
    pub(super) fn lobs(&self) -> &SpilledLobs {
        &self.lobs
    }

    /// The bytes it may hold in memory.
    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    /// The bytes of records a run gathers before it writes them.
    pub(super) fn batch(&self) -> usize {
        self.batch
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
            // Its pool caches the area's pages, its rows held of the grant.
            self.area = Some(self.areas.area(Area::new(self.limit.min(WORK_FLOOR), self.batch))?);
        }
        self.area.as_deref().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a spill area was not made"))
    }

    /// A new table of its spill area.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the area cannot be reserved; the area's errors.
    pub(super) fn table(&mut self) -> Result<Box<dyn SpillTable>> {
        let table = self.area()?.table()?;
        Ok(match &self.stop {
            Some(stop) => Box::new(Weighed { table, stop: Arc::clone(stop) }),
            None => table,
        })
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
        // [DESIGN] RULES.md D521: the bytes the statement spilled.
        self.stop.activity().spilled(record.len());
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

/// A table of a spill area whose bytes added are counted as the
/// statement's spilled (RULES.md D521).
struct Weighed {
    table: Box<dyn SpillTable>,
    stop: Arc<Stop>,
}

impl SpillTable for Weighed {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.stop.activity().spilled(key.len().saturating_add(value.len()));
        self.table.insert(key, value)
    }

    fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.table.get(key)
    }

    fn values(&self, key: &[u8]) -> Result<Records> {
        self.table.values(key)
    }

    fn remove(&mut self, key: &[u8], value: &[u8]) -> Result<bool> {
        self.table.remove(key, value)
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
        self.change(key, |n| n.saturating_add(more)).map(drop)
    }

    /// Adds `key` if it is not in the bag; whether it was not.
    pub(super) fn add_new(&mut self, key: &[u8]) -> Result<bool> {
        Ok(self.change(key, |n| n.max(1))? == 0)
    }

    /// Adds one to the count of `key`; whether it was 0.
    pub(super) fn raise(&mut self, key: &[u8]) -> Result<bool> {
        Ok(self.change(key, |n| n.saturating_add(1))? == 0)
    }

    /// Takes one from the count of `key`, if it is above 0; whether it is
    /// 0 now, one taken.
    pub(super) fn lower(&mut self, key: &[u8]) -> Result<bool> {
        Ok(self.change(key, |n| n.saturating_sub(1))? == 1)
    }

    /// Takes one from the count of `key`, if it is above 0; whether it was.
    pub(super) fn take(&mut self, key: &[u8]) -> Result<bool> {
        Ok(self.change(key, |n| n.saturating_sub(1))? > 0)
    }

    /// Changes the count of `key` to `now` of it; the count it was. In
    /// memory a key held is looked up once and one not twice, its bytes
    /// made only then, and one whose count stays 0 is not added.
    fn change(&mut self, key: &[u8], now: impl FnOnce(u64) -> u64) -> Result<u64> {
        if self.table.is_some() {
            let was = self.count(key)?;
            self.set(key, (was, now(was)))?;
            return Ok(was);
        }
        if let Some(n) = self.counts.get_mut(key) {
            let was = *n;
            *n = now(was);
            return Ok(was);
        }
        let count = now(0);
        if count > 0 {
            self.set(key, (0, count))?;
        }
        Ok(0)
    }

    /// Changes the count of `key` from `was` to `now`: in memory, of a
    /// key not held (`change`).
    fn set(&mut self, key: &[u8], (was, now): (u64, u64)) -> Result<()> {
        if let Some(table) = &mut self.table {
            if was > 0 {
                table.remove(key, &was.to_le_bytes())?;
            }
            return if now > 0 { table.insert(key, &now.to_le_bytes()) } else { Ok(()) };
        }
        self.counts.insert(key.to_vec(), now);
        self.bytes = self.bytes.saturating_add(key.len()).saturating_add(ENTRY_BYTES);
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
