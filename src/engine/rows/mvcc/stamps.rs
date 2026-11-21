//! \[DESIGN\] A commit in a table, and the stamping of its versions after
//! (RULES.md D29, D440). Commit stores the commit timestamp in the store's
//! status table (`status.rs`), which makes every version the transaction
//! wrote committed, and files what they superseded as garbage, without
//! touching the tree. The versions still name the transaction; each is
//! stamped with the timestamp once the transaction's locks are released
//! (`TableStorage::settle`), or by a later collection if that fails, and a
//! writer that replaces one first appends it stamped. See `mvcc.rs`.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::decode_change;
use super::{Change, Dead, Maker, Version};
use crate::bytes::{Put, Reader};
use crate::engine::rows::{RowTable, key};
use crate::error::Result;
use crate::hash::xxhash::Xxh3Set;
use crate::spill::{ListItem, SpillList};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::txn::{Timestamp, TxId};

/// Tries of a stamp that finds its row's entry changed as it writes:
/// each change is a write or a freeze over the row, after which the stamp
/// is not needed or finds the entry again.
const STAMP_TRIES: usize = 8;

/// A row whose newest version a commit is to stamp.
pub(super) struct Stamp(RowId);

impl ListItem for Stamp {
    fn bytes(&self) -> usize {
        size_of::<RowId>()
    }
}

fn encode_stamp(stamp: &Stamp) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u64(stamp.0);
    out
}

fn decode_stamp(record: &[u8]) -> Result<Stamp> {
    let mut r = Reader::new(record, "row to stamp");
    let id = r.u64()?;
    r.finish()?;
    Ok(Stamp(id))
}

/// A commit whose versions in a table are not all stamped: its
/// transaction, its timestamp, and the rows left, none if its changes could
/// not be read back, so it is never stamped and its status kept.
pub(super) struct Unstamped {
    tx: TxId,
    at: Timestamp,
    rows: Option<SpillList<Stamp>>,
}

