//! \[DESIGN\] Recovery on start (RULES.md D25): a
//! database brought back from its log. Either the log's definitions are its
//! catalog, the database recovered into a new one with its engines, index
//! methods and functions registered but no schema (`Database::recover`), or
//! its last checkpoint is, its tables reopened as the checkpoint left them
//! (`db/files.rs`). Reading the log from where the checkpoint says, or its
//! start:
//!
//! 1. **Analysis**: which transactions committed, and where; which rolled
//!    back, and where; which statements were undone (`StatementUndone`),
//!    cancelling their changes; definitions replayed (not logged again),
//!    those the checkpoint holds left out.
//! 2. **Undo**, in reverse order: each change that does not stand (its
//!    transaction did not commit, or a statement undone cancelled it) and
//!    whose undo the engine may not hold. For a `Logged` table, whose pages
//!    hold every change logged before its `durable_through` (L), that is
//!    one whose undo, its rollback's abort record or its statement's
//!    undone record, is not before L, or never came; for a `Forced` table,
//!    one of a transaction a crash interrupted.
//! 3. **Redo**, in order: each change that stands, of a `Logged` table, at
//!    or after L, or whose commit record is: its version in the pages, made
//!    before L, is not yet stamped committed.
//! 4. The indexes made whole, as index entries are not logged: rebuilt from
//!    their tables, or, for indexes reopened as the checkpoint left them,
//!    mended where recovery touched rows (`State::mend_indexes`); the
//!    transaction counter moved past every id and timestamp read.
//!
//! The row changes read are kept in a list that spills past WORK_MEMORY
//! (`recovery/changes.rs`, RULES.md D38), read a segment at a time by each
//! pass, so recovery holds a work area's bytes of them however much of the
//! log it reads.
//!
//! A row is written by one transaction at a time, until it ends, so undoing
//! then redoing in log order leaves each row as the last change that stands
//! left it. Changes are applied with `TableStorage::apply`, which is
//! idempotent, so recovering again after a crash during recovery gives the
//! same result.

mod changes;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(super) use changes::Touched;
use changes::{RowChange, decode_change, encode_change};

use super::indexes::IndexSpec;
use super::{Database, State};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn};
use crate::spill::Area;
use crate::storage::{Row, RowId, project};
use crate::txn::Timestamp;
use crate::txn::{NO_TRANSACTION, Persistence, TxId};
use crate::value::Value;

/// Which of the log's definitions reading it replays: those after the
/// checkpoint whose image gave the others (`db/files.rs`), or all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Definitions {
    /// Every one.
    All,
    /// Those at or after this position.
    From(Lsn),
}

/// How reading the log replays definitions: which (`Definitions`), and
/// those each transaction made, until its commit record makes them, or its
/// abort record forgets them.
struct Replaying {
    definitions: Definitions,
    pending: HashMap<TxId, Vec<Change>>,
}

impl Replaying {
    /// Whether a definition made at `lsn` is replayed.
    fn replays(&self, lsn: Lsn) -> bool {
        match self.definitions {
            Definitions::All => true,
            Definitions::From(at) => lsn >= at,
        }
    }
}

/// What recovery did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Records read.
    pub records: u64,
    /// Definitions replayed: `<schema>`s and indexes.
    pub definitions: u64,
    /// Transactions that changed rows and committed.
    pub committed: u64,
    /// Transactions that changed rows and did not commit: rolled back, or
    /// interrupted.
    pub not_committed: u64,
    /// Row changes redone.
    pub redone: u64,
    /// Row changes undone.
    pub undone: u64,
}

/// The most entries of one row under one key mending takes back: a bound
/// on a loop that should take back a few.
const MOST_ENTRIES: usize = 1_024;

/// The key of `row` in an index on `columns`, unless it has a null: such
/// keys are never inserted (`Index::insert`).
fn key_of(row: &Row, columns: &[usize]) -> Option<Key> {
    project(row, columns).map(Key).filter(|k| !k.0.iter().any(Value::is_null))
}

/// Takes back every entry of `row` under `key` in `index`.
fn take_back(index: &dyn Index, key: &Key, row: RowId) -> Result<()> {
    for _ in 0..MOST_ENTRIES {
        if !index.rows(key)?.contains(&row) {
            break;
        }
        index.remove(key, row)?;
    }
    Ok(())
}

