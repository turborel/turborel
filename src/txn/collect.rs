//! \[DESIGN\] The garbage collection of versions that follows the ends of
//! transactions (RULES.md D29, D445, D450, D451, D920): a table a
//! transaction wrote is collected by its end once a batch of garbage is
//! due, and noted as awaiting collection until then; the maintenance
//! job's pass, a checkpoint and a close collect every table awaiting,
//! as far as the horizon lets.

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::kept::{key_reads, needs_row};
use super::owed::{Entry, Owed};
use super::{Joined, Sharing, TableLock, Timestamp, Transactions, Txn};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::index::{Index, Key};
use crate::storage::{Kept, Needed, Row, RowId, TableStorage};
use crate::sync::lock_builtin;

/// \[DESIGN\] The commits a table's batch may be due before the end of a
/// transaction that wrote it collects it itself (RULES.md D925): four
/// batches (D451). A pass that keeps up takes a batch within the one
/// after it, so the bound is met only when the pass does not run, or
/// runs behind the commits, and the versions held stay bounded by it.
const COLLECT_BEHIND: Timestamp = 256;

impl State {
    /// \[DESIGN\] After `tx` ended, the garbage collection of the tables of
    /// `joined` (RULES.md D29, D451, D920, D925). A table is left noted as
    /// awaiting collection, for the maintenance job's pass. One with a
    /// batch due has the pass asked for at once, so the pass, not the
    /// end, removes the versions no open transaction can read, their
    /// index entries with them. Only a table whose batch has been due for
    /// `COLLECT_BEHIND` commits, the pass not keeping up or not running,
    /// is collected here. A failure leaves the garbage for a later
    /// collection: the transaction has already committed or rolled back.
    /// A temporary table's instances are each collected by their session
    /// instead (`db/temporary.rs`).
    ///
    /// A session that collects once its reply is sent (RULES.md D450) only
    /// notes them, for that collection, which collects them itself (D453).
    pub(super) fn collect_ended(&self, tx: &Txn, joined: &[Joined]) {
        if tx.stop.collects_later() {
            return self.collect_later(tx, joined);
        }
        let horizon = self.tx.horizon();
        // The gate is entered for the first table collected: most ends
        // find none.
        let (mut gate, mut due) = (None, false);
        for Joined { table, storage } in joined {
            if storage.collection_due(horizon.saturating_sub(COLLECT_BEHIND)) {
                gate.get_or_insert_with(|| self.tx.gate.enter());
                self.collect_noting(table, horizon, false);
            } else {
                due |= storage.collection_due(horizon);
                self.leave_awaiting(table, storage);
            }
        }
        if due {
            self.tx.pass_soon();
        }
    }

    /// Notes the tables of `joined`, written by `tx`, whose session
    /// collects once its reply is sent (RULES.md D450, D453).
    fn collect_later(&self, tx: &Txn, joined: &[Joined]) {
        tx.stop.owe(joined.iter().map(|j| Arc::clone(&j.table)));
        // [DESIGN] RULES.md D795: a table noted already is left so,
        // with nothing shared written.
        let new: Vec<&QualifiedName> =
            joined.iter().map(|j| &*j.table).filter(|t| !self.lock_of(t).is_ok_and(|lock| lock.noted())).collect();
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
    }

    /// \[DESIGN\] Leaves `table`, of `storage`, whose collection is not
    /// due, for a later one (RULES.md D920): noted as awaiting collection
    /// if it holds garbage and is not noted yet, so nothing shared is
    /// written by the ends that follow (D795); and noted for maintenance
    /// if its engine has some due, as a collection would have noted it.
    fn leave_awaiting(&self, table: &QualifiedName, storage: &Arc<dyn TableStorage>) {
        if storage.maintenance_due() {
            self.tx.note_maintenance(table);
        }
        let Ok(lock) = self.lock_of(table) else { return };
        if lock.noted() || !storage.uncollected() {
            return;
        }
        let mut uncollected = lock_builtin(&self.tx.uncollected);
        lock.set_noted(true);
        uncollected.insert(table.clone());
        self.awaiting(&uncollected);
    }

