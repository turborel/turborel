//! The ends of transactions [X3.135-1986 §8.2, §8.9]: commit, two-phase
//! across the participants at one timestamp, or at the snapshot for a
//! transaction that wrote nothing; rollback; and the garbage collection of
//! versions that follows either (RULES.md D25, D29).

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::kept::{key_reads, needs_row};
use super::owed::{Entry, Owed};
use super::{Committing, Joined, Sharing, TableLock, Timestamp, Transactions, Txn, UndoFailure};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::index::{Index, Key};
use crate::memory::Undoing;
use crate::storage::{Kept, Needed, Row, RowId, TableStorage};
use crate::sync::lock_builtin;

impl State {
    /// [X3.135-1986 §8.2 GR1, GR3] terminate the transaction and commit its
    /// changes: two-phase across the participants, at one commit timestamp.
    /// Between the phases its commit record is written; its locks are
    /// released, and then the record is synced, the moment it commits
    /// (RULES.md D25, D441). It returns once its commit is durable, so its
    /// session's next snapshot counts it. If a participant refuses, or the
    /// commit record cannot be written, the whole transaction is rolled back
    /// instead.
    ///
    /// # Errors
    ///
    /// As for rollback, and SQLCODE -922 if the commit record was written
    /// but it, or one before it, could not be synced: the transaction is in
    /// doubt, never durable, so no snapshot counts it, for the caller to
    /// keep until the database is dropped; recovery commits it if the log
    /// holds its commit record (RULES.md D25).
    pub(crate) fn commit(&self, tx: &Txn) -> Result<()> {
        self.commit_publishing(tx, &mut |_| Ok(()))
    }

    /// `commit`, running `publish` with the commit timestamp once the
    /// transaction has committed and
    /// before any snapshot counts it, still under the gate: so its
    /// definitions are published with its rows (`db/ddl.rs`). It is not run
    /// if the transaction rolls back instead.
    ///
    /// # Errors
    ///
    /// As for `commit`; and the error of `publish`, the transaction having
    /// committed.
    pub(crate) fn commit_publishing(&self, tx: &Txn, publish: &mut dyn FnMut(Timestamp) -> Result<()>) -> Result<()> {
        // A load under way is ended before it commits (RULES.md D532).
        self.end_load(tx)?;
        let (joined, wrote) = {
            let mut st = tx.state()?;
            // One that wrote keeps its participants, for its rollback if the commit fails; one that read alone
            // is done with them.
            let joined = if st.wrote { st.joined.clone() } else { mem::take(&mut st.joined) };
            (joined, st.wrote)
        };
        if !wrote {
            return self.commit_read(tx, &joined);
        }
        // The commit record and the versions stamped are one change to a
        // checkpoint.
        let gate = self.tx.gate.enter();
        // Under the gate: an index published shut is either seen here, or
        // published after this commit, which it then counts.
        let validated =
            tx.check_definitions().and_then(|()| tx.check_undone()).and_then(|()| self.validate_participants(tx, &joined)).and_then(
                |(participants, mut committing)| {
                    let now = self.clock.now();
                    let lsn = tx.append_commit(committing.at, now)?;
                    if lsn.is_some() {
                        committing.syncs();
                    }
                    Ok((participants, committing, (lsn, now)))
                },
            );
        match validated {
            Ok((participants, committing, (lsn, now))) => {
                // Phase 2 uses storages already found, so it cannot fail part
                // way. Each engine makes the changes committed at once, as
                // the row store does with one store (RULES.md D440).
                for storage in &participants {
                    storage.commit(tx.id, committing.at);
                }
                // When each table it wrote last changed (RULES.md D522).
                tx.stamp_written(now);
                let at = committing.at;
                let published = publish(at);
                // The commit has reached every engine: it has ended, and is
                // durable once its record is synced.
                drop(committing);
                // [DESIGN] RULES.md D441: its locks go before the sync, so
                // the next writer of its rows goes on at once.
                tx.release();
                // [DESIGN] RULES.md D440: written in place once its locks
                // are released, and before the gate lets a checkpoint
                // capture the pages, whose record syncs this one's.
                for storage in participants {
                    storage.settle(tx.id);
                }
                drop(gate);
                let durable = self.durable(tx, lsn, at);
                self.collect_ended(tx, &joined);
                durable.and(published)
            }
            Err(e) => {
                drop(gate);
                self.rollback(tx)?;
                // The transaction was rolled back: a conflict is a
                // serialization failure [ISO/IEC 9075:1992 §22.1].
                tx.replied(Err(if e.code == Code::Conflict { e.with_state(SqlState::SERIALIZATION_FAILURE) } else { e }))
            }
        }
    }

