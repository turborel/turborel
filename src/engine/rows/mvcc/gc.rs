//! \[DESIGN\] Garbage collection of row versions (RULES.md D29, D31): what a
//! commit superseded, removed once its timestamp is past the horizon; what
//! an undone change appended, released once the transactions then open in
//! the table have ended; and the bookkeeping of transactions no other can
//! be ordered against. Split from mvcc.rs (RULES.md R-SIZE-2); see its
//! documentation.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::{decode_dead, encode_dead};
use super::{Dead, Maker, Version};
use crate::engine::rows::{RowTable, key};
use crate::error::{Result, SqlError};
use crate::log::Lsn;
use crate::spill::SpillList;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, Timestamp, TxId};

impl RowTable {
    /// Removes the garbage of commits at or before `horizon`, each removed
    /// version's row given to `removed` as it goes, a segment of garbage
    /// read at a time.
    pub(in crate::engine::rows) fn collect(&self, horizon: Timestamp, removed: Removed<'_>) -> Result<()> {
        // Noted before anything is released (`RowTable::unpinned`).
        self.versions.collected.fetch_max(horizon, Ordering::SeqCst);
        self.collect_undone();
        let late = !self.versions.late.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty();
        let due = self.versions.garbage_due.load(Ordering::SeqCst).min(self.versions.validated_due.load(Ordering::SeqCst));
        if !late && horizon < due {
            return self.undo.truncate();
        }
        self.stamp_late();
        let ready = {
            let mut garbage = lock_builtin(&self.versions.garbage);
            let later = garbage.split_off(&horizon.saturating_add(1));
            let ready = mem::replace(&mut *garbage, later);
            self.versions.garbage_due.store(garbage.first_key_value().map_or(u64::MAX, |(&at, _)| at), Ordering::SeqCst);
            ready
        };
        let mut failure = None;
        for (at, dead) in ready {
            // A segment at a time; one that cannot be read back is lost to
            // collection, its versions left where they are.
            for segment in dead.into_segments(false, &decode_dead) {
                let items = match segment {
                    Ok(items) => items,
                    Err(e) => {
                        failure.get_or_insert(e);
                        continue;
                    }
                };
                self.remove_all(items, at, (&mut *removed, &mut failure));
            }
        }
        {
            let mut validated = lock_builtin(&self.versions.validated);
            validated.retain(|v| v.at > horizon);
            self.versions.validated_due.store(validated.iter().map(|v| v.at).min().unwrap_or(u64::MAX), Ordering::SeqCst);
        }
        self.undo.truncate()?;
        failure.map_or(Ok(()), Err)
    }

    /// What `read` makes of the entry `newest` of the row of key `k`, read by
    /// a pass with no transaction open, the maintenance thread's, at
    /// `horizon`, or the table's latest collection if later: none if the
    /// row has left the tree meanwhile.
    ///
    /// No transaction pins the older versions such a pass goes back to, so
    /// one may be released, and the undo store truncated, as it reads: by
    /// a rollback, which puts back the entry naming it first, or by a
    /// collection past the commit that superseded it, which notes its
    /// horizon first (`Versions::collected`). So the entry and the horizon
    /// are read again, and `read` tried again, while either has changed;
    /// failing on the same ones, its error is a real one.
    pub(in crate::engine::rows) fn unpinned<T>(
        &self,
        (k, newest): (&[u8], Vec<u8>),
        horizon: Timestamp,
        read: &mut dyn FnMut(Vec<u8>, Timestamp) -> Result<T>,
    ) -> Result<Option<T>> {
        let latest = || horizon.max(self.versions.collected.load(Ordering::SeqCst));
        let (mut entry, mut at) = (newest, latest());
        loop {
            let error = match read(entry.clone(), at) {
                Ok(found) => return Ok(Some(found)),
                Err(e) => e,
            };
            let Some(again) = self.tree.get(k)? else { return Ok(None) };
            let later = latest();
            if again == entry && later == at {
                return Err(error);
            }
            (entry, at) = (again, later);
        }
    }