/// Whether `change` was made in the storage its table has now. A table no
/// longer in the catalog was dropped, and one of another generation was
/// given a new storage by ALTER TABLE, its rows copied there by changes
/// logged after (`db/ddl.rs`), or is another table defined under the name
/// of one dropped, its generation later (`State::retired`): the changes of
/// each need nothing.
fn current(st: &State, change: &RowChange) -> bool {
    st.catalog.table(&change.table).is_some_and(|t| t.generation == change.generation)
}

/// What analysis found.
#[derive(Default)]
struct Analysis {
    /// Each transaction committed, and where its commit record is.
    committed: HashMap<TxId, Lsn>,
    /// Each transaction rolled back, and where its abort record is.
    rolled_back: HashMap<TxId, Lsn>,
    /// For each transaction, the ranges of positions its undone statements
    /// cancelled: from the statement's first change to its undo record.
    cancelled: HashMap<TxId, Vec<(Lsn, Lsn)>>,
    transactions: HashSet<TxId>,
    /// The highest transaction id or timestamp read.
    highest: u64,
}

impl Analysis {
    fn note(&mut self, lsn: Lsn, record: &Record) {
        self.highest = self.highest.max(record.tx);
        match &record.change {
            Change::Commit { at } => {
                self.highest = self.highest.max(*at);
                self.committed.insert(record.tx, lsn);
                self.transactions.insert(record.tx);
            }
            Change::StatementUndone { from } => self.cancelled.entry(record.tx).or_default().push((*from, lsn)),
            // A transaction counts once it changed a row or ended; one that
            // only defined an index does neither.
            Change::Abort => {
                self.rolled_back.insert(record.tx, lsn);
                self.transactions.insert(record.tx);
            }
            Change::Insert { .. } | Change::Update { .. } | Change::Delete { .. } => {
                self.transactions.insert(record.tx);
            }
            Change::Schema { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Drop { .. }
            | Change::AlterTable { .. }
            | Change::Defined { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }
            | Change::Checkpoint
            | Change::Stale { .. } => {}
        }
    }

    /// Where the undone statement that cancelled `change` was undone, if
    /// one did.
    fn cancelled_at(&self, change: &RowChange) -> Option<Lsn> {
        let ranges = self.cancelled.get(&change.tx)?;
        ranges.iter().find(|&&(from, to)| from <= change.lsn && change.lsn < to).map(|&(_, to)| to)
    }

    /// Whether `change` stands: its transaction committed, and no undone
    /// statement cancelled it.
    fn stands(&self, change: &RowChange) -> bool {
        self.committed.contains_key(&change.tx) && self.cancelled_at(change).is_none()
    }

    /// Where a change that does not stand was undone as the database ran:
    /// its statement's undo record, or its transaction's abort record;
    /// `None` if a crash interrupted it.
    fn undone_at(&self, change: &RowChange) -> Option<Lsn> {
        self.cancelled_at(change).or_else(|| self.rolled_back.get(&change.tx).copied())
    }
}

impl Database {
    /// \[DESIGN\] Rebuilds this database from `log`, then logs to it: see the
    /// module's documentation. The engines, index methods and functions the
    /// log's definitions name must be registered first.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database already holds a schema; the errors of
    /// reading the log (SQLCODE -916 for a malformed record), of replaying
    /// a definition, or of an engine applying a change.
    pub fn recover(&self, log: Arc<dyn Log>) -> Result<RecoveryReport> {
        if !self.snapshot()?.catalog.tables().is_empty() {
            return Err(SqlError::new(Code::Api, "[DESIGN] recovery rebuilds a database from its log, so it must begin with no schema"));
        }
        let (report, _) = self.replay_log(&*log, Lsn(0), Definitions::All)?;
        self.change(State::rebuild_indexes)?;
        self.set_log(log)?;
        self.budget_from_settings()?;
        Ok(report)
    }