    /// \[DESIGN\] The commit of `tx`, which changed nothing, so it read one
    /// committed state, its snapshot, and is serializable there: no engine
    /// need check its reads, and it takes no commit timestamp (RULES.md
    /// D29).
    fn commit_read(&self, tx: &Txn, joined: &[Joined]) -> Result<()> {
        for participant in joined {
            participant.storage.commit(tx.id, tx.snapshot());
        }
        tx.release();
        if !tx.stop.collects_later() {
            self.gc_read();
        }
        // [DESIGN] RULES.md D441: what it read past its snapshot is durable
        // before it says it committed.
        tx.replied(Ok(()))
    }

    /// \[DESIGN\] Syncs the commit record of `tx`, committed at `at`, at
    /// `lsn` if it has one to sync, and returns once every commit up to `at`
    /// is durable: not before every commit before it is, so its session's
    /// next snapshot counts it (RULES.md D29, D441).
    ///
    /// # Errors
    ///
    /// SQLCODE -922 if its sync, or one before, failed: it is in doubt.
    fn durable(&self, tx: &Txn, lsn: Option<crate::log::Lsn>, at: Timestamp) -> Result<()> {
        if let Some(lsn) = lsn {
            // Marked durable by its group's sync (`txn/group.rs`).
            tx.sync_commit(lsn, at)?;
        }
        self.tx.wait_durable(at)
    }

