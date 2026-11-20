//! Writing a row's versions (RULES.md D29, D79): a transaction's insert
//! and its change over the newest version, checked against what another
//! wrote, the large objects each names checked as its own and adopted;
//! a write outside any transaction, committed at once; and the objects a
//! transaction made, noted so undoing it frees them.

use super::lists::encode_change;
use super::{Change, Found, Past, conflict};
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, key, no_row};
use crate::error::Result;
use crate::log::Lsn;
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, Timestamp, TxId};

impl RowTable {
    /// Inserts row `id`, whose record is `record`, by `tx`.
    pub(in crate::engine::rows) fn insert_version(&self, tx: TxId, id: RowId, record: &[u8]) -> Result<bool> {
        let maker = if tx == NO_TRANSACTION { Maker::Committed(0) } else { Maker::Open(tx) };
        let version = Version { maker, deleted: false, older: None, record }.encode();
        if !self.tree.insert(&key(id), &version)? {
            return Ok(false);
        }
        self.mark_shadow(id);
        if tx != NO_TRANSACTION {
            let named = self.objects(id, record)?;
            self.owns(tx, &named)?;
            let open = self.open_of(tx)?;
            let mut open = lock_builtin(&open);
            open.adopted.extend(named);
            open.changes.push(Change::of_row(id, None, None, Vec::new()), &mut encode_change)?;
        }
        Ok(true)
    }

    /// Makes `record` the new version of row `id` by `tx`, or with none,
    /// deletes it: the version it replaced, for its zone. Outside any
    /// transaction, the newest is replaced at once, committed, and what it
    /// held freed, as recovery writes, and none is given back. A write that
    /// finds the entry it read gone or changed is tried once
    /// more: a freeze may have taken out the entry of a row frozen, the same
    /// row then read from its group (`rows/frozen.rs`); else it conflicts.
    pub(in crate::engine::rows) fn write_version(&self, tx: TxId, id: RowId, record: Option<&[u8]>, past: Past) -> Result<Option<Vec<u8>>> {
        if tx == NO_TRANSACTION {
            return self.write_latest(id, record).map(|()| None);
        }
        if let Some(replaced) = self.try_write(tx, id, record, past)? {
            return Ok(Some(replaced));
        }
        self.try_write(tx, id, record, past)?.map(Some).ok_or_else(conflict)
    }

    /// `write_version`'s work: the version it replaced, none if the entry
    /// it read was gone or changed by then.
    fn try_write(&self, tx: TxId, id: RowId, record: Option<&[u8]>, past: Past) -> Result<Option<Vec<u8>>> {
        let open = self.open_of(tx)?;
        let snapshot = lock_builtin(&open).snapshot;
        let k = key(id);
        let Some((newest, thawed)) = self.newest_or_frozen(id)? else { return no_row(id) };
        let (older, appended) = self.older_of((tx, snapshot), id, &newest, past)?;
        let (dropped, added) = self.owned_change(id, Version::parse(&newest)?.record, record)?;
        if let Err(e) = self.owns(tx, &added) {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            return Err(e);
        }
        let new = Version { maker: Maker::Open(tx), deleted: record.is_none(), older, record: record.unwrap_or_default() }.encode();
        let written = if thawed.is_some() { self.tree.insert(&k, &new)? } else { self.tree.replace_if(&k, &newest, &new)? };
        if !written {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            self.free(&thawed.unwrap_or_default());
            return Ok(None);
        }
        self.mark_shadow(id);
        if thawed.is_some() {
            self.frozen.shadow();
        }
        let before = appended.is_none().then(|| newest.clone());
        let mut open = lock_builtin(&open);
        open.adopted.extend(added);
        open.changes.push(Change::of_row(id, before, appended, dropped), &mut encode_change)?;
        Ok(Some(newest))
    }

