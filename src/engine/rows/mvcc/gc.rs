//! \[DESIGN\] Garbage collection of row versions (RULES.md D29, D31): what a
//! commit superseded, removed once its timestamp is past the horizon, and
//! the bookkeeping of transactions no other can be ordered against. Split
//! from mvcc.rs (RULES.md R-SIZE-2); see its documentation.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::{decode_dead, encode_dead};
use super::{Dead, Maker, Version};
use crate::engine::rows::{RowTable, key};
use crate::error::{Result, SqlError};
use crate::spill::SpillList;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{Timestamp, TxId};

impl RowTable {
    /// Removes the garbage of commits at or before `horizon`, each removed
    /// version's row given to `removed` as it goes, a segment of garbage
    /// read at a time.
    pub(in crate::engine::rows) fn collect(&self, horizon: Timestamp, removed: Removed<'_>) -> Result<()> {
        let late = !self.versions.late.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty();
        let due = self.versions.garbage_due.load(Ordering::SeqCst).min(self.versions.validated_due.load(Ordering::SeqCst));
        if !late && horizon < due {
            return Ok(());
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

    /// Whether anything awaits collection: garbage, transactions validated
    /// that others may be ordered against, or stamps left to make.
    pub(in crate::engine::rows) fn awaits_collection(&self) -> bool {
        !lock_builtin(&self.versions.garbage).is_empty()
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
            self.live.fetch_sub(1, Ordering::AcqRel);
        } else if version.deleted {
            self.live.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// Where a collection gives each version it removes: its row id and row.
pub(in crate::engine::rows) type Removed<'a> = &'a mut dyn FnMut(RowId, Row);
