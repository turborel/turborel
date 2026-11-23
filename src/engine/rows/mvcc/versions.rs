//! Writing a row's versions (RULES.md D29, D79): a transaction's insert
//! and its change over the newest version, checked against what another
//! wrote, the large objects each names checked as its own and adopted;
//! a write outside any transaction, committed at once; and the objects a
//! transaction made, noted so undoing it frees them.

use std::sync::Mutex;

use super::lists::encode_change;
use super::{Change, Found, Open, Past, conflict};
use crate::engine::rows::version::{FRAMED, Maker, Version, frame};
use crate::engine::rows::{RowTable, key, no_row};
use crate::error::Result;
use crate::log::Lsn;
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, Timestamp, TxId};

impl RowTable {
    /// Inserts row `id` by `tx`, whose record `framed` holds behind the
    /// room for its version's header (`version::frame`).
    pub(in crate::engine::rows) fn insert_version(&self, tx: TxId, id: RowId, framed: &mut [u8]) -> Result<bool> {
        let maker = if tx == NO_TRANSACTION { Maker::Committed(0) } else { Maker::Open(tx) };
        let version = frame(framed, (maker, None))?;
        let record = Version::parse(version)?.record;
        if tx != NO_TRANSACTION {
            // Claimed before its first version is in the tree (`status.rs`).
            self.versions.statuses.claim(tx);
        }
        let Some(placed) = self.tree.insert_placed(&key(id), version)? else { return Ok(false) };
        self.mark_shadow(id);
        if tx != NO_TRANSACTION {
            let named = self.objects(id, record)?;
            self.owns(tx, &named)?;
            let open = self.open_of(tx)?;
            let mut open = lock_builtin(&open);
            open.adopted.extend(named);
            open.changes_in(&self.versions).push(Change { placed, ..Change::of_row(id, None, None, Vec::new()) }, &mut encode_change)?;
        }
        Ok(true)
    }

    /// Makes the record `framed` holds behind the room for its version's
    /// header (`version::frame`) the new version of row `id` by `tx`, or with none,
    /// deletes it: the version it replaced, for its zone. Outside any
    /// transaction, the newest is replaced at once, committed, and what it
    /// held freed, as recovery writes, and none is given back. A write that
    /// finds the entry it read gone or changed is tried once
    /// more: a freeze may have taken out the entry of a row frozen, the same
    /// row then read from its group (`rows/frozen.rs`); else it conflicts.
    pub(in crate::engine::rows) fn write_version(
        &self,
        tx: TxId,
        id: RowId,
        mut framed: Option<&mut [u8]>,
        past: Past,
    ) -> Result<Option<Vec<u8>>> {
        if tx == NO_TRANSACTION {
            return self.write_latest(id, framed.as_deref().map(|f| f.get(FRAMED..).unwrap_or_default())).map(|()| None);
        }
        if let Some(replaced) = self.try_write(tx, id, framed.as_deref_mut(), past)? {
            return Ok(Some(replaced));
        }
        self.try_write(tx, id, framed, past)?.map(Some).ok_or_else(conflict)
    }

    /// `write_version`'s work: the version it replaced, none if the entry
    /// it read was gone or changed by then.
    fn try_write(&self, tx: TxId, id: RowId, framed: Option<&mut [u8]>, past: Past) -> Result<Option<Vec<u8>>> {
        let opened = self.open_of(tx)?;
        let open: &Mutex<Open> = &opened;
        let Some(newest) = self.newest_or_frozen(id)? else { return no_row(id) };
        self.write_found((tx, open), id, newest, (framed, past))
    }