    /// Removes `items`, garbage of `at`, their rows to `removed`; one that
    /// cannot be is garbage again, its error the first `failure`.
    fn remove_all(&self, items: Vec<Dead>, at: Timestamp, (removed, failure): (Removed<'_>, &mut Option<SqlError>)) {
        for item in items {
            if let Err(e) = self.remove_dead(&item, at, &mut *removed) {
                failure.get_or_insert(e);
                self.add_garbage(at, vec![item]);
            }
        }
    }

    /// Whether anything awaits collection: garbage, versions undone changes
    /// appended, transactions validated that others may be ordered
    /// against, or stamps left to make.
    pub(in crate::engine::rows) fn awaits_collection(&self) -> bool {
        !lock_builtin(&self.versions.garbage).is_empty()
            || !lock_builtin(&self.versions.undone).is_empty()
            || !lock_builtin(&self.versions.validated).is_empty()
            || !self.versions.late.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty()
    }

    /// Adds `dead`, garbage once the horizon reaches `at`.
    pub(super) fn add_garbage(&self, at: Timestamp, dead: Vec<Dead>) {
        let space = self.versions.space();
        let mut garbage = lock_builtin(&self.versions.garbage);
        let list = garbage.entry(at).or_insert_with(|| SpillList::new(space));
        for item in dead {
            // An item that cannot be written is held all the same.
            list.push(item, &mut encode_dead).unwrap_or_default();
        }
        self.versions.garbage_due.fetch_min(at, Ordering::SeqCst);
    }

    /// Adds the version a change of `tx` appended, `appended`, the change
    /// undone, to be released once no other transaction open in the table
    /// now is: a reader of the table begins in it before it reads, so one
    /// that read the undone version and may go back to `appended` is open
    /// in it now. `tx` is not counted: it reads its own versions, never
    /// the committed ones they replaced. Not by timestamp, as a commit's
    /// garbage is: a reader that begins in the table from now on finds
    /// the version put back, whatever its snapshot, and a timestamp past
    /// the snapshots of those open would wait for a commit to come.
    pub(super) fn add_undone(&self, tx: TxId, appended: Lsn) {
        let space = self.versions.space();
        // Under the lock collection reads the transactions open by, so it
        // never releases by what was open before this is added.
        let mut undone = lock_builtin(&self.versions.undone);
        let newest =
            self.versions.open.read().unwrap_or_else(std::sync::PoisonError::into_inner).keys().filter(|&&by| by != tx).max().copied();
        let list = undone.entry(newest.unwrap_or(NO_TRANSACTION)).or_insert_with(|| SpillList::new(space));
        // An item that cannot be written is held all the same.
        list.push(Dead::Undone(appended), &mut encode_dead).unwrap_or_default();
    }

    /// Releases the versions undone changes appended for which no
    /// transaction open in the table then is still open: those added when
    /// each open then is older than the oldest open now.
    fn collect_undone(&self) {
        let ready = {
            let mut undone = lock_builtin(&self.versions.undone);
            if undone.is_empty() {
                return;
            }
            let oldest = self.versions.open.read().unwrap_or_else(std::sync::PoisonError::into_inner).keys().min().copied();
            let later = oldest.map(|tx| undone.split_off(&tx)).unwrap_or_default();
            mem::replace(&mut *undone, later)
        };
        // A segment that cannot be read back is lost, its versions kept in
        // the undo store until the database closes.
        let segments = ready.into_values().flat_map(|list| list.into_segments(false, &decode_dead));
        for item in segments.flatten().flatten() {
            // Each a `Dead::Undone`, which only releases its position, of no
            // timestamp.
            let _ = self.remove_dead(&item, 0, &mut |_, _| {});
        }
    }

    fn remove_dead(&self, dead: &Dead, at: Timestamp, removed: Removed<'_>) -> Result<()> {
        match dead {
            Dead::Own(id, bytes) => self.remove_version(*id, bytes, removed),
            Dead::Older(id, appended) => {
                self.remove_version(*id, &self.undo.read(*appended)?, removed)?;
                self.undo.release(*appended);
                Ok(())
            }
            Dead::Undone(appended) => {
                self.undo.release(*appended);
                Ok(())
            }
            Dead::Deletion(id) => {
                let k = key(*id);
                let Some(newest) = self.tree.get(&k)? else { return Ok(()) };
                let version = Version::parse(&newest)?;
                // A frozen row's deletion is kept, so its group's row is not
                // seen again (`rows/frozen.rs`).
                if version.deleted && self.maker(&version) == Maker::Committed(at) && !self.frozen_holds(*id) {
                    self.tree.delete_if(&k, &newest)?;
                }
                Ok(())
            }
        }
    }

    /// The row of the version garbage `item` superseded, `columns` decoded,
    /// or all; none for a deletion, or no version.
    pub(super) fn garbage_row(&self, item: Dead, columns: Option<&[usize]>) -> Result<Option<(RowId, Row)>> {
        let (id, bytes) = match item {
            Dead::Own(id, bytes) => (id, bytes),
            Dead::Older(id, at) => (id, self.undo.read(at)?),
            Dead::Deletion(_) | Dead::Undone(_) => return Ok(None),
        };
        let version = Version::parse(&bytes)?;
        if version.deleted {
            return Ok(None);
        }
        Ok(Some((id, self.decode(id, version.record, columns)?)))
    }

    /// Frees what version `bytes` of row `id` holds out of line, and
    /// records its row.
    fn remove_version(&self, id: RowId, bytes: &[u8], removed: Removed<'_>) -> Result<()> {
        let version = Version::parse(bytes)?;
        if !version.deleted {
            removed(id, self.decode(id, version.record, None)?);
            self.free_values(id, version.record);
        }
        Ok(())
    }

    /// Stamps what commits could not, and forgets each transaction once all
    /// its rows are.
    fn stamp_late(&self) {
        let late: Vec<(TxId, Timestamp, Option<Vec<RowId>>)> = self
            .versions
            .late
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(&tx, (at, ids))| (tx, *at, ids.clone()))
            .collect();
        // A commit whose rows could not be read back stays late.
        for (tx, at, ids) in late.into_iter().filter_map(|(tx, at, ids)| Some((tx, at, ids?))) {
            if ids.iter().all(|&id| self.stamp(tx, id, at).is_ok()) {
                self.versions.late.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&tx);
            }
        }
    }

    /// Counts a change undone: an insert taken out, or a deletion put back.
    pub(super) fn count_undone(&self, version: &Version<'_>, inserted: bool) {
        if inserted {
            self.counted.live.fetch_sub(1, Ordering::AcqRel);
        } else if version.deleted {
            self.counted.live.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// Where a collection gives each version it removes: its row id and row.
pub(in crate::engine::rows) type Removed<'a> = &'a mut dyn FnMut(RowId, Row);