    /// Reads `log` from `from`: analysis, definitions replayed as
    /// `definitions` says, then undo and redo; the counters moved past the
    /// ids and timestamps read. What it did, and the rows it read changes
    /// of.
    pub(super) fn replay_log(&self, log: &dyn Log, from: Lsn, definitions: Definitions) -> Result<(RecoveryReport, Touched)> {
        let mut report = RecoveryReport::default();
        let mut analysis = Analysis::default();
        let mut changes = Touched::new(self.snapshot()?.list_space());
        let mut replaying = Replaying { definitions, pending: HashMap::new() };
        let encodings = Arc::clone(&self.snapshot()?.encodings);
        let types = self.snapshot()?.types.clone();
        for read in log.read_from(from) {
            let (lsn, bytes) = read?;
            let record = crate::log::sealed::open(&bytes, &encodings, &types)?;
            report.records = report.records.saturating_add(1);
            analysis.note(lsn, &record);
            if let Some(change) = self.replay(lsn, record, &mut replaying, &mut report)? {
                changes.push(change, &mut encode_change)?;
            }
        }
        changes.visit_newest(&|record| decode_change(record, &types), &mut |change| {
            if !analysis.stands(change) && self.undo(change, &analysis)? {
                report.undone = report.undone.saturating_add(1);
            }
            Ok(())
        })?;
        changes.visit(&|record| decode_change(record, &types), &mut |change| {
            if analysis.stands(change) && self.redo(change, &analysis)? {
                report.redone = report.redone.saturating_add(1);
            }
            Ok(())
        })?;
        let committed = u64::try_from(analysis.committed.len()).unwrap_or(u64::MAX);
        report.committed = committed;
        report.not_committed = u64::try_from(analysis.transactions.len()).unwrap_or(u64::MAX).saturating_sub(committed);
        self.snapshot()?.tx.advance_past(analysis.highest);
        Ok((report, changes))
    }

    /// Replays a definition: one made outside transactions at once, one a
    /// transaction made at its commit record, if `replaying` says so;
    /// returns a row change, kept for undo and redo.
    fn replay(&self, lsn: Lsn, record: Record, replaying: &mut Replaying, report: &mut RecoveryReport) -> Result<Option<RowChange>> {
        let tx = record.tx;
        let change = |table, generation, row, before, after| Some(RowChange { lsn, tx, table, generation, row, before, after });
        Ok(match record.change {
            Change::Insert { table, generation, row, after } => change(table, generation, row, None, Some(after)),
            Change::Update { table, generation, row, before, after } => change(table, generation, row, Some(before), Some(after)),
            Change::Delete { table, generation, row, before } => change(table, generation, row, Some(before), None),
            definition @ (Change::Schema { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }) => {
                if replaying.replays(lsn) {
                    self.replay_definition(&definition)?;
                    report.definitions = report.definitions.saturating_add(1);
                }
                None
            }
            definition @ (Change::Drop { .. } | Change::AlterTable { .. } | Change::Defined { .. }) => {
                replaying.pending.entry(tx).or_default().push(definition);
                None
            }
            Change::Commit { .. } => {
                let made = replaying.pending.remove(&tx).unwrap_or_default();
                if replaying.replays(lsn) {
                    for definition in &made {
                        self.replay_definition(definition)?;
                        report.definitions = report.definitions.saturating_add(1);
                    }
                }
                None
            }
            Change::Abort => {
                replaying.pending.remove(&tx);
                None
            }
            Change::Stale { through, index, row, key } => {
                // Only the checkpoint reopened from's: one before it, or one
                // not recorded, saw garbage its indexes need not have.
                let from = match replaying.definitions {
                    Definitions::From(at) => Some(at),
                    Definitions::All => None,
                };
                if from == Some(through) {
                    self.take_back_stale(&index, row, &Key(key))?;
                }
                None
            }
            Change::StatementUndone { .. } | Change::Checkpoint => None,
        })
    }

    /// Takes back one entry of `row` under `key` in index `name`, if it is
    /// there: an entry of a version a checkpoint saw awaiting collection,
    /// which the crash lost. Before `mend_indexes`, which then sets right
    /// the rows recovery touched; an index to be built anew is built
    /// whatever it held.
    fn take_back_stale(&self, name: &str, row: RowId, key: &Key) -> Result<()> {
        let st = self.snapshot()?;
        let Ok(index) = st.index_of(name) else { return Ok(()) };
        if index.rows(key)?.contains(&row) {
            index.remove(key, row)?;
        }
        Ok(())
    }

