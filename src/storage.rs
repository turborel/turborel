//! \[DESIGN\] Row storage. "The physical representation of a value is
//! implementor-defined" [X3.135-1986 §4.2], so how rows are kept is an
//! extension point.
//!
//! A `StorageEngine` makes a `TableStorage` for each base table. An engine
//! hands back rows by id, scans them, prices those operations, and declares
//! what it can do inside a transaction. The planner, executor and transaction
//! coordinator never know which engine they are talking to.
//!
//! Every method takes `&self`: an engine guards its own state, which is the
//! shape a thread-safe engine needs. Every data method takes the `TxId`,
//! which is also the transaction's logical start timestamp, and commit hands
//! each engine the transaction's commit timestamp (RULES.md D29), so an
//! engine with its own concurrency control (say, multi-versioning) can show
//! each transaction what it should see.
//!
//! One engine is built in, the default: "rowstore" (`engine/rows.rs`, RULES.md
//! D31), which isolates transactions itself, with versions, and undoes its
//! own changes; a database made with `Database::new` keeps it in memory.

mod batch;
mod condition;
mod options;

pub use batch::{BATCH_ROWS, Batch, Batches, Column, Ints, batches_of};
pub use condition::{Condition, Conjunct, Conjunction};

use std::sync::Arc;

use crate::catalog::TableDesc;
use crate::error::Result;
use crate::log::Lsn;
use crate::memory::Host;
use crate::stats::Statistics;
use crate::txn::{NO_TRANSACTION, ReadMode, Timestamp, TxCapabilities, TxId};
use crate::value::Value;

pub use options::{ENCODINGS, Rebuild, TableOption, chosen, encodings_of, encodings_option, needs_rebuild, option_value};

/// A row: one value per column, in column order.
pub type Row = Vec<Value>;

/// The values of `row` at the positions `columns`, in that order; `None` if a
/// position is past the end of the row.
pub fn project(row: &[Value], columns: &[usize]) -> Option<Row> {
    columns.iter().map(|&c| row.get(c).cloned()).collect()
}

/// Identifies a row within its table, for as long as the table exists.
pub type RowId = u64;

/// \[DESIGN\] Rows a table gives, with their ids, read as they are pulled.
/// The iterator owns what it reads, so it can be kept between calls, and
/// moved to another thread with the cursor reading it.
pub type Rows = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;

/// \[DESIGN\] The default cost of one fetch by row id: a random read costs
/// about three sequential ones.
pub const DEFAULT_FETCH_COST: f64 = 3.0;

/// A count of rows as a cost. Exact up to 2^32 rows; costs are estimates, so
/// beyond that it saturates.
pub fn cost_of(rows: usize) -> f64 {
    f64::from(u32::try_from(rows).unwrap_or(u32::MAX))
}