    /// Phase 1: takes the commit timestamp, and asks every participant in
    /// `joined` to validate at it. Nothing has committed if this fails.
    fn validate_participants<'j>(&self, tx: &Txn, joined: &'j [Joined]) -> Result<(Vec<&'j dyn TableStorage>, Committing<'_>)> {
        let committing = self.tx.commit_timestamp();
        let at = committing.at;
        for Joined { table, storage } in joined {
            if let Err(e) = storage.validate(tx.id, at) {
                // [DESIGN] RULES.md D441: the refusal rests on commits
                // before it, told once they are durable.
                if e.code == Code::Conflict {
                    tx.depend(at.saturating_sub(1));
                }
                // A conflict keeps its code, so the program can tell it to
                // run the transaction again (RULES.md D29).
                let code = if e.code == Code::Conflict { Code::Conflict } else { Code::CommitFailed };
                return Err(SqlError::new(
                    code,
                    format!(
                        "[X3.135-1986 §8.2] the engine of {table} refused to commit, so the transaction was rolled back: {}",
                        e.described()
                    ),
                ));
            }
        }
        Ok((joined.iter().map(|j| &*j.storage).collect(), committing))
    }

    /// After a transaction ends, garbage collection: each of `tables`, and
    /// each table whose versions still awaited collection when it was last
    /// collected, removes the versions no open transaction can read, and
    /// their index entries go with them (RULES.md D29). The horizon moves
    /// only as a transaction ends, so a table no transaction touches is
    /// collected all the same, as soon as it can be. A failure leaves them
    /// for the next transaction to end: the one that ended has already
    /// committed or rolled back. A temporary table's instances are each
    /// collected by their session instead (`db/temporary.rs`). `tables`
    /// names each once; one still awaited is collected once too.
    pub(crate) fn gc<'t>(&self, tables: impl Iterator<Item = &'t QualifiedName> + Clone) {
        self.gc_with(tables, false);
    }

    /// \[DESIGN\] As `gc`, of the tables of `joined`, after `tx` ended;
    /// unless its session collects once its reply is sent (RULES.md D450):
    /// then they are noted, for that collection, which collects them itself
    /// (D453), or the end of any other transaction to collect.
    fn collect_ended(&self, tx: &Txn, joined: &[Joined]) {
        let tables = joined.iter().map(|j| &*j.table);
        if tx.stop.collects_later() {
            tx.stop.owe(joined.iter().map(|j| Arc::clone(&j.table)));
            // [DESIGN] RULES.md D795: a table noted already is left so,
            // with nothing shared written.
            let new: Vec<&QualifiedName> = tables.filter(|t| !self.lock_of(t).is_ok_and(|lock| lock.noted())).collect();
            if new.is_empty() {
                return;
            }
            let mut uncollected = lock_builtin(&self.tx.uncollected);
            for table in new {
                if let Ok(lock) = self.lock_of(table) {
                    lock.set_noted(true);
                }
                uncollected.insert(table.clone());
            }
            self.awaiting(&uncollected);
        } else {
            self.gc(tables);
        }
    }

    /// Whether tables await collection, as `uncollected` says: stored only
    /// when that changes (RULES.md D795).
    fn awaiting(&self, uncollected: &BTreeSet<QualifiedName>) {
        let now = !uncollected.is_empty();
        if self.tx.awaiting.load(Ordering::Relaxed) != now {
            self.tx.awaiting.store(now, Ordering::Release);
        }
    }

    /// As `gc`, but waiting for a collection of a table under way rather
    /// than passing it over (RULES.md D445): for a checkpoint that needs
    /// every table collected as far as the horizon lets.
    pub(crate) fn gc_waiting<'t>(&self, tables: impl Iterator<Item = &'t QualifiedName> + Clone) {
        self.gc_with(tables, true);
    }

    fn gc_with<'t>(&self, tables: impl Iterator<Item = &'t QualifiedName> + Clone, wait: bool) {
        let _gate = self.tx.gate.enter();
        let horizon = self.tx.horizon();
        // [DESIGN] RULES.md D795: the set is read only while it holds any.
        let waiting: Vec<QualifiedName> = if self.tx.awaiting.load(Ordering::Acquire) {
            lock_builtin(&self.tx.uncollected).iter().cloned().collect()
        } else {
            Vec::new()
        };
        for table in waiting.iter().filter(|w| !tables.clone().any(|t| t == *w)) {
            self.collect_noting(table, horizon, wait);
        }
        for table in tables {
            self.collect_noting(table, horizon, wait);
        }
    }

    /// \[DESIGN\] Collects `tables`, those a served session's transactions
    /// wrote since its last collection, as their own ends would have
    /// (RULES.md D450, D453): only those, so the set of tables awaiting
    /// collection is not read, and each at once, as the session's other
    /// collection, of that set, may be under way in another session.
    pub(crate) fn gc_owed(&self, tables: &[Arc<QualifiedName>]) {
        let _gate = self.tx.gate.enter();
        let horizon = self.tx.horizon();
        for table in tables {
            self.collect_noting(table, horizon, false);
        }
    }

    /// Collects `table` at `horizon` (`collect_table`), and notes in the
    /// set of tables awaiting collection whether it still does.
    fn collect_noting(&self, table: &QualifiedName, horizon: Timestamp, wait: bool) {
        let left = self.collect_table(table, horizon, wait);
        let lock = self.lock_of(table).ok();
        if unchanged(left, lock.map(|lock| lock.noted())) {
            return;
        }
        let mut uncollected = lock_builtin(&self.tx.uncollected);
        // Asked again under the lock: a commit that added garbage while
        // this one collected, and was passed over (RULES.md D445), has
        // noted the table, and its note must not be taken out.
        let still = || self.storage_of(table).is_ok_and(|s| s.uncollected());
        let keep = match left {
            Some(true) => true,
            Some(false) => still(),
            None => false,
        };
        if keep {
            uncollected.insert(table.clone());
        } else {
            uncollected.remove(table);
        }
        if let Some(lock) = lock {
            lock.set_noted(keep);
        }
        self.awaiting(&uncollected);
    }

    /// \[DESIGN\] After a transaction that wrote nothing ends, garbage
    /// collection as `gc`'s, of the tables awaiting it alone (RULES.md D29):
    /// a transaction makes garbage only as it commits or rolls back changes,
    /// collecting its tables then, so the tables this one read hold none it
    /// left. Its end may move the horizon, so those awaiting are collected,
    /// and none is while none awaits, nor is the gate taken. It notes no
    /// maintenance: a table's is noted as a writer collects it, and noted
    /// again after each round while still due (`db/maintenance.rs`).
    ///
    /// \[DESIGN\] One such end collects at a time (RULES.md D453): another
    /// ending meanwhile passes by, as the one collecting collects the same
    /// tables, and any later end collects what it leaves.
    pub(crate) fn gc_read(&self) {
        if self.tx.awaiting.load(Ordering::Acquire) && !self.tx.read_collecting.swap(true, Ordering::AcqRel) {
            self.gc(std::iter::empty());
            self.tx.read_collecting.store(false, Ordering::Release);
        }
    }

    /// Collects `table`'s garbage past `horizon`: whether versions of it
    /// still await collection, or none if it is not there, or is a
    /// temporary table: an instance of one is collected by its session as
    /// its transactions end (`db/temporary.rs`), the definition's storage
    /// holding nothing.
    fn collect_table(&self, table: &QualifiedName, horizon: Timestamp, wait: bool) -> Option<bool> {
        if self.catalog.table(table).is_none_or(|t| t.temporary.is_some()) {
            return None;
        }
        // Not while an index of it is built, which counts every version it
        // finds; nor with definitions older than its indexes, whose entries
        // for the versions removed would stay. A later transaction collects
        // them.
        let lock = self.lock_of(table).ok()?;
        let storage = self.storage_of(table).ok()?;
        if lock.collection_held() || lock.changed_after(self.generation) {
            return Some(true);
        }
        // One collector at a time (RULES.md D445): another ending now finds
        // the work under way and passes the table over, noted as awaiting.
        // Nor, at a transaction's end, before it is worth its cost (RULES.md
        // D451); a checkpoint's collection, which waits, takes everything.
        if !wait && !storage.collection_due(horizon) {
            return Some(true);
        }
        let Some(_collecting) = lock.try_collect(wait) else { return Some(true) };
        self.collect_versions(table, (storage, lock), horizon);
        if storage.maintenance_due() {
            self.tx.note_maintenance(table);
        }
        Some(storage.uncollected())
    }

    /// Removes the versions of `table` no reader at or after `horizon`
    /// reads, through `storage`, each with its index entries; one not taken
    /// back is owed on `lock` (`txn/owed.rs`).
    fn collect_versions(&self, table: &QualifiedName, (storage, lock): (&Arc<dyn TableStorage>, &TableLock), horizon: Timestamp) {
        // The table's indexes, found as the first version is removed:
        // most calls remove none, so most never look them up.
        let found: OnceCell<Vec<Found<'_>>> = OnceCell::new();
        let indexes: OnceCell<Vec<(&dyn Index, &IndexDesc, Sharing)>> = OnceCell::new();
        let of_table = || {
            self.catalog.indexes(table).into_iter().filter_map(|ix| Some((ix.name.as_str(), self.index_of(&ix.name).ok()?, ix))).collect()
        };
        // The entries owed first; then each version's go as it is
        // removed, an entry not taken back owed (`txn/owed.rs`). A
        // failure leaves the rest of the garbage for a later call.
        lock.owed.retry(&|name, index| self.index_of(name).is_ok_and(|now| Arc::ptr_eq(now, index)));
        // [EXTENSION] The global indexes over a partition's rows, whose
        // entries go with its versions; one not taken back is passed
        // over by every reader, which reads its row again (RULES.md D157).
        let globals: OnceCell<Vec<Global>> = OnceCell::new();
        let over = || {
            let all = self.globals_over(table).into_iter();
            all.filter_map(|(ix, partition)| Some((Arc::clone(self.index_of(&ix.name).ok()?), ix, partition))).collect()
        };
        // A version whose every entry its successor shares is not read
        // (RULES.md D750): none of a partition with global indexes, whose
        // entries are each version's own.
        let sharing = || found.get_or_init(of_table).iter().map(|&(_, index, ix)| (&**index, ix, Sharing::of(ix, lock))).collect();
        let needs = |kept: Kept| {
            needs_row(!globals.get_or_init(over).is_empty(), indexes.get_or_init(sharing).iter().map(|&(_, _, shared)| shared), kept)
        };
        // Of a version read, only the columns its entries' keys read (RULES.md D751).
        let reads: OnceCell<Vec<usize>> = OnceCell::new();
        let local = || found.get_or_init(of_table).iter().map(|&(_, _, ix)| ix);
        let read = || key_reads(local().chain(globals.get_or_init(over).iter().map(|(_, ix, _)| ix)));
        let columns = || Some(reads.get_or_init(read).as_slice());
        let removed = |id, row: Row, kept: Kept| {
            remove_global(globals.get_or_init(over), id, &row);
            let found = found.get_or_init(of_table);
            let entries = owed_entries(found, remove_entries(indexes.get_or_init(sharing), id, &row, kept));
            let gone = entries.is_empty();
            if !gone {
                lock.owed.add(Owed { row: id, storage: Arc::clone(storage), entries });
            }
            gone
        };
        let _ = storage.gc(horizon, &mut Needed { needs, columns: &columns, removed });
    }

    /// [X3.135-1986 §8.9 GR1, GR3] cancel every change and terminate the transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone. The transaction still ends.
    /// SQLCODE -922 if a commit it rests on was left in doubt by a failed sync.
    pub(crate) fn rollback(&self, tx: &Txn) -> Result<()> {
        // Never refused by the memory budget, nor the collection after,
        // which gives back what the undo leaves (RULES.md D38).
        let _undoing = Undoing::begin();
        // Undo first, then release the locks. A damaged table is marked before
        // its locks are released, so no other transaction sees it partly undone.
        let gate = self.tx.gate.enter();
        let (undo, joined) = {
            let mut st = tx.state()?;
            (st.undo.take_from(0), mem::take(&mut st.joined))
        };
        // A run of entries that cannot be read back fails the tables it
        // may have changed, the rest undone all the same.
        let mut failures = match undo {
            Ok(undo) => undo.apply(tx.id),
            Err(error) => vec![UndoFailure { table: None, error, rows: true }],
        };
        for Joined { table, storage } in &joined {
            if let Err(error) = storage.rollback(tx.id) {
                failures.push(UndoFailure { table: Some(QualifiedName::clone(table)), error, rows: true });
            }
        }
        let damage = self.report_damage(&failures, "the transaction's rollback");
        tx.log_abort();
        drop(gate);
        tx.release();
        self.collect_ended(tx, &joined);
        // [DESIGN] RULES.md D846: a completion told only as the
        // transaction ends, rolled back, waits as it would have.
        tx.replied(damage)
    }
}