    /// \[DESIGN\] The maintenance job's pass (RULES.md D920): collects
    /// every table awaiting collection as far as the horizon lets, due or
    /// not, waiting for a collection under way, so one pass takes the
    /// garbage many commits left. An end that leaves a table awaiting
    /// after it asks for the next.
    pub(crate) fn collect_awaiting(&self) {
        self.tx.pass_begun();
        if self.tx.awaiting.load(Ordering::Acquire) {
            self.gc_waiting(std::iter::empty());
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

    /// Collects each of `tables`, and each table awaiting collection, due
    /// or not, waiting for a collection of a table under way rather than
    /// passing it over (RULES.md D445): for a checkpoint that needs every
    /// table collected as far as the horizon lets. `tables` names each
    /// once; one still awaited is collected once too.
    pub(crate) fn gc_waiting<'t>(&self, tables: impl Iterator<Item = &'t QualifiedName> + Clone) {
        let _gate = self.tx.gate.enter();
        let horizon = self.tx.horizon();
        // [DESIGN] RULES.md D795: the set is read only while it holds any.
        let waiting: Vec<QualifiedName> = if self.tx.awaiting.load(Ordering::Acquire) {
            lock_builtin(&self.tx.uncollected).iter().cloned().collect()
        } else {
            Vec::new()
        };
        for table in waiting.iter().filter(|w| !tables.clone().any(|t| t == *w)) {
            self.collect_noting(table, horizon, true);
        }
        for table in tables {
            self.collect_noting(table, horizon, true);
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
        // Its lock and its storage, found once for its collection.
        let (lock, storage) = (self.lock_of(table).ok(), self.storage_of(table).ok());
        let left = self.collect_table(table, (lock, storage), horizon, wait);
        if unchanged(left, lock.map(|lock| lock.noted())) {
            return;
        }
        let mut uncollected = lock_builtin(&self.tx.uncollected);
        // Asked again under the lock: a commit that added garbage while
        // this one collected, and was passed over (RULES.md D445), has
        // noted the table, and its note must not be taken out.
        let still = || storage.is_some_and(|s| s.uncollected());
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

    /// Collects `table`'s garbage past `horizon`: whether versions of it
    /// still await collection, or none if it is not there, or is a
    /// temporary table: an instance of one is collected by its session as
    /// its transactions end (`db/temporary.rs`), the definition's storage
    /// holding nothing.
    fn collect_table(&self, table: &QualifiedName, found: Resolved<'_>, horizon: Timestamp, wait: bool) -> Option<bool> {
        if self.catalog.table(table).is_none_or(|t| t.temporary.is_some()) {
            return None;
        }
        // Not while an index of it is built, which counts every version it
        // finds; nor with definitions older than its indexes, whose entries
        // for the versions removed would stay. A later transaction collects
        // them.
        let (lock, storage) = (found.0?, found.1?);
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

/// A table's lock and its storage, each if the definitions hold it, found
/// once for its collection.
type Resolved<'a> = (Option<&'a Arc<TableLock>>, Option<&'a Arc<dyn TableStorage>>);

/// An index of a table collected: its name, the index and its description.
type Found<'a> = (&'a str, &'a Arc<dyn Index>, &'a IndexDesc);

/// \[EXTENSION\] A global index over a partition's rows (RULES.md D157),
/// its description, and the partition's id its entries carry.
type Global = (Arc<dyn Index>, IndexDesc, u64);

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

/// The entries of `failed`, each the position in `found` of its index and
/// its key, as owed (`txn/owed.rs`).
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
    /// \[DESIGN\] Whether the pass collecting the tables awaiting
    /// collection is to be asked for (RULES.md D920): some await, and none
    /// has been asked for since the last began. It counts as asked from
    /// now.
    pub(crate) fn ask_pass(&self) -> bool {
        let asks = self.awaiting.load(Ordering::Acquire) && !self.pass_asked.swap(true, Ordering::AcqRel);
        if asks {
            *lock_builtin(&self.pass_since) = Some(Instant::now());
        }
        asks
    }

    /// \[DESIGN\] How long until the pass asked for is to be made (RULES.md
    /// D920), `after` from when it was asked for: zero if that has passed,
    /// none if no pass is asked for. The maintenance job runs at once for
    /// the maintenance noted, as nearly every commit of a busy table notes
    /// some, and a pass made with each run would be a collection at every
    /// commit again, on its thread.
    pub(crate) fn pass_wait(&self, after: Duration) -> Option<Duration> {
        if !self.pass_asked.load(Ordering::Acquire) {
            return None;
        }
        // With no time noted it was asked for at once (`pass_soon`).
        let waited = lock_builtin(&self.pass_since).map_or(after, |since| since.elapsed());
        Some(after.saturating_sub(waited))
    }

    /// \[DESIGN\] A table has a batch due (RULES.md D925): the pass is
    /// asked for, to be made at once.
    fn pass_soon(&self) {
        if self.pass_now.load(Ordering::Acquire) {
            return;
        }
        *lock_builtin(&self.pass_since) = None;
        self.pass_asked.store(true, Ordering::Release);
        self.pass_now.store(true, Ordering::Release);
    }

    /// Whether the pass's job is to be asked for at once: a pass was, and
    /// its job has not been since. It counts as asked from now.
    pub(crate) fn take_pass_soon(&self) -> bool {
        self.pass_now.load(Ordering::Acquire) && !self.pass_told.swap(true, Ordering::AcqRel)
    }

    /// The pass begins: an end that leaves a table awaiting from now asks
    /// for another.
    fn pass_begun(&self) {
        *lock_builtin(&self.pass_since) = None;
        self.pass_asked.store(false, Ordering::Release);
        self.pass_now.store(false, Ordering::Release);
        self.pass_told.store(false, Ordering::Release);
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