    /// Makes the definition `change` again, as it was first made, without
    /// logging it; it is recorded among the definitions.
    pub(super) fn replay_definition(&self, change: &Change) -> Result<()> {
        self.change(|st| {
            match change {
                Change::Schema { sql, owner, engines } => {
                    let schema = crate::parser::Parser::new(sql)?.of_definition().schema()?.name;
                    for (ident, engine) in engines {
                        st.placement.insert(QualifiedName { auth: schema.clone(), ident: ident.clone() }, engine.clone());
                    }
                    st.schema(sql, Some(owner))?;
                }
                Change::CreateIndex { name, table, columns, method, options } => {
                    let spec = IndexSpec {
                        name: name.clone(),
                        table: table.clone(),
                        columns: columns.clone(),
                        method: method.clone(),
                        options: options.clone(),
                    };
                    st.create_index(&spec)?;
                }
                Change::AlterIndex { name, options } => st.replay_index_options(name, options.clone())?,
                Change::DropIndex { name } => st.drop_index(name)?,
                // As it was set: an engine registered later is found as the
                // setting is used, and a plug-in's setting is kept until it
                // is declared.
                Change::Setting { name, value } => st.settings.set(name, value.as_ref())?,
                Change::User { name, verifier } => st.users.replay_user(name, verifier.clone()),
                Change::Owner { name } => st.users.replay_owner(name),
                Change::Drop { .. } | Change::AlterTable { .. } | Change::Defined { .. } => st.apply_change(change, None)?.discard(),
                Change::Insert { .. }
                | Change::Update { .. }
                | Change::Delete { .. }
                | Change::Commit { .. }
                | Change::Abort
                | Change::Checkpoint
                | Change::Stale { .. }
                | Change::StatementUndone { .. } => return Err(SqlError::new(Code::Corrupt, "[DESIGN] a definition is a row change")),
            }
            Ok(())
        })
    }

    /// Redoes `change`, which stands, if its table's engine is recovered
    /// from the log and its pages lack it, or hold it unstamped; whether it
    /// did.
    fn redo(&self, change: &RowChange, analysis: &Analysis) -> Result<bool> {
        let st = self.snapshot()?;
        if !current(&st, change) {
            return Ok(false);
        }
        let storage = st.storage_of(&change.table)?;
        let durable = storage.durable_through();
        let committed_late = analysis.committed.get(&change.tx).is_some_and(|&at| at >= durable);
        let lacks = storage.capabilities().persistence == Persistence::Logged && (change.lsn >= durable || committed_late);
        if lacks {
            storage.apply(change.row, change.after.clone())?;
        }
        Ok(lacks)
    }

    /// Undoes `change`, which does not stand, if its table's engine may
    /// hold it without its undo; whether it did.
    fn undo(&self, change: &RowChange, analysis: &Analysis) -> Result<bool> {
        let st = self.snapshot()?;
        if !current(&st, change) {
            return Ok(false);
        }
        let storage = st.storage_of(&change.table)?;
        let undone_at = analysis.undone_at(change);
        let holds = match storage.capabilities().persistence {
            Persistence::Forced => undone_at.is_none(),
            Persistence::Logged => undone_at.is_none_or(|at| at >= storage.durable_through()),
            Persistence::External | Persistence::None => false,
        };
        if holds {
            storage.apply(change.row, change.before.clone())?;
        }
        Ok(holds)
    }
}

impl State {
    /// Rebuilds every index from its table's rows.
    pub(super) fn rebuild_indexes(&mut self) -> Result<()> {
        self.rebuild_indexes_where(|_| true)
    }

    /// Rebuilds from its table's rows every index `chosen` chooses.
    pub(super) fn rebuild_indexes_where(&mut self, chosen: impl Fn(&IndexDesc) -> bool) -> Result<()> {
        let indexes: Vec<IndexDesc> = self
            .catalog
            .tables()
            .into_iter()
            .flat_map(|t| self.catalog.indexes(&t.name).into_iter().cloned().collect::<Vec<_>>())
            .filter(|ix| chosen(ix))
            .collect();
        for ix in indexes {
            let method =
                self.methods.get(&ix.method).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no index method {}", ix.method)))?;
            let types = self.catalog.table(&ix.table).and_then(|t| t.types(&ix.columns));
            let types =
                types.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] index {} is on a column its table lacks", ix.name)))?;
            let index = method.create(&types)?;
            self.load_index(&*index, &ix.table, &ix.columns, Timestamp::MAX)?;
            self.indexes.insert(ix.name.clone(), Arc::from(index));
        }
        Ok(())
    }