/// Removes the entries of row `id`, a version collected, of `row`, from
/// `indexes`, each with its columns, of those it holds
/// (`IndexDesc::entry_of`), but those it shares with its successor, which
/// `kept` (RULES.md D443). Those that cannot be are left: their places
/// among `indexes`, and their keys.
pub(crate) fn remove_entries(indexes: &[(&dyn Index, &IndexDesc, Sharing)], id: RowId, row: &Row, kept: Kept) -> Vec<(usize, Key)> {
    let mut failed = Vec::new();
    for (at, (index, ix, sharing)) in indexes.iter().enumerate() {
        if sharing.shares(kept) {
            continue;
        }
        if let Some(key) = ix.entry_of(row)
            && index.remove(&key, id).is_err()
        {
            failed.push((at, key));
        }
    }
    failed
}

/// An index of a table collected: its name, the index and its description.
type Found<'a> = (&'a str, &'a Arc<dyn Index>, &'a IndexDesc);

/// \[EXTENSION\] A global index over a partition's rows (RULES.md D157),
/// its description, and the partition's id its entries carry.
type Global = (Arc<dyn Index>, IndexDesc, u64);

/// The entries of `failed`, each the position in `found` of its index and
/// its key, as owed (`txn/owed.rs`).
/// \[DESIGN\] RULES.md D795: whether a collection that `left` a table
/// (garbage left, none left, or not collected as it is gone) changes
/// nothing shared, its lock `noted` as awaiting already or not. A table
/// left noted, or gone and not noted, changes nothing.
///
/// MC/DC pairs, in `unchanged_tests`: (left, noted) of (garbage, noted)
/// against (garbage, not noted) and (none left, noted); (gone, not noted)
/// against (gone, noted) and (none left, not noted).
fn unchanged(left: Option<bool>, noted: Option<bool>) -> bool {
    matches!((left, noted), (Some(true), Some(true)) | (None, Some(false)))
}

