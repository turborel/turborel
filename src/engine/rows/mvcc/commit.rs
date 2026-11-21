//! Validation of a transaction in a table (RULES.md D29, D38): the
//! two-sided check of its reads and writes against those validated before
//! and after it. Its commit is `stamps.rs`'s. See `mvcc.rs`.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::decode_change;
use super::reads::Changed;
use super::{Maker, Validated, Version};
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
}
