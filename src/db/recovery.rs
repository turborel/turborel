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
//!    before L, is not yet stamped committed; of a `Forced` table, every
//!    one, as a transaction in doubt (RULES.md D25) logged its commit
//!    record but never told the engine it committed.
//! 4. The indexes made whole, as index entries are not logged: rebuilt from
//!    their tables, or, for indexes reopened as the checkpoint left them,
//!    mended where recovery touched rows (`State::mend_indexes`); the
//!    transaction counter moved past every id and timestamp read.
//!
//! The row changes read are kept in a list that spills past MEMORY.WORK_BYTES
//! (`recovery/changes.rs`, RULES.md D38), read a segment at a time by each
//! pass, so recovery holds a work area's bytes of them however much of the
//! log it reads.
//!
//! A row is written by one transaction at a time, until it ends, so undoing
//! then redoing in log order leaves each row as the last change that stands
//! left it. Changes are applied with `TableStorage::apply`, which is
//! idempotent, so recovering again after a crash during recovery gives the
//! same result.

mod analysis;
mod changes;
mod stale;

use std::collections::HashMap;
use std::sync::Arc;

use analysis::Analysis;
pub(super) use changes::Touched;
use changes::{RowChange, decode_change, encode_change};

use super::indexes::IndexSpec;
use super::{Database, State};
use crate::catalog::IndexDesc;
use crate::disk::encoding::Encodings;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::{Change, Record};
use crate::log::{Log, Lsn};
use crate::spill::Area;
use crate::storage::{Row, RowId};
use crate::txn::Timestamp;
use crate::txn::{NO_TRANSACTION, Persistence, TxId};
use crate::value::types::TypeRegistry;

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
/// those each transaction made, each with its place in the log, until its
/// commit record makes them, or its abort record forgets them; and the
/// large objects of a transaction that made definitions, kept until them,
/// as the table of one may be among them (RULES.md D79).
struct Replaying {
    definitions: Definitions,
    pending: HashMap<TxId, Vec<(Lsn, Change)>>,
    lobs: HashMap<TxId, Vec<(Lsn, Change)>>,
}

impl Replaying {
    /// [ISO/IEC 9075-2:2023 §17.8 GR3d] Forgets the definitions `tx` made
    /// from `from` on: a failed statement, or a rollback to a savepoint,
    /// cancelled them.
    fn cancel(&mut self, tx: TxId, from: Lsn) {
        if let Some(made) = self.pending.get_mut(&tx) {
            made.retain(|(at, _)| *at < from);
        }
    }

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
    /// Transactions that changed rows or definitions and committed.
    pub committed: u64,
    /// Transactions that changed rows or definitions and did not commit:
    /// rolled back, or interrupted after changing rows.
    pub not_committed: u64,
    /// Row changes redone.
    pub redone: u64,
    /// Row changes undone.
    pub undone: u64,
    /// Bytes cut from the end of the log as it opened: what writes a crash
    /// interrupted left, of records never synced, so of no transaction
    /// committed (`OpenedLog::cut`).
    pub cut: u64,
}

/// The most entries of one row under one key mending takes back: a bound
/// on a loop that should take back a few.
const MOST_ENTRIES: usize = 1_024;