    /// `try_write`'s work over `newest`, row `id`'s newest version as it
    /// was read, by `tx`, whose bookkeeping is `open`. A caller that makes
    /// the new record of that version's bytes reads it first, and writes
    /// over it here (`mvcc/patch.rs`, RULES.md D916).
    pub(super) fn write_found(
        &self,
        (tx, open): (TxId, &Mutex<Open>),
        id: RowId,
        (newest, found): Found,
        (framed, past): (Option<&mut [u8]>, Past),
    ) -> Result<Option<Vec<u8>>> {
        let snapshot = lock_builtin(open).snapshot;
        let k = key(id);
        self.versions.statuses.claim(tx);
        let (older, appended) = self.older_of((tx, snapshot), id, &newest, past)?;
        let record = framed.as_deref().map(|f| f.get(FRAMED..).unwrap_or_default());
        let (dropped, added) = self.owned_change(id, Version::parse(&newest)?.record, record)?;
        if let Err(e) = self.owns(tx, &added) {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            return Err(e);
        }
        // A deletion's version is its header alone; a row's is framed in
        // the buffer its record was encoded in, not copied (RULES.md D846).
        let deletion;
        let new = if let Some(framed) = framed {
            frame(framed, (Maker::Open(tx), older))?
        } else {
            deletion = Version { maker: Maker::Open(tx), deleted: true, older, record: &[] }.encode();
            &deletion
        };
        // In the tree, written from the leaf the read found it in, with no
        // second descent (RULES.md D446).
        let (written, thawed) = match found {
            Ok(descended) => (self.tree.replace_found_placed(&descended, &k, (&newest, new))?, None),
            Err(pages) => (self.tree.insert_placed(&k, new)?, Some(pages)),
        };
        let Some(placed) = written else {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            self.free(&thawed.unwrap_or_default());
            return Ok(None);
        };
        self.mark_shadow(id);
        if thawed.is_some() {
            self.frozen.shadow();
        }
        let before = appended.is_none().then(|| newest.clone());
        let mut open = lock_builtin(open);
        open.adopted.extend(added);
        open.changes_in(&self.versions).push(Change { placed, ..Change::of_row(id, before, appended, dropped) }, &mut encode_change)?;
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
                // A version committed but not yet stamped is appended
                // stamped, as only the newest version is ever stamped
                // (`stamps.rs`).
                let appended = if version.maker == Maker::Committed(at) {
                    self.undo.append(newest)?
                } else {
                    self.undo.append(&Version { maker: Maker::Committed(at), ..version }.encode())?
                };
                Ok((Some(appended), Some(appended)))
            }
        }
    }

    /// Row `id`'s newest version: its entry in the tree; else, if it is
    /// frozen, a version holding its frozen values, to change it over, its
    /// entry to be inserted (`rows/frozen.rs`), with the pages of its values
    /// out of line; none if neither.
    pub(super) fn newest_or_frozen(&self, id: RowId) -> Result<Option<Found>> {
        // Read into the buffer the thread's last write gave back, so an
        // update allocates none for it (RULES.md D874).
        let mut newest = spare();
        if let Some(descended) = self.tree.get_for_write_into(&key(id), &mut newest)? {
            return Ok(Some((newest, Ok(descended))));
        }
        give_back(newest);
        Ok(self.frozen_version(id)?.map(|(version, pages)| (version, Err(pages))))
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
        open.changes_in(&self.versions).push(Change { made: true, ..Change::of_row(id, None, None, Vec::new()) }, &mut encode_change)
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

/// \[DESIGN\] The most bytes of a buffer a thread keeps between writes: a
/// longer version's is let go, so a thread that once wrote a long row does
/// not hold its bytes for good.
const KEPT_BYTES: usize = 64 * 1024;

thread_local! {
    /// The buffer this thread's last write read the version it replaced
    /// into, given back once the write was done with it.
    static SPARE: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The thread's spare buffer, taken; an empty one if it keeps none.
fn spare() -> Vec<u8> {
    SPARE.try_with(|kept| std::mem::take(&mut *kept.borrow_mut())).unwrap_or_default()
}

/// \[DESIGN\] Gives `buffer`, the version a write replaced, back to the
/// thread for its next write to read into (RULES.md D874), unless it is
/// longer than a thread keeps.
pub(super) fn give_back(buffer: Vec<u8>) {
    if buffer.capacity() > KEPT_BYTES {
        return;
    }
    // A thread ending keeps none.
    let _ = SPARE.try_with(|kept| *kept.borrow_mut() = buffer);
}

// Unit tests: a buffer given back is the next taken (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D874: the buffer a write gives back is
    /// the one the thread's next write reads into, its room kept; one
    /// longer than a thread keeps is let go.
    #[test]
    fn a_buffer_given_back_is_the_next_taken() {
        let mut buffer = spare();
        buffer.reserve(100);
        let room = buffer.capacity();
        give_back(buffer);
        let again = spare();
        assert!(again.capacity() >= 100 && again.capacity() == room, "the room given back");
        assert_eq!(spare().capacity(), 0, "taken, so none is kept");
        give_back(Vec::with_capacity(KEPT_BYTES + 1));
        assert_eq!(spare().capacity(), 0, "too long to keep");
    }
}
