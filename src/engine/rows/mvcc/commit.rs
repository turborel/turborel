//! Validation of a transaction in a table (RULES.md D29, D38): the
//! two-sided check of its reads and writes against those validated before
//! and after it. Its commit is `stamps.rs`'s. See `mvcc.rs`.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::decode_change;
use super::reads::{EVERY_COLUMN, changed_columns};
use super::{Images, Maker, Reads, Validated, Version};
use crate::engine::rows::undo::Held;
use crate::engine::rows::{RowTable, key};
use crate::error::{Code, Result, SqlError};
use crate::hash::xxhash::Xxh3Set;
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
            // RULES.md D448: with no change and no read recorded here it is
            // ordered against none, so it is not kept, nor the lock every
            // commit of the table takes taken.
            if open.changes.is_empty() && !open.reads.any() {
                return Ok(());
            }
            let wrote_many = open.changes.len() > self.versions.tracked();
            let mut writes = Xxh3Set::default();
            if !wrote_many {
                open.changes.visit(&decode_change, &mut |c| {
                    writes.extend((!c.made).then_some(c.id));
                    Ok(())
                })?;
            }
            let snapshot = open.reads_since.unwrap_or(open.snapshot);
            Validated { tx, snapshot, at, reads: mem::take(&mut open.reads), writes, wrote_many, images: None }
        };
        let mut validated = lock_builtin(&self.versions.validated);
        // RULES.md D448: with no reads here nor in any kept, no order is
        // missed either way. Either order missed needs the other's commit
        // after this one's snapshot, so only those are walked.
        let others: &mut [Validated] = if me.reads.any() || validated.readers > 0 { validated.after(me.snapshot) } else { &mut [] };
        for other in others.iter_mut().filter(|v| v.tx != tx) {
            // It committed between this one's snapshot and its commit, and
            // wrote what this one read: this one missed it.
            let missed_other = me.snapshot < other.at && other.at < at && self.wrote_any(other, (&me.reads, &me.writes))?;
            // It commits after this one but read before it, what this one
            // writes: it missed this one.
            let missed_me = other.snapshot < at && at < other.at && self.wrote_any(&mut me, (&other.reads, &other.writes))?;
            if missed_other || missed_me {
                return Err(SqlError::new(
                    Code::Conflict,
                    "[DESIGN] the transaction read rows another transaction changed meanwhile, or changed rows another read: it is not serializable; run it again",
                ));
            }
        }
        // [DESIGN] RULES.md D795: written only when it moves back.
        if at < self.versions.validated_due.load(Ordering::SeqCst) {
            self.versions.validated_due.fetch_min(at, Ordering::SeqCst);
        }
        validated.push(me);
        Ok(())
    }

    /// Whether `reads` consumed a column of a row `v` wrote that its change
    /// changed, read by id or with the whole table, or a condition or
    /// region read matches the row's version before or after its change,
    /// though the reader never saw it (a phantom). Its images are decoded
    /// only when the columns read do not answer (RULES.md D444, D448): its
    /// versions are still there, as the reader, open with a snapshot
    /// before its commit, holds the horizon before it.
    ///
    /// A row the reader wrote itself, `own`, is no read of `v`'s change
    /// (RULES.md D455): the reader held it from its write to its commit,
    /// so `v` changed it before that write, which the write judged. At
    /// one snapshot it was refused; at READ COMMITTED it was made over
    /// `v`'s version, as the reader's own end-of-statement checks read it.
    fn wrote_any(&self, v: &mut Validated, (reads, own): (&Reads, &Xxh3Set<RowId>)) -> Result<bool> {
        if v.wrote_many {
            return Ok(reads.any());
        }
        if v.writes.is_empty() || !reads.any() {
            return Ok(false);
        }
        let mut by_id = reads.rows.iter().filter(|(id, _)| v.writes.contains(id) && !own.contains(id));
        if reads.all == Some(EVERY_COLUMN) || by_id.clone().any(|(_, &columns)| columns == EVERY_COLUMN) {
            return Ok(true);
        }
        for (&id, &columns) in &mut by_id {
            let [made, before] = self.changed_by(id, (v.tx, v.at))?;
            if changed_columns(before.as_ref(), made.as_ref()) & columns != 0 {
                return Ok(true);
            }
        }
        if reads.all.is_none() && reads.conditions.is_empty() && reads.regions.is_empty() {
            return Ok(false);
        }
        let images = self.images(v)?;
        Ok(reads.all.is_some_and(|columns| columns & images.columns != 0)
            || images.rows.matched_by(&reads.conditions, images.columns, &reads.regions))
    }

    /// `v`'s images (`Images`), decoded the first time they are asked for.
    fn images<'v>(&self, v: &'v mut Validated) -> Result<&'v Images> {
        if let Some(images) = v.images.take() {
            return Ok(v.images.insert(images));
        }
        let held = self.versions.held();
        let mut images = Images::default();
        for &id in &v.writes {
            let [made, before] = self.changed_by(id, (v.tx, v.at))?;
            images.columns |= changed_columns(before.as_ref(), made.as_ref());
            for row in [made, before].into_iter().flatten() {
                images.rows.add(row, held);
            }
        }
        Ok(v.images.insert(images))
    }

    /// The rows of the version of row `id` made by transaction `tx`, open
    /// or committed at `at`, and of the version before it, unless a
    /// deletion or none: a condition matching either was answered
    /// differently by the change. Found going back from the newest; one not
    /// found is an error, which refuses the transaction validated.
    fn changed_by(&self, id: RowId, (tx, at): (TxId, Timestamp)) -> Result<[Option<Row>; 2]> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok([None, None]) };
        let mut bytes = Held::from(newest);
        loop {
            let version = Version::parse(&bytes)?;
            let maker = self.maker(&version);
            if maker == Maker::Open(tx) || maker == Maker::Committed(at) {
                let made = self.row_of(id, &version)?;
                let lost = |e| self.lost(e, &bytes, super::gone::Walker::Validating(tx));
                let before = version.older.map(|older| self.undo.read(older).map_err(lost)).transpose()?;
                let before = before.map(|b| Version::parse(&b).and_then(|v| self.row_of(id, &v))).transpose()?.flatten();
                return Ok([made, before]);
            }
            let Some(older) = version.older else {
                return Err(SqlError::new(
                    Code::Conflict,
                    format!("[DESIGN] the version row {id} had at {at} is gone; run the transaction again"),
                ));
            };
            bytes = self.undo.read(older).map_err(|e| self.lost(e, &bytes, super::gone::Walker::Validating(tx)))?;
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

/// \[DESIGN\] The transactions validated in a table that one could still
/// be ordered against, in the order of their commits, and how many of them
/// read anything: while none did, one that read nothing is checked against
/// none (RULES.md D448). Kept in that order, a validation walks only those
/// committed after its snapshot, and a collection drops those at or before
/// its horizon from the front, however many a lagging horizon keeps.
#[derive(Default)]
pub(super) struct Certified {
    entries: Vec<Validated>,
    pub(super) readers: usize,
}

impl Certified {
    /// Adds `v` in its commit's place: after every one kept committed
    /// before it, which a validation out of commit order may follow.
    pub(super) fn push(&mut self, v: Validated) {
        self.readers = self.readers.saturating_add(usize::from(v.reads.any()));
        let place = self.entries.partition_point(|kept| kept.at <= v.at);
        self.entries.insert(place, v);
    }

    /// Those committed after `snapshot`.
    pub(super) fn after(&mut self, snapshot: Timestamp) -> &mut [Validated] {
        let from = self.entries.partition_point(|kept| kept.at <= snapshot);
        self.entries.get_mut(from..).unwrap_or_default()
    }

    /// Keeps those `keep` says.
    pub(super) fn retain(&mut self, keep: impl FnMut(&Validated) -> bool) {
        self.entries.retain(keep);
        self.readers = self.entries.iter().filter(|v| v.reads.any()).count();
    }

    /// Drops those committed at or before `horizon`: no transaction open
    /// or to come has a snapshot before them.
    pub(super) fn drop_through(&mut self, horizon: Timestamp) {
        let upto = self.entries.partition_point(|kept| kept.at <= horizon);
        let dropped = self.entries.drain(..upto).filter(|v| v.reads.any()).count();
        self.readers = self.readers.saturating_sub(dropped);
    }

    /// Whether none is kept.
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The earliest commit of one kept.
    pub(super) fn earliest(&self) -> Option<Timestamp> {
        self.entries.first().map(|v| v.at)
    }
}