/// The key of `row`'s entry in the index `ix`, if it holds one
/// (`IndexDesc::entry_of`).
fn key_of(row: &Row, ix: &IndexDesc) -> Option<Key> {
    ix.entry_of(row)
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
        let (report, _) = self.replay(&mut log.read_from(Lsn(0)), Definitions::All)?;
        self.change(State::rebuild_indexes)?;
        self.snapshot()?.prune_counters()?;
        self.set_log(log)?;
        self.apply_settings()?;
        Ok(report)
    }

    /// \[DESIGN\] The one replay of the database (RULES.md D57): applies
    /// `records`, each a position and the bytes a `Log` holds there, opaque
    /// to whoever reads them, in position order: analysis, definitions
    /// replayed as `definitions` says, then undo and redo; the counters
    /// moved past the ids and timestamps read. What it did, and the rows it
    /// read changes of. Recovery is this replay of its log from a
    /// checkpoint's start, or from the log's; whatever else follows a log
    /// (a standby) runs it too, so a kind of record recovery handles needs
    /// nothing more. Each kind is matched without a wildcard, in
    /// `replay_record`, `replay_definition` and `Analysis::note`, so a kind
    /// added does not compile until it is replayed.
    pub(super) fn replay(
        &self,
        records: &mut dyn Iterator<Item = Result<(Lsn, Vec<u8>)>>,
        definitions: Definitions,
    ) -> Result<(RecoveryReport, Touched)> {
        let (encodings, types) = self.openers()?;
        let mut opened = records.map(|read| read.and_then(|(lsn, bytes)| Ok((lsn, crate::log::sealed::open(&bytes, &encodings, &types)?))));
        self.replay_records(&mut opened, definitions)
    }

    /// What opens a log's records: the database's encodings, of sealed
    /// ones (`log/sealed.rs`), and its types.
    pub(super) fn openers(&self) -> Result<(Arc<Encodings>, TypeRegistry)> {
        let st = self.snapshot()?;
        Ok((Arc::clone(&st.encodings), st.types.clone()))
    }

    /// `replay`, of records already opened.
    pub(super) fn replay_records(
        &self,
        records: &mut dyn Iterator<Item = Result<(Lsn, Record)>>,
        definitions: Definitions,
    ) -> Result<(RecoveryReport, Touched)> {
        let mut report = RecoveryReport::default();
        let mut analysis = Analysis::default();
        let mut changes = Touched::new(self.snapshot()?.list_space());
        let mut replaying = Replaying { definitions, pending: HashMap::new(), lobs: HashMap::new() };
        let types = self.snapshot()?.types.clone();
        for read in records {
            let (lsn, record) = read?;
            report.records = report.records.saturating_add(1);
            analysis.note(lsn, &record);
            for change in self.replay_record(lsn, record, &mut replaying, &mut report)? {
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
        // The large objects no row names now, of transactions that did not
        // commit or of versions replaced, freed (RULES.md D79).
        for storage in self.snapshot()?.storage.values() {
            storage.recovered()?;
        }
        let committed = u64::try_from(analysis.committed.len()).unwrap_or(u64::MAX);
        report.committed = committed;
        report.not_committed = u64::try_from(analysis.transactions.len()).unwrap_or(u64::MAX).saturating_sub(committed);
        self.snapshot()?.tx.advance_past(analysis.highest);
        Ok((report, changes))
    }

    /// Replays a definition: one made outside transactions at once, one a
    /// transaction made at its commit record, if `replaying` says so;
    /// returns its row changes, kept for undo and redo: \[EXTENSION\] of
    /// rows inserted together, one for each (RULES.md D530).
    fn replay_record(&self, lsn: Lsn, record: Record, replaying: &mut Replaying, report: &mut RecoveryReport) -> Result<Vec<RowChange>> {
        let tx = record.tx;
        let one = |table, generation, row, before, after| RowChange { lsn, tx, table, generation, row, before, after };
        let change = |table, generation, row, before, after| Some(one(table, generation, row, before, after));
        Ok(Vec::from_iter(match record.change {
            Change::Insert { table, generation, row, after } => change(table, generation, row, None, Some(after)),
            Change::InsertRows { table, generation, rows } => {
                return Ok(rows.into_iter().map(|(row, after)| one(table.clone(), generation, row, None, Some(after))).collect());
            }
            Change::Update { table, generation, row, before, after } => change(table, generation, row, Some(before), Some(after)),
            Change::Delete { table, generation, row, before } => change(table, generation, row, Some(before), None),
            definition @ (Change::Schema { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }
            | Change::RoleCreator { .. }
            | Change::Sequence { .. }
            | Change::Reconfiguration { .. }) => {
                self.definition_at((lsn, tx), definition, replaying, report)?;
                None
            }
            definition @ (Change::Drop { .. } | Change::AlterTable { .. } | Change::Defined { .. } | Change::Spatial(_)) => {
                replaying.pending.entry(tx).or_default().push((lsn, definition));
                None
            }
            Change::Commit { .. } => {
                self.committed((lsn, tx), replaying, report)?;
                None
            }
            Change::Abort => {
                replaying.pending.remove(&tx);
                replaying.lobs.remove(&tx);
                None
            }
            kept @ (Change::Stale { .. } | Change::LobStale { .. } | Change::LobPut { .. }) => {
                self.kept_at((lsn, tx), kept, replaying)?;
                None
            }
            Change::StatementUndone { from } => {
                replaying.cancel(tx, from);
                None
            }
            Change::Checkpoint | Change::RestorePoint { .. } => None,
        }))
    }

    /// [ISO/IEC 9075:1992 §20.1 GR6b ii] The definitions `tx`, committed at
    /// `lsn`, made, if replayed; then its large objects (`kept_at`).
    fn committed(&self, (lsn, tx): (Lsn, TxId), replaying: &mut Replaying, report: &mut RecoveryReport) -> Result<()> {
        let made = replaying.pending.remove(&tx).unwrap_or_default();
        if replaying.replays(lsn) {
            for (_, definition) in &made {
                self.replay_definition(definition)?;
                report.definitions = report.definitions.saturating_add(1);
            }
        }
        for (at, kept) in replaying.lobs.remove(&tx).unwrap_or_default() {
            self.replay_kept(at, kept, &replaying.definitions)?;
        }
        Ok(())
    }

    /// The definition `definition`, logged at `lsn` by `tx`: made now, if
    /// replayed; but a session's `<schema definition>`, of its transaction,
    /// as that commits [ISO/IEC 9075:1992 §20.1 GR6b ii].
    fn definition_at(
        &self,
        (lsn, tx): (Lsn, TxId),
        definition: Change,
        replaying: &mut Replaying,
        report: &mut RecoveryReport,
    ) -> Result<()> {
        if tx != NO_TRANSACTION && matches!(definition, Change::Schema { .. }) {
            replaying.pending.entry(tx).or_default().push((lsn, definition));
        } else if replaying.replays(lsn) {
            self.replay_definition(&definition)?;
            report.definitions = report.definitions.saturating_add(1);
        }
        Ok(())
    }

    /// Makes the definition `change` again, as it was first made, without
    /// logging it; it is recorded among the definitions.
    pub(super) fn replay_definition(&self, change: &Change) -> Result<()> {
        self.change(|st| {
            match change {
                Change::Schema { sql, owner, engines, now, epochs, typed } => {
                    let _typed = crate::binder::making_typed(*typed);
                    st.reschema((sql, owner, *now), engines, None, epochs)?.discard();
                }
                Change::CreateIndex { name, table, columns, method, options, expressions } => {
                    let spec = IndexSpec {
                        name: name.clone(),
                        table: table.clone(),
                        columns: columns.clone(),
                        method: method.clone(),
                        options: options.clone(),
                        nulls: false,
                        expressions: expressions.clone(),
                    };
                    st.create_index(&spec)?;
                }
                Change::AlterIndex { name, options } => st.replay_index_options(name, options.clone())?,
                Change::DropIndex { name } => st.drop_index(name)?,
                // As it was set: an engine registered later is found as the setting is used,
                // and a plug-in's setting is kept until it is declared.
                Change::Setting { name, value } => st.settings.set(name, value.as_ref())?,
                Change::User { name, verifier } => st.users.replay_user(name, verifier.clone()),
                Change::Owner { name } => st.users.replay_owner(name),
                Change::RoleCreator { name, allowed } => st.users.replay_creator(name, *allowed),
                // The group's, not the database's: the replica knows it.
                Change::Reconfiguration { .. } => {}
                // [DESIGN] RULES.md D62: its counter goes on from there.
                Change::Sequence { name, epoch, resume } => st.sequences.replay(name, *epoch, *resume)?,
                Change::Drop { .. } | Change::AlterTable { .. } | Change::Defined { .. } | Change::Spatial(_) => {
                    st.apply_change(change, None)?.discard();
                }
                Change::Insert { .. }
                | Change::InsertRows { .. }
                | Change::Update { .. }
                | Change::Delete { .. }
                | Change::Commit { .. }
                | Change::Abort
                | Change::Checkpoint
                | Change::RestorePoint { .. }
                | Change::Stale { .. }
                | Change::LobPut { .. }
                | Change::LobStale { .. }
                | Change::StatementUndone { .. } => return Err(SqlError::new(Code::Corrupt, "[DESIGN] a definition is a row change")),
            }
            Ok(())
        })
    }

    /// Redoes `change`, which stands, if its table's engine is recovered
    /// from the log and its pages lack it, or hold it unstamped, or makes
    /// its own writes durable but may not have been told it committed;
    /// whether it did.
    fn redo(&self, change: &RowChange, analysis: &Analysis) -> Result<bool> {
        let st = self.snapshot()?;
        if !current(&st, change) {
            return Ok(false);
        }
        let storage = st.storage_of(&change.table)?;
        let durable = storage.durable_through();
        let committed_late = analysis.committed.get(&change.tx).is_some_and(|&at| at >= durable);
        let lacks = match storage.capabilities().persistence {
            Persistence::Logged => change.lsn >= durable || committed_late,
            // Its commit may have been in doubt, never told the engine.
            Persistence::Forced => true,
            Persistence::External | Persistence::None => false,
        };
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
            let found = self.catalog.table(&ix.table).and_then(|t| ix.key_types(t).map(|types| (t, types)));
            let (table, types) =
                found.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] index {} is on a column its table lacks", ix.name)))?;
            // Made as CREATE INDEX made it, its options and its table's
            // read again (RULES.md D41).
            let index = method.create_on(&types, (table, &ix))?;
            self.load_index(&*index, &ix, Timestamp::MAX)?;
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
        for (ix, partition) in self.table_indexes(&change.table).into_iter().filter(|(ix, _)| kept(ix)) {
            let (index, id) = (self.index_of(&ix.name)?, Self::entry_id(partition, change.row)?);
            for key in change.before.iter().chain(&change.after).filter_map(|r| key_of(r, &ix)) {
                take_back(&**index, &key, id)?;
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
        for (ix, partition) in self.table_indexes(&change.table).into_iter().filter(|(ix, _)| kept(ix)) {
            let (index, id) = (self.index_of(&ix.name)?, Self::entry_id(partition, change.row)?);
            if let Some(key) = key_of(&now, &ix).filter(|k| index.rows(k).is_ok_and(|ids| !ids.contains(&id))) {
                index.insert(key, id)?;
            }
        }
        Ok(())
    }

    /// Adds to `index` an entry for each version of each row of `table`
    /// readers may still see, keyed as `ix` says; keys with a null are left
    /// out (`Index::insert`). As writers may add to it too
    /// (`Index::load_beside_writers`).
    pub(super) fn fill_index(&self, index: &dyn Index, ix: &IndexDesc, as_of: Timestamp) -> Result<()> {
        index.load_beside_writers(&mut self.entries_for(ix, as_of)?, (&self.work, self.load_area()))
    }

    /// As `fill_index`, into `index`, new, which nothing else reads or
    /// writes meanwhile: loaded whole (`Index::load`).
    pub(super) fn load_index(&self, index: &dyn Index, ix: &IndexDesc, as_of: Timestamp) -> Result<()> {
        index.load(&mut self.entries_for(ix, as_of)?, (&self.work, self.load_area()))
    }

    /// MEMORY.WORK_BYTES in batches of SPILL.BATCH_BYTES, for a load.
    pub(super) fn load_area(&self) -> Area {
        use crate::settings::{MEMORY_WORK_BYTES, SPILL_BATCH_BYTES, batch_bytes, held_bytes};
        Area::new(held_bytes(self.settings.get(MEMORY_WORK_BYTES)), batch_bytes(self.settings.get(SPILL_BATCH_BYTES)))
    }

    /// The entries the index `ix` holds, of each version
    /// readers may still see: keys the index does not hold left out
    /// (`IndexDesc::holds`).
    fn entries_for<'a>(&self, ix: &'a IndexDesc, as_of: Timestamp) -> Result<impl Iterator<Item = Result<(Key, RowId)>> + 'a> {
        // [EXTENSION] a global index's, of its table's partitions (RULES.md D157).
        let versions = if crate::catalog::is_global_index(&ix.name) {
            self.global_versions(ix, as_of)?
        } else {
            // [DESIGN] RULES.md D443: every version it counts shares no
            // entry; with no snapshot given, the table's writers are kept
            // out, and every commit given is counted.
            let through = if as_of == Timestamp::MAX { self.tx.highest() } else { as_of };
            if let Ok(lock) = self.lock_of(&ix.table) {
                lock.filled(&ix.name, through);
            }
            Arc::clone(self.storage_of(&ix.table)?).versions(self.tx.horizon(), as_of, &ix.reads())
        };
        Ok(versions.filter_map(move |read| {
            let keyed = read.and_then(|(id, row)| {
                let key = ix
                    .key_of(&row)
                    .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a row of {} is shorter than an index", ix.table)))?;
                Ok((key, id))
            });
            match keyed {
                Ok((key, id)) => ix.holds(&key).then_some(Ok((key, id))),
                Err(e) => Some(Err(e)),
            }
        }))
    }
}