impl RowTable {
    /// Commits `tx` at `at` here: its status stored, and what its versions
    /// superseded made garbage for when `at` is past the horizon, a segment
    /// of its changes at a time; the rows it wrote first are left to stamp.
    pub(in crate::engine::rows) fn commit_open(&self, tx: TxId, at: Timestamp) {
        let removed = self.versions.open.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&tx);
        let Some(open) = removed else { return };
        let (changes, adopted) = {
            let mut open = lock_builtin(&open);
            open.ended = true;
            (mem::take(&mut open.changes), mem::take(&mut open.adopted))
        };
        let count = u64::try_from(changes.len()).unwrap_or(u64::MAX);
        let (mut stamps, mut lost) = (SpillList::new(self.versions.space()), false);
        for segment in changes.into_segments(false, &decode_change) {
            let Ok(segment) = segment else {
                lost = true;
                continue;
            };
            let mut dead = Vec::new();
            for change in segment {
                // A row's first change by it replaced a committed version or
                // none; its later ones replaced its own.
                if !change.made && change.before.is_none() {
                    lost |= stamps.push(Stamp(change.id), &mut encode_stamp).is_err();
                }
                self.committed(change, &adopted, &mut dead);
            }
            self.add_garbage(at, dead);
        }
        let rows = lost || !stamps.is_empty();
        // Counted before it is queued, so a collection stamping it counts
        // it down after.
        self.versions.statuses.commit(tx, at, rows);
        if rows {
            lock_builtin(&self.versions.unstamped).push(Unstamped { tx, at, rows: (!lost).then_some(stamps) });
        }
        if count > 0 {
            self.gathered.changed(count);
        }
    }

    /// Adds to `dead` the garbage `change` leaves. An object made for no
    /// version it wrote, as for an attempt that failed, is no one's, and
    /// freed now.
    fn committed(&self, change: Change, adopted: &Xxh3Set<u64>, dead: &mut Vec<Dead>) {
        if change.made {
            if !adopted.contains(&change.id) {
                self.free_objects(&[change.id]);
            }
            return;
        }
        if !change.dropped.is_empty() {
            dead.push(Dead::Objects(change.dropped));
        }
        // A version superseded leaves its zone not visible until it is
        // collected, with its index entries (`rows/zones.rs`).
        match (change.before, change.appended) {
            (Some(own), _) => {
                self.zones.superseded(change.id, true);
                dead.push(Dead::Own(change.id, own));
            }
            (None, Some(appended)) => {
                self.zones.superseded(change.id, true);
                dead.push(Dead::Older(change.id, appended));
            }
            (None, None) => {}
        }
    }

    /// Stamps the versions `tx` committed here, its locks released.
    pub(in crate::engine::rows) fn settle_open(&self, tx: TxId) {
        let mine: Vec<Unstamped> = {
            let mut unstamped = lock_builtin(&self.versions.unstamped);
            let (mine, others) = mem::take(&mut *unstamped).into_iter().partition(|u| u.tx == tx);
            *unstamped = others;
            mine
        };
        for commit in mine {
            self.stamp_all(commit);
        }
    }

    /// Stamps what commits have left: their own stamping failed, or a
    /// table was committed without being settled.
    pub(super) fn stamp_late(&self) {
        let late: Vec<Unstamped> = {
            let mut unstamped = lock_builtin(&self.versions.unstamped);
            let (late, kept) = mem::take(&mut *unstamped).into_iter().partition(|u| u.rows.is_some());
            *unstamped = kept;
            late
        };
        for commit in late {
            self.stamp_all(commit);
        }
    }

    /// Stamps the rows of `commit`, a deletion's row made garbage to leave
    /// the tree; once all are, its status is told so. Those that cannot
    /// be are queued again.
    fn stamp_all(&self, commit: Unstamped) {
        let Unstamped { tx, at, rows } = commit;
        let Some(rows) = rows else {
            lock_builtin(&self.versions.unstamped).push(Unstamped { tx, at, rows: None });
            return;
        };
        let mut left = SpillList::new(self.versions.space());
        let (mut lost, mut dead) = (false, Vec::new());
        for segment in rows.into_segments(false, &decode_stamp) {
            let Ok(segment) = segment else {
                lost = true;
                continue;
            };
            for Stamp(id) in segment {
                match self.stamp(tx, id, at) {
                    Ok(true) => dead.push(Dead::Deletion(id)),
                    Ok(false) => {}
                    Err(_) => lost |= left.push(Stamp(id), &mut encode_stamp).is_err(),
                }
            }
        }
        if !dead.is_empty() {
            self.add_garbage(at, dead);
        }
        if lost || !left.is_empty() {
            lock_builtin(&self.versions.unstamped).push(Unstamped { tx, at, rows: (!lost).then_some(left) });
            return;
        }
        self.versions.statuses.stamped(tx, at);
    }

    /// Stamps the newest version of row `id`, made by `tx`, committed at
    /// `at`, unless it is no longer the newest: whether it is a deletion.
    /// The entry is replaced only as read, so a writer that has written over
    /// the version meanwhile, which appended it stamped, keeps its own.
    pub(super) fn stamp(&self, tx: TxId, id: RowId, at: Timestamp) -> Result<bool> {
        let k = key(id);
        for _ in 0..STAMP_TRIES {
            let Some(newest) = self.tree.get(&k)? else { return Ok(false) };
            let version = Version::parse(&newest)?;
            if version.maker != Maker::Open(tx) {
                return Ok(false);
            }
            if self.tree.replace_if(&k, &newest, &Version { maker: Maker::Committed(at), ..version }.encode())? {
                return Ok(version.deleted);
            }
        }
        Err(super::conflict())
    }

    /// Whether commits here have versions left to stamp.
    pub(super) fn stamps_left(&self) -> bool {
        !lock_builtin(&self.versions.unstamped).is_empty()
    }

    /// From now a transaction of this table reads at `snapshot`, which the
    /// status table keeps the latest of.
    pub(super) fn seen(&self, snapshot: Timestamp) {
        self.versions.statuses.seen(snapshot);
    }

    /// The table is collected at `horizon`.
    pub(super) fn collected_at(&self, horizon: Timestamp) {
        self.versions.collected.fetch_max(horizon, Ordering::SeqCst);
        self.versions.statuses.collected(horizon);
        self.versions.statuses.sweep();
    }
}

// Unit tests: commits read through the status table before their stamps,
// and versions appended stamped (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
