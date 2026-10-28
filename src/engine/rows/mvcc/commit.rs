//! Validation and commit of a transaction in a table (RULES.md D29, D38):
//! the two-sided check of its reads and writes against those validated
//! before and after it, and the stamping of its versions, its garbage made
//! a segment of its changes at a time. Split from engine/rows/mvcc.rs
//! (RULES.md R-SIZE-2); see its documentation.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::decode_change;
use super::reads::Changed;
use super::{Dead, Maker, Validated, Version};
use crate::engine::rows::{RowTable, key};
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::MurmurSet;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{Timestamp, TxId};

impl RowTable {
    /// Two-sided validation of `tx` at `at` (see the module's
    /// documentation).
    pub(in crate::engine::rows) fn validate_open(&self, tx: TxId, at: Timestamp) -> Result<()> {
        let mut me = {
            let open = self.open_of(tx)?;
            let mut open = lock_builtin(&open);
            let wrote_many = open.changes.len() > self.versions.tracked();
            let mut writes = MurmurSet::default();
            if !wrote_many {
                open.changes.visit(&decode_change, &mut |c| {
                    writes.insert(c.id);
                    Ok(())
                })?;
            }
            let snapshot = open.reads_since.unwrap_or(open.snapshot);
            Validated { tx, snapshot, at, reads: mem::take(&mut open.reads), writes, wrote_many, changed: Changed::default() }
        };
        let held = self.versions.held();
        for &id in &me.writes {
            for row in self.changed_by(id, tx)?.into_iter().flatten() {
                me.changed.add(row, held);
            }
        }
        let mut validated = lock_builtin(&self.versions.validated);
        for other in validated.iter().filter(|v| v.tx != tx) {
            // It committed between this one's snapshot and its commit, and
            // wrote what this one read: this one missed it.
            let missed_other = me.snapshot < other.at && other.at < at && other.wrote_any(&me.reads);
            // It commits after this one but read before it, what this one
            // writes: it missed this one.
            let missed_me = other.snapshot < at && at < other.at && me.wrote_any(&other.reads);
            if missed_other || missed_me {
                return Err(SqlError::new(
                    Code::Conflict,
                    "[DESIGN] the transaction read rows another transaction changed meanwhile, or changed rows another read: it is not serializable; run it again",
                ));
            }
        }
        self.versions.validated_due.fetch_min(at, Ordering::SeqCst);
        validated.push(me);
        Ok(())
    }

    /// The rows of the newest version of row `id`, made by open transaction
    /// `tx`, and of the version before it, unless a deletion or none: a
    /// condition matching either was answered differently by the change.
    fn changed_by(&self, id: RowId, tx: TxId) -> Result<[Option<Row>; 2]> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok([None, None]) };
        let version = Version::parse(&newest)?;
        if version.maker != Maker::Open(tx) {
            return Ok([None, None]);
        }
        let made = self.row_of(id, &version)?;
        let before = version.older.map(|at| self.undo.read(at)).transpose()?;
        let before = before.map(|b| Version::parse(&b).and_then(|v| self.row_of(id, &v))).transpose()?.flatten();
        Ok([made, before])
    }

    /// The row `version` of row `id` holds, unless a deletion.
    fn row_of(&self, id: RowId, version: &Version<'_>) -> Result<Option<Row>> {
        if version.deleted {
            return Ok(None);
        }
        self.decode(id, version.record, None).map(Some)
    }

    /// Stamps the versions of `tx` with `at`, and makes garbage of what they
    /// superseded, for when `at` is past the horizon.
    pub(in crate::engine::rows) fn commit_open(&self, tx: TxId, at: Timestamp) {
        let removed = self.versions.open.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&tx);
        let Some(open) = removed else { return };
        let changes = mem::take(&mut lock_builtin(&open).changes);
        if changes.is_empty() {
            return;
        }
        let (mut late, mut lost) = (Vec::new(), false);
        // A segment at a time: its garbage added before the next is read.
        for segment in changes.into_segments(false, &decode_change) {
            let Ok(segment) = segment else {
                lost = true;
                continue;
            };
            let mut dead = Vec::new();
            for change in segment {
                // The first change of a row stamps it; its later ones find
                // it stamped.
                match self.stamp(tx, change.id, at) {
                    Ok(true) => dead.push(Dead::Deletion(change.id)),
                    Ok(false) => {}
                    Err(_) => late.push(change.id),
                }
                match (change.before, change.appended) {
                    (Some(own), _) => dead.push(Dead::Own(change.id, own)),
                    (None, Some(appended)) => dead.push(Dead::Older(change.id, appended)),
                    (None, None) => {}
                }
            }
            self.add_garbage(at, dead);
        }
        if lost || !late.is_empty() {
            let rows = (!lost).then_some(late);
            self.versions.late.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(tx, (at, rows));
        }
    }

    /// Stamps the newest version of row `id`, made by `tx`, committed at
    /// `at`; whether it is a deletion.
    pub(super) fn stamp(&self, tx: TxId, id: RowId, at: Timestamp) -> Result<bool> {
        let k = key(id);
        let Some(newest) = self.tree.get(&k)? else { return Ok(false) };
        let version = Version::parse(&newest)?;
        if version.maker != Maker::Open(tx) {
            return Ok(false);
        }
        self.tree.replace(&k, &Version { maker: Maker::Committed(at), ..version }.encode())?;
        Ok(version.deleted)
    }
}