/// \[DESIGN\] A kind of table storage: creates the `TableStorage` of each
/// table placed in it.
///
/// Contract (RULES.md R-SAFE-3; checked for the built-in engines by
/// `test/integration/contracts.rs`):
/// * `name` is unique among the registered engines, and constant.
/// * `create_table` returns an empty table for the columns of `desc`, or an
///   error, and then makes nothing.
/// * `open_table` returns the table whose `locator` it gave, holding every
///   change up to its `durable_through`, or `None` if it keeps nothing
///   between runs; the database then makes it anew and replays the log.
/// * `checkpoint(at)` is called with no change half made, and captures
///   every change logged before `at`; after `checkpoint_finish` then
///   succeeds, they are durable in the engine, and each table's
///   `durable_through` is at least `at`. Changes made between the two are
///   not part of the checkpoint.
/// * `attach` is called once, as the engine is registered, before any other
///   method but `name`; the memory its caches hold it reserves of the
///   `Host`'s budget, giving it back as they are dropped (RULES.md D38).
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait StorageEngine: Send + Sync {
    /// The name the engine is chosen by.
    fn name(&self) -> &'static str;
    /// \[EXTENSION\] The options its tables take (RULES.md D41), each with
    /// its kind and default, set by `CREATE TABLE ... WITH (...)` and changed
    /// by ALTER TABLE; `create_table` and `open_table` find a table's in
    /// its descriptor (`TableDesc::options`). Default: none.
    fn table_options(&self) -> Vec<TableOption> {
        Vec::new()
    }
    /// \[EXTENSION\] Whether it passes its tables' rows through their
    /// ENCODINGS as it stores them (RULES.md D48), each with its encodings
    /// named (`Pipeline::seal`); a table of an engine that does not may name
    /// none. Default: no.
    fn seals(&self) -> bool {
        false
    }
    /// Takes what the database registering it provides: its memory budget.
    /// Default: nothing, for an engine that holds no cache.
    ///
    /// # Errors
    ///
    /// The engine cannot work with it; it is not registered.
    fn attach(&self, _host: &Host) -> Result<()> {
        Ok(())
    }
    /// The storage for a new, empty table described by `desc`.
    ///
    /// # Errors
    ///
    /// The engine cannot make the table (an engine on disk, its file); the
    /// definition fails.
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>>;
    /// The storage of the table described by `desc` that this engine made
    /// before, found by the `locator` it gave (`TableStorage::locator`), or
    /// `None` if it keeps nothing between runs. Default: `None`.
    ///
    /// # Errors
    ///
    /// The engine cannot read the table; the database is not opened.
    fn open_table(&self, _desc: &TableDesc, _locator: &[u8]) -> Result<Option<Box<dyn TableStorage>>> {
        Ok(None)
    }
    /// Captures every change logged before position `at`, for
    /// `checkpoint_finish` to make durable (RULES.md D25,
    /// `db/checkpoint.rs`). Called with writers held, so it should be quick.
    /// Default: nothing, for an engine that keeps nothing.
    ///
    /// # Errors
    ///
    /// The engine cannot write its files; the checkpoint fails, and the
    /// last one stands.
    fn checkpoint(&self, _at: Lsn) -> Result<()> {
        Ok(())
    }
    /// Makes what the last `checkpoint` captured durable, writers going on
    /// meanwhile. Default: nothing.
    ///
    /// # Errors
    ///
    /// The engine cannot write or sync its files; the checkpoint fails, and
    /// the last one stands.
    fn checkpoint_finish(&self) -> Result<()> {
        Ok(())
    }
}

