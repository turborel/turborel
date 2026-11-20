//! Validation of a transaction in a table (RULES.md D29, D38): the
//! two-sided check of its reads and writes against those validated before
//! and after it. Its commit is `stamps.rs`'s. See `mvcc.rs`.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::decode_change;
use super::reads::Changed;
use super::{Maker, Reads, Validated, Version};
use crate::engine::rows::{RowTable, key};
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::MurmurSet;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{Timestamp, TxId};

impl RowTable {
    /// Two-sided validation of `tx` at `at` (see the module's
    /// documentation); none of one whose writes keep what every snapshot
    /// reads (`TableStorage::preserving`), which no other is checked
    /// against either.
    pub(in crate::engine::rows) fn validate_open(&self, tx: TxId, at: Timestamp) -> Result<()> {
        let mut me = {
            let open = self.open_of(tx)?;
            let mut open = lock_builtin(&open);
            if open.preserving {
                return Ok(());
            }
            let wrote_many = open.changes.len() > self.versions.tracked();
            let mut writes = MurmurSet::default();
            if !wrote_many {
                open.changes.visit(&decode_change, &mut |c| {
                    writes.extend((!c.made).then_some(c.id));
                    Ok(())
                })?;
            }
            let snapshot = open.reads_since.unwrap_or(open.snapshot);
            Validated { tx, snapshot, at, reads: mem::take(&mut open.reads), writes, wrote_many, changed: None }
        };
        let mut validated = lock_builtin(&self.versions.validated);
        for other in validated.iter_mut().filter(|v| v.tx != tx) {
            // It committed between this one's snapshot and its commit, and
            // wrote what this one read: this one missed it.
            let missed_other = me.snapshot < other.at && other.at < at && self.wrote_any(other, &me.reads)?;
            // It commits after this one but read before it, what this one
            // writes: it missed this one.
            let missed_me = other.snapshot < at && at < other.at && self.wrote_any(&mut me, &other.reads)?;
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

    /// Whether `reads` cover a row `v` wrote (`Validated::wrote_by_id`), or
    /// one whose version before or after its change a condition or region
    /// read matches, though the reader never saw it (a phantom). Its images
    /// are decoded the first time a reader's conditions ask (RULES.md
    /// D444): its versions are still there, as the reader, open with a
    /// snapshot before its commit, holds the horizon before it.
    fn wrote_any(&self, v: &mut Validated, reads: &Reads) -> Result<bool> {
        if let Some(known) = v.wrote_by_id(reads) {
            return Ok(known);
        }
        if v.changed.is_none() {
            let held = self.versions.held();
            let mut changed = Changed::default();
            for &id in &v.writes {
                for row in self.changed_by(id, (v.tx, v.at))?.into_iter().flatten() {
                    changed.add(row, held);
                }
            }
            v.changed = Some(changed);
        }
        Ok(v.changed.as_ref().is_some_and(|c| c.matched_by(&reads.conditions, &reads.regions)))
    }

    /// The rows of the version of row `id` made by transaction `tx`, open
    /// or committed at `at`, and of the version before it, unless a
    /// deletion or none: a condition matching either was answered
    /// differently by the change. Found going back from the newest; one not
    /// found is an error, which refuses the transaction validated.
    fn changed_by(&self, id: RowId, (tx, at): (TxId, Timestamp)) -> Result<[Option<Row>; 2]> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok([None, None]) };
        let mut bytes = newest;
        loop {
            let version = Version::parse(&bytes)?;
            let maker = self.maker(&version);
            if maker == Maker::Open(tx) || maker == Maker::Committed(at) {
                let made = self.row_of(id, &version)?;
                let before = version.older.map(|older| self.undo.read(older)).transpose()?;
                let before = before.map(|b| Version::parse(&b).and_then(|v| self.row_of(id, &v))).transpose()?.flatten();
                return Ok([made, before]);
            }
            let Some(older) = version.older else {
                return Err(SqlError::new(
                    Code::Conflict,
                    format!("[DESIGN] the version row {id} had at {at} is gone; run the transaction again"),
                ));
            };
            bytes = self.undo.read(older)?;
        }
    }

    /// The row `version` of row `id` holds, unless a deletion.
    fn row_of(&self, id: RowId, version: &Version<'_>) -> Result<Option<Row>> {
        if version.deleted {
            return Ok(None);
        }
        self.decode(id, version.record, None).map(Some)
    }
}