    /// The older version a new one of row `id` by `tx`, of `snapshot`,
    /// names over `newest`, and where `newest` was appended to the undo
    /// store if it was committed; checked as `past` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if another transaction changed the row, or committed a
    /// change after `past`; -910 if the row is deleted.
    fn older_of(&self, (tx, snapshot): (TxId, Timestamp), id: RowId, newest: &[u8], past: Past) -> Result<(Option<Lsn>, Option<Lsn>)> {
        let version = Version::parse(newest)?;
        match self.maker(&version) {
            Maker::Open(by) => {
                if by != tx {
                    return Err(conflict());
                }
                if version.deleted {
                    return no_row(id);
                }
                Ok((version.older, None))
            }
            Maker::Committed(at) => {
                let newer = match past {
                    Past::Snapshot => at > snapshot,
                    Past::Stamp(seen) => at != seen,
                };
                if newer {
                    return Err(conflict());
                }
                if version.deleted {
                    return no_row(id);
                }
                let appended = self.undo.append(newest)?;
                Ok((Some(appended), Some(appended)))
            }
        }
    }

    /// Row `id`'s newest version: its entry in the tree; else, if it is
    /// frozen, a version holding its frozen values, to change it over, its
    /// entry to be inserted (`rows/frozen.rs`), with the pages of its values
    /// out of line; none if neither.
    fn newest_or_frozen(&self, id: RowId) -> Result<Option<Found>> {
        if let Some(newest) = self.tree.get(&key(id))? {
            return Ok(Some((newest, None)));
        }
        Ok(self.frozen_version(id)?.map(|(version, pages)| (version, Some(pages))))
    }

    /// Outside any transaction: replaces row `id`'s newest version with
    /// `record`, committed, or removes it; frees the values it held. A
    /// frozen row keeps its deletion (`rows/frozen.rs`).
    fn write_latest(&self, id: RowId, record: Option<&[u8]>) -> Result<()> {
        let k = key(id);
        let frozen = self.frozen_holds(id);
        let version =
            Version { maker: Maker::Committed(0), deleted: record.is_none(), older: None, record: record.unwrap_or_default() }.encode();
        let old = if record.is_some() || frozen { self.tree.replace(&k, &version)? } else { self.tree.delete(&k)? };
        self.mark_shadow(id);
        let Some(old) = old else {
            if self.frozen_row(id, Some(&[]))?.is_none() {
                return no_row(id);
            }
            self.tree.insert(&k, &version)?;
            self.mark_shadow(id);
            self.frozen.shadow();
            return Ok(());
        };
        let old = Version::parse(&old)?;
        let (dropped, _) = self.owned_change(id, old.record, record)?;
        self.free_values(id, old.record);
        self.free_objects(&dropped);
        Ok(())
    }

    /// Notes object `id` as made by `tx`, one of its changes, so undoing
    /// it frees the object.
    pub(in crate::engine::rows) fn note_made(&self, tx: TxId, id: u64) -> Result<()> {
        let open = self.open_of(tx)?;
        let mut open = lock_builtin(&open);
        open.made.insert(id);
        open.changes.push(Change { id, before: None, appended: None, dropped: Vec::new(), made: true }, &mut encode_change)
    }

    /// Notes the segments `ids`, made by `tx`, as parts of a directory it
    /// made: freed with it, never at its commit alone.
    pub(in crate::engine::rows) fn note_parts(&self, tx: TxId, ids: &[u64]) -> Result<()> {
        let open = self.open_of(tx)?;
        lock_builtin(&open).adopted.extend(ids.iter().copied());
        Ok(())
    }

    /// Whether `tx` made every object of `ids`, and no version it wrote
    /// names one yet: one a version names is that row's.
    pub(in crate::engine::rows) fn made_by(&self, tx: TxId, ids: &[u64]) -> Result<bool> {
        let open = self.open_of(tx)?;
        let open = lock_builtin(&open);
        Ok(ids.iter().all(|id| open.made.contains(id) && !open.adopted.contains(id)))
    }
}