fn owed_entries(found: &[Found<'_>], failed: Vec<(usize, Key)>) -> Vec<Entry> {
    let owed =
        |(at, key)| found.get(at).map(|&(name, index, _): &Found<'_>| Entry { name: name.to_owned(), index: Arc::clone(index), key });
    failed.into_iter().filter_map(owed).collect()
}

/// \[EXTENSION\] Removes the entries of row `id`, a version collected, of
/// `row`, from the global indexes `globals` over its partition. One not
/// taken back is passed over by every reader (RULES.md D157).
fn remove_global(globals: &[Global], id: RowId, row: &Row) {
    for (index, ix, partition) in globals {
        if let (Some(key), Ok(entry)) = (ix.entry_of(row), crate::db::partitions::global::packed(*partition, id)) {
            let _ = index.remove(&key, entry);
        }
    }
}

impl Transactions {
    /// Notes that `table`'s engine has maintenance due, for the
    /// database's maintenance thread (`db/maintenance.rs`).
    pub(crate) fn note_maintenance(&self, table: &QualifiedName) {
        let mut noted = lock_builtin(&self.maintenance);
        noted.insert(table.clone());
        self.noted.store(true, Ordering::Release);
    }

    /// Whether a table has maintenance due, looked at without a lock, as
    /// every transaction's end looks.
    pub(crate) fn maintenance_due(&self) -> bool {
        self.noted.load(Ordering::Acquire)
    }

    /// The tables noted with maintenance due, forgotten.
    pub(crate) fn take_maintenance(&self) -> BTreeSet<QualifiedName> {
        let mut noted = lock_builtin(&self.maintenance);
        self.noted.store(false, Ordering::Release);
        mem::take(&mut *noted)
    }

    /// Notes that `table`, of an assertion's kept totals, has rows to fold,
    /// for the database's job folding them (`db/assertions/fold.rs`).
    pub(crate) fn note_fold(&self, table: &QualifiedName) {
        lock_builtin(&self.folds).insert(table.clone());
    }

    /// Whether a table of kept totals has rows to fold.
    pub(crate) fn folds_due(&self) -> bool {
        !lock_builtin(&self.folds).is_empty()
    }

    /// The tables noted with rows to fold, forgotten.
    pub(crate) fn take_folds(&self) -> BTreeSet<QualifiedName> {
        mem::take(&mut *lock_builtin(&self.folds))
    }
}

#[cfg(test)]
mod unchanged_tests {
    use super::unchanged;

    /// Verifies: [DESIGN] RULES.md D795, a table left with garbage and
    /// noted is unchanged; each condition flipped alone changes the outcome.
    #[test]
    fn a_table_left_noted_changes_nothing() {
        assert!(unchanged(Some(true), Some(true)));
        assert!(!unchanged(Some(true), Some(false)));
        assert!(!unchanged(Some(false), Some(true)));
    }

    /// Verifies: [DESIGN] RULES.md D795, a table gone and not noted is
    /// unchanged; each condition flipped alone changes the outcome.
    #[test]
    fn a_table_gone_and_not_noted_changes_nothing() {
        assert!(unchanged(None, Some(false)));
        assert!(!unchanged(None, Some(true)));
        assert!(!unchanged(Some(false), Some(false)));
        assert!(!unchanged(None, None));
    }
}