/// \[DESIGN\] The rows of one table. Row ids identify rows within the table.
///
/// Contract (RULES.md R-SAFE-3; checked for the built-in engines by
/// `test/integration/contracts.rs`):
/// * `insert` returns an id that no live row has and that no deleted row ever
///   had, so the undo log's `restore` can never collide with a newer row.
/// * `get` returns exactly the live row with that id; `scan` yields every live
///   row once, with its id, in an order of the engine's choosing; `row_count`
///   is the number of live rows. An engine that cannot read a row (an I/O
///   error, a corrupt page) says so with an error, from `get` or as the
///   scan's next item, never by leaving the row out.
/// * `update` and `delete` change a live row and fail, changing nothing, for
///   an id that is not live. `restore` makes a deleted row live again under
///   its old id.
/// * An engine declaring `Concurrency::Coordinator` may ignore `tx`.
/// * Costs are finite and not negative.
/// * Undo (`rollback_to`, `rollback`) reports any failure (RULES.md D19), and
///   `commit` does not fail after a successful `validate`.
/// * An engine declaring `Concurrency::Engine` shows transaction `tx` the
///   rows committed at timestamps before `tx`, and its own changes; it
///   refuses a write to a row another open transaction wrote, and if it
///   declares itself serializable, refuses at `validate` a transaction that
///   read what a transaction committed meanwhile changed (SQLCODE -911),
///   of the reads it was told are checked (`read_mode`).
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait TableStorage: Send + Sync {
    /// Adds `row` and returns its new id.
    ///
    /// # Errors
    ///
    /// The engine cannot store the row; the statement fails.
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId>;
    /// Replaces the live row `id` with `row`.
    ///
    /// # Errors
    ///
    /// No live row has that id, or the engine cannot store the row.
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()>;
    /// Removes the live row `id`.
    ///
    /// # Errors
    ///
    /// No live row has that id, or the engine cannot remove it.
    fn delete(&self, tx: TxId, id: RowId) -> Result<()>;
    /// Put a deleted row back under its old id. The coordinator's undo log
    /// uses this, so an engine with `Undo::Log` must support it.
    ///
    /// # Errors
    ///
    /// The row cannot be put back; the coordinator then marks the table damaged.
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()>;
    /// The live row `id`, if there is one.
    ///
    /// # Errors
    ///
    /// The engine cannot read the row.
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>>;
    /// Every live row, once each, with its id. The rows have every column
    /// at its position, but only those in `columns` need hold their value:
    /// the others may hold anything (a column layout leaves them NULL), and
    /// callers do not read them (RULES.md D31). A row the engine cannot read
    /// is an error item, after which the scan may end. The scan holds what
    /// it reads (`self`), so it can outlive the call that started it: a
    /// cursor reads its rows as it is fetched from.
    fn scan(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Rows;
    /// As `scan`, for a statement that keeps only rows `conditions` all
    /// match, and checks every row itself: an engine may leave out rows that
    /// do not match, and, checking serializability, count only those that
    /// do as read. Default: `scan`.
    fn scan_where(self: Arc<Self>, tx: TxId, columns: &[usize], _conditions: &Conjunction) -> Rows {
        self.scan(tx, columns)
    }
    /// \[DESIGN\] The rows `scan_where` gives, or `scan` with no
    /// `conditions`, in no particular order, a batch of up to `BATCH_ROWS` of
    /// them at a time: the values of `columns`, a vector for each, in the
    /// order asked (RULES.md D31). An engine that keeps columns gives them
    /// without making each row. Default: `scan_where`'s rows, gathered.
    fn scan_batches(self: Arc<Self>, tx: TxId, columns: &[usize], conditions: Option<&Conjunction>) -> Batches {
        let rows = match conditions {
            Some(c) => self.scan_where(tx, columns, c),
            None => self.scan(tx, columns),
        };
        batches_of(rows, columns)
    }
    /// \[DESIGN\] Whether `scan_batches` gives its rows without making each,
    /// so a statement given rows a few at a time, a cursor's, reads them by
    /// batch too, a batch read ahead of what it is given. Default: false,
    /// its batches being its rows gathered, read ahead for nothing.
    fn reads_by_batch(&self) -> bool {
        false
    }
    /// Used by the planner as a statistic.
    fn row_count(&self) -> usize;
    /// `tx` is about to read, by `get`, the rows an index chose by
    /// `condition`: its result depends on every row the condition matches,
    /// which an engine checking serializability must count as read, rows
    /// added or changed to match meanwhile too (phantoms). Default:
    /// nothing, for an engine whose isolation is the coordinator's.
    ///
    /// # Errors
    ///
    /// The engine cannot record the read; the statement fails.
    fn read_by_condition(&self, _tx: TxId, _condition: &Condition) -> Result<()> {
        Ok(())
    }
    /// The newest state of row `id` and any change to it not yet committed,
    /// whichever transaction made them, for checking UNIQUE
    /// [X3.135-1986 §6.4 GR2] against rows `tx` may not see. Default: the
    /// row as `tx` sees it, for an engine whose isolation is the
    /// coordinator's, where no other transaction's change is pending.
    ///
    /// # Errors
    ///
    /// As `get`.
    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        Ok(Newest { row: self.get(tx, id)?, pending: None })
    }

    // ----- Cost model. Units: 1.0 = reading one whole row in sequence. -----

    /// Cost of reading every row when the query needs only `columns`.
    /// Default: a row store reads whole rows whatever columns are needed.
    fn scan_cost(&self, _columns: &[usize]) -> f64 {
        cost_of(self.row_count())
    }

    /// \[DESIGN\] The share of its rows a scan told `conditions`
    /// (`scan_where`) reads, from what the engine keeps of them, such as
    /// the least and greatest values of blocks of rows (RULES.md D31); the
    /// planner scales `scan_cost` by it. Default: 1, every row.
    fn scan_share(&self, _conditions: &Conjunction) -> f64 {
        1.0_f64
    }

    /// Cost of one `get` by row id. Default: a random read in a row store.
    fn fetch_cost(&self) -> f64 {
        DEFAULT_FETCH_COST
    }

    /// What the engine knows of its rows, for the planner's estimates
    /// (`stats.rs`): each engine answers what its layout tells it. Default:
    /// nothing, and the planner assumes its defaults.
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        None
    }

    /// Gathers the statistics `statistics` gives anew, now. Default:
    /// nothing to gather.
    ///
    /// # Errors
    ///
    /// The engine cannot read its rows; the statistics are left as they were.
    fn analyze(&self) -> Result<()> {
        Ok(())
    }

    // ----- Transactions (see txn.rs). The defaults suit an engine that lets
    // the coordinator do undo and locking. An engine declaring `Undo::Native`
    // or `Concurrency::Engine` implements these. -----

    /// What the engine does for transactions itself. Default: nothing; the
    /// coordinator keeps the undo log and the locks.
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities::default()
    }
    /// Transaction `tx` joins this table: at its first write, or, for an
    /// engine declaring `Concurrency::Engine`, at its first read. `tx` is its
    /// start timestamp; it reads at `snapshot`, seeing what committed at or
    /// before it, every such commit having ended (RULES.md D29). Only a table
    /// whose `begin` succeeded is asked to `validate`, `commit` or `rollback`.
    ///
    /// # Errors
    ///
    /// The engine cannot take part in the transaction; the statement fails.
    fn begin(&self, _tx: TxId, _snapshot: Timestamp) -> Result<()> {
        Ok(())
    }
    /// \[EXTENSION\] How `tx` reads from now, at a level below SERIALIZABLE
    /// [ISO/IEC 9075:1992 §4.28] (RULES.md D29): as of `mode.snapshot`,
    /// later than or the same as the one before, and, unless `mode.checked`,
    /// with what it reads not checked at `validate`; a write to a row
    /// committed after the snapshot is refused still, so no update is lost.
    /// Told once `tx` has begun here, and whenever the mode changes. Default:
    /// nothing, for an engine that reads at the snapshot `tx` began at and
    /// checks every read, as SERIALIZABLE, which shows none of the
    /// phenomena a weaker level would [§14.1 GR5].
    ///
    /// # Errors
    ///
    /// The engine cannot change how `tx` reads; the statement fails.
    fn read_mode(&self, _tx: TxId, _mode: ReadMode) -> Result<()> {
        Ok(())
    }
    /// Mark a point that `rollback_to` can return to (statement atomicity).
    fn savepoint(&self, _tx: TxId) -> u64 {
        0
    }
    /// Undo the changes `tx` made after `savepoint`.
    ///
    /// # Errors
    ///
    /// Any failure to undo. The coordinator then marks the table damaged and
    /// refuses every later statement on it (SQLCODE -914), because its
    /// contents are no longer a state any transaction produced.
    fn rollback_to(&self, _tx: TxId, _savepoint: u64) -> Result<()> {
        Ok(())
    }
    /// Commit, phase 1: `tx` is to commit at timestamp `at`, later than every
    /// timestamp given out before it. The engine checks what its isolation
    /// needs (RULES.md D29): that `tx` committing at `at` keeps what its
    /// concurrency control promises. That is all a `Persistence::Logged`
    /// or `None` engine promises here: its changes are as durable as the
    /// log's commit record, written after this (RULES.md D25), and its
    /// `commit` has nothing left that could fail. An engine that keeps its
    /// own durability (`Forced`, `External`) promises too that its `commit`
    /// will succeed, as a participant of a two-phase commit prepares. Until
    /// `commit` or `rollback`, the changes of `tx` are committing at `at`: a
    /// transaction that started after `at` must not read past them as if
    /// they were not there.
    ///
    /// # Errors
    ///
    /// The engine refuses: SQLCODE -911 for a conflict with another
    /// transaction; any other error if it cannot promise to commit, as only
    /// an engine keeping its own durability may. The transaction is rolled
    /// back in every engine.
    fn validate(&self, _tx: TxId, _at: Timestamp) -> Result<()> {
        Ok(())
    }
    /// Commit, phase 2: the changes of `tx` become committed at timestamp
    /// `at`, the same in every engine. Must not fail after a successful
    /// `validate`. A transaction that wrote nothing commits at its snapshot,
    /// with no `validate` before: it read one committed state.
    fn commit(&self, _tx: TxId, _at: Timestamp) {}
    /// The log position before which every change of this table is
    /// durable in the engine itself, as of its last checkpoint: recovery
    /// redoes and undoes from there (RULES.md D25, `db/recovery.rs`).
    /// Default: 0, the engine keeping nothing, as memory.
    fn durable_through(&self) -> Lsn {
        Lsn(0)
    }
    /// The rows of the versions superseded by commits at or before `as_of`
    /// but not yet collected, whose index entries collection (`gc`) would
    /// take back, with their ids, read as they are pulled: what a checkpoint
    /// logs, so recovery takes them back if the crash lost the collection
    /// (`db/checkpoint.rs`). Collection of the table is held back while
    /// they are read. Default: none, for an engine keeping no versions.
    ///
    /// # Errors
    ///
    /// The engine cannot read a version; the checkpoint fails.
    fn garbage(self: Arc<Self>, _as_of: Timestamp) -> Rows {
        Box::new(std::iter::empty())
    }
    /// What finds this table again when the database is opened
    /// (`StorageEngine::open_table`), kept in the catalog; meaningful only to
    /// the engine. Default: nothing.
    fn locator(&self) -> Vec<u8> {
        Vec::new()
    }
    /// \[EXTENSION\] The table was dropped [ISO/IEC 9075:1992 §11.18 GR2]:
    /// once the last reference to it goes, it is not read or written again,
    /// and may then release its storage. Until then, readers of earlier
    /// snapshots may still read it. Default: nothing, for a table that
    /// holds only memory.
    fn discard(&self) {}
    /// \[EXTENSION\] The table's options that change in place
    /// (`Rebuild::No`) changed: `desc` holds them now (RULES.md D41).
    /// Called once the change is published, never with a change that needs
    /// a rebuild; such options tune the storage and change no result, so a
    /// table that cannot take one at once may take it when it is next made
    /// or opened. Default: nothing, for a table with no such options.
    fn reconfigure(&self, _desc: &TableDesc) {}
    /// Makes row `id` hold `row`, or be absent if `None`, whatever it holds
    /// now: how recovery replays and undoes logged changes, so applying the
    /// same change twice is as applying it once. Default: through `get`,
    /// `update`, `restore` and `delete`, outside any transaction.
    ///
    /// # Errors
    ///
    /// The engine's, if it cannot store or remove the row.
    fn apply(&self, id: RowId, row: Option<Row>) -> Result<()> {
        match (row, self.get(NO_TRANSACTION, id)?.is_some()) {
            (Some(row), true) => self.update(NO_TRANSACTION, id, row),
            (Some(row), false) => self.restore(NO_TRANSACTION, id, row),
            (None, true) => self.delete(NO_TRANSACTION, id),
            (None, false) => Ok(()),
        }
    }
    /// Garbage collection of versions: removes the versions of rows that no
    /// transaction reading at or after `horizon` can read, and gives each
    /// removed version's row, with its id, to `removed` as it goes, so
    /// their index entries are removed too (one each: `Index`), none held
    /// all together.
    /// Called after a transaction that joined the table ends. Default:
    /// nothing, for an engine that keeps no versions.
    ///
    /// # Errors
    ///
    /// The engine cannot read or remove them; they stay for a later call.
    fn gc(&self, _horizon: Timestamp, _removed: &mut dyn FnMut(RowId, Row)) -> Result<()> {
        Ok(())
    }
    /// Whether versions of it await collection (`gc`), so it is collected
    /// as any transaction ends, though none touches it. Default: none, for
    /// an engine that keeps no versions.
    fn uncollected(&self) -> bool {
        false
    }
    /// \[DESIGN\] Whether work is due that the engine leaves to the
    /// database's maintenance thread (`maintain`), so no transaction's end
    /// waits for it: asked after each `gc`, so it should be quick. Default:
    /// none.
    fn maintenance_due(&self) -> bool {
        false
    }
    /// \[DESIGN\] Does some of the work due (`maintenance_due`), of rows
    /// committed at or before `horizon`, before every snapshot; on the
    /// database's maintenance thread, or when it is asked to
    /// (`Database::maintain`), never two calls at once: whether it did any,
    /// for it to be called again. Default: none.
    ///
    /// # Errors
    ///
    /// The engine cannot do it; it is tried again later.
    fn maintain(&self, _horizon: Timestamp) -> Result<bool> {
        Ok(false)
    }
    /// Every version a new index needs an entry for, with its row id,
    /// `columns` holding its values as for `scan`, among those made by
    /// transactions committed at or before `as_of`: each committed version
    /// a transaction reading at or after `horizon` may still read, and each
    /// version `gc` will report removed that none can. An index holds one
    /// entry per version, which `gc` takes back (`Index`), so it must have
    /// been given one. Default: the rows as read outside any transaction,
    /// for an engine that keeps no versions.
    fn versions(self: Arc<Self>, _horizon: Timestamp, _as_of: Timestamp, columns: &[usize]) -> Rows {
        self.scan(NO_TRANSACTION, columns)
    }
    /// Undo every change `tx` made that the engine keeps itself, and forget
    /// the transaction. Also called after a failed `validate`.
    ///
    /// # Errors
    ///
    /// Any failure to undo, with the consequence described at `rollback_to`.
    fn rollback(&self, _tx: TxId) -> Result<()> {
        Ok(())
    }
}

/// \[DESIGN\] The newest state of a row, for checking UNIQUE against rows a
/// transaction may not see (`TableStorage::newest`).
#[derive(Debug, Clone)]
pub struct Newest {
    /// The newest committed version, or the transaction's own change: none
    /// if the row is absent or deleted.
    pub row: Option<Row>,
    /// A change another open transaction made to it: the row it leaves,
    /// none for a delete. While it is pending the row is in doubt.
    pub pending: Option<Option<Row>>,
}