    /// Makes every index `kept` chooses hold, for each row recovery
    /// touched, one entry, under its key now, and none under any key its
    /// images had: what recovery leaves of an index reopened as a
    /// checkpoint left it, which holds the entries of versions undone,
    /// redone or folded into one. Rows not touched keep their entries.
    /// Two passes over the changes, read a segment at a time: every entry
    /// of a row under the keys of a change's images taken back, which takes
    /// back its key now too, as a row recovery touched is as one of its
    /// images left it; then its entry under its key now made, once, by its
    /// first change.
    pub(super) fn mend_indexes(&self, touched: &mut Touched, kept: impl Fn(&IndexDesc) -> bool) -> Result<()> {
        let types = self.types.clone();
        touched.visit(&|record| decode_change(record, &types), &mut |change| self.take_back_images(change, &kept))?;
        touched.visit(&|record| decode_change(record, &types), &mut |change| self.restore_entry(change, &kept))
    }

    /// Takes back every entry of the row `change` touched under the keys
    /// of its images, in each index `kept` chooses.
    fn take_back_images(&self, change: &RowChange, kept: &impl Fn(&IndexDesc) -> bool) -> Result<()> {
        // A dropped table has no indexes left to mend (`Database::redo`).
        for ix in self.catalog.indexes(&change.table).into_iter().filter(|ix| kept(ix)) {
            let index = self.index_of(&ix.name)?;
            for key in change.before.iter().chain(&change.after).filter_map(|r| key_of(r, &ix.columns)) {
                take_back(index, &key, change.row)?;
            }
        }
        Ok(())
    }

    /// Makes the entry of the row `change` touched under its key now, in
    /// each index `kept` chooses, unless there.
    fn restore_entry(&self, change: &RowChange, kept: &impl Fn(&IndexDesc) -> bool) -> Result<()> {
        if self.catalog.table(&change.table).is_none() {
            return Ok(());
        }
        let Some(now) = self.storage_of(&change.table)?.get(NO_TRANSACTION, change.row)? else { return Ok(()) };
        for ix in self.catalog.indexes(&change.table).into_iter().filter(|ix| kept(ix)) {
            let index = self.index_of(&ix.name)?;
            if let Some(key) = key_of(&now, &ix.columns).filter(|k| index.rows(k).is_ok_and(|ids| !ids.contains(&change.row))) {
                index.insert(key, change.row)?;
            }
        }
        Ok(())
    }

    /// Adds to `index` an entry for each version of each row of `table`
    /// readers may still see, keyed by `columns`; keys with a null are left
    /// out (`Index::insert`). One by one, as writers may add to it too.
    pub(super) fn fill_index(&self, index: &dyn Index, table: &QualifiedName, columns: &[usize], as_of: Timestamp) -> Result<()> {
        for entry in self.entries_for(table, columns, as_of)? {
            let (key, id) = entry?;
            index.insert(key, id)?;
        }
        Ok(())
    }

    /// As `fill_index`, into `index`, new, which nothing else reads or
    /// writes meanwhile: loaded whole (`Index::load`), in a work area of
    /// the database's WORK_MEMORY.
    pub(super) fn load_index(&self, index: &dyn Index, table: &QualifiedName, columns: &[usize], as_of: Timestamp) -> Result<()> {
        use crate::settings::{SPILL_BATCH, WORK_MEMORY, batch_bytes, work_bytes};
        let area = Area::new(work_bytes(self.settings.get(WORK_MEMORY)), batch_bytes(self.settings.get(SPILL_BATCH)));
        index.load(&mut self.entries_for(table, columns, as_of)?, (&self.work, area))
    }

    /// The entries an index on `columns` of `table` holds, of each version
    /// readers may still see: keys with a null left out.
    fn entries_for<'a>(
        &self,
        table: &'a QualifiedName,
        columns: &'a [usize],
        as_of: Timestamp,
    ) -> Result<impl Iterator<Item = Result<(Key, RowId)>> + 'a> {
        let versions = Arc::clone(self.storage_of(table)?).versions(self.tx.horizon(), as_of, columns);
        Ok(versions.filter_map(move |read| {
            let keyed = read.and_then(|(id, row)| {
                let key = project(&row, columns)
                    .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a row of {table} is shorter than an index")))?;
                Ok((Key(key), id))
            });
            match keyed {
                Ok((key, id)) => (!key.0.iter().any(Value::is_null)).then_some(Ok((key, id))),
                Err(e) => Some(Err(e)),
            }
        }))
    }
}
