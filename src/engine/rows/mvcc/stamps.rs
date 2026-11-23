//! \[DESIGN\] A commit in a table, and the stamping of its versions after
//! (RULES.md D29, D440). Commit stores the commit timestamp in the store's
//! status table (`status.rs`), which makes every version the transaction
//! wrote committed, and files what they superseded as garbage, without
//! touching the tree. The versions still name the transaction; each is
//! stamped with the timestamp once the transaction's locks are released
//! (`TableStorage::settle`), or by a later collection if that fails, and a
//! writer that replaces one first appends it stamped. A stamp starts at
//! the leaf the row's write left its entry in, with no descent, while that
//! leaf is sure to be in the tree, and changes the entry in place
//! (RULES.md D843). See `mvcc.rs`.

use std::mem;
use std::sync::atomic::Ordering;

use super::lists::{FIXED, decode_change};
use super::{Change, Dead, Maker, Version};
use crate::bytes::{Put, Reader};
use crate::engine::rows::version::stamp_in_place;
use crate::engine::rows::{RowTable, key};
use crate::engine::tree::Placed;
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

/// A row whose newest version a commit is to stamp, and where its write
/// left its entry.
pub(super) struct Stamp(RowId, Placed);

impl ListItem for Stamp {
    fn bytes(&self) -> usize {
        size_of::<Stamp>()
    }
}

/// A stamp as a record: the row id, the leaf and the tree's stamp, each
/// a u64.
fn encode_stamp(stamp: &Stamp) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.put_u64(stamp.0);
    out.put_u64(stamp.1.leaf);
    out.put_u64(stamp.1.stamp);
    out
}

fn decode_stamp(record: &[u8]) -> Result<Stamp> {
    let mut r = Reader::new(record, "row to stamp");
    let id = r.u64()?;
    let placed = Placed { leaf: r.u64()?, stamp: r.u64()? };
    r.finish()?;
    Ok(Stamp(id, placed))
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
    /// superseded made garbage for when `at` is past the horizon, filed
    /// once while it is within the bytes held (RULES.md D870); the rows it
    /// wrote first are left to stamp.
    pub(in crate::engine::rows) fn commit_open(&self, tx: TxId, at: Timestamp) {
        let removed = self.versions.open.remove(tx);
        let Some(open) = removed else { return };
        let (changes, adopted) = {
            let mut open = lock_builtin(&open);
            (mem::take(&mut open.changes), mem::take(&mut open.adopted))
        };
        let count = u64::try_from(changes.len()).unwrap_or(u64::MAX);
        if count == 0 {
            // It changed nothing here: no list, nor the work areas (RULES.md D783).
            return self.versions.statuses.commit(tx, at, false);
        }
        let held = self.versions.held();
        let (mut stamps, mut lost) = (SpillList::new(self.versions.space_within(changes.len().saturating_mul(size_of::<Stamp>()))), false);
        // The garbage gathered, its bytes, and whether some was filed early.
        let (mut dead, mut bytes, mut filed) = (Vec::new(), 0_usize, false);
        for segment in changes.into_segments(false, &decode_change) {
            let Ok(segment) = segment else {
                lost = true;
                continue;
            };
            // A group's making is stamped with its groups (`frozen/stamped.rs`).
            for change in segment.into_iter().filter(|c| !c.group) {
                // A row's first change by it replaced a committed version or
                // none; its later ones replaced its own.
                if !change.made && change.before.is_none() {
                    lost |= stamps.push(Stamp(change.id, change.placed), &mut encode_stamp).is_err();
                }
                bytes = bytes.saturating_add(self.committed(change, &adopted, &mut dead));
            }
            if bytes > held {
                self.add_garbage(at, mem::take(&mut dead), usize::MAX);
                (bytes, filed) = (0, true);
            }
        }
        // Its deletions are filed with it once stamped (`stamp_all`).
        let later = if filed { usize::MAX } else { stamps.len().saturating_mul(FIXED) };
        self.add_garbage(at, dead, later);
        let rows = lost || !stamps.is_empty();
        // Counted before it is queued, so a collection stamping it counts
        // it down after.
        self.versions.statuses.commit(tx, at, rows);
        if rows {
            self.queue(Unstamped { tx, at, rows: (!lost).then_some(stamps) });
        }
        self.gathered.changed(count);
    }

    /// Queues `commit`, its versions here not all stamped, and counts the
    /// queue, under its lock.
    fn queue(&self, commit: Unstamped) {
        let mut unstamped = lock_builtin(&self.versions.unstamped);
        unstamped.push(commit);
        self.versions.queued.store(unstamped.len(), Ordering::SeqCst);
    }

    /// Adds to `dead` the garbage `change` leaves: its bytes. An object
    /// made for no version it wrote, as for an attempt that failed, is no
    /// one's, and freed now.
    fn committed(&self, change: Change, adopted: &Xxh3Set<u64>, dead: &mut Vec<Dead>) -> usize {
        if change.made {
            if !adopted.contains(&change.id) {
                self.free_objects(&[change.id]);
            }
            return 0;
        }
        let before = dead.len();
        if !change.dropped.is_empty() {
            dead.push(Dead::Objects(change.dropped));
        }
        // A version superseded leaves its zone not visible until it is
        // collected, with its index entries (`rows/zones.rs`).
        match (change.before, change.appended) {
            (Some(own), _) => {
                self.zones.superseded(change.id, true);
                dead.push(Dead::Own(change.id, own, change.unchanged));
            }
            (None, Some(appended)) => {
                self.zones.superseded(change.id, true);
                dead.push(Dead::Older(change.id, appended, change.unchanged));
            }
            (None, None) => {}
        }
        dead.iter().skip(before).map(ListItem::bytes).fold(0, usize::saturating_add)
    }

    /// Stamps the versions `tx` committed here, its locks released. With
    /// no commit queued in the table, as when it only read here, the
    /// queue's lock is not taken (RULES.md D870).
    pub(in crate::engine::rows) fn settle_open(&self, tx: TxId) {
        if self.versions.queued.load(Ordering::SeqCst) == 0 {
            return;
        }
        let mine: Vec<Unstamped> = {
            let mut unstamped = lock_builtin(&self.versions.unstamped);
            let mine = unstamped.extract_if(.., |u| u.tx == tx).collect();
            self.versions.queued.store(unstamped.len(), Ordering::SeqCst);
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
            let late = unstamped.extract_if(.., |u| u.rows.is_some()).collect();
            self.versions.queued.store(unstamped.len(), Ordering::SeqCst);
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
            self.queue(Unstamped { tx, at, rows: None });
            return;
        };
        let mut left = SpillList::new(self.versions.space_within(rows.len().saturating_mul(size_of::<Stamp>())));
        let (mut lost, mut dead) = (false, Vec::new());
        let mut unread = false;
        let mut note = |id, stamped: Result<bool>| match stamped {
            Ok(true) => dead.push(Dead::Deletion(id)),
            Ok(false) => {}
            Err(_) => lost |= left.push(Stamp(id, Placed::default()), &mut encode_stamp).is_err(),
        };
        for segment in rows.into_segments(false, &decode_stamp) {
            let Ok(mut segment) = segment else {
                unread = true;
                continue;
            };
            // [DESIGN] RULES.md D924: the rows a leaf holds one after
            // another, so each leaf is latched once for them all.
            segment.sort_unstable_by_key(|Stamp(id, placed)| (placed.stamp, placed.leaf, *id));
            let mut from = 0;
            while let Some(Stamp(_, placed)) = segment.get(from) {
                let rest = segment.get(from..).unwrap_or_default();
                let run = rest.get(..rest.iter().take_while(|s| s.1 == *placed).count()).unwrap_or_default();
                self.stamp_run(tx, at, run, &mut note);
                from = from.saturating_add(run.len().max(1));
            }
        }
        lost |= unread;
        if !dead.is_empty() {
            self.add_garbage(at, dead, 0);
        }
        if lost || !left.is_empty() {
            self.queue(Unstamped { tx, at, rows: (!lost).then_some(left) });
            return;
        }
        self.versions.statuses.stamped(tx, at);
    }

    /// \[DESIGN\] Stamps the rows of `run`, ascending, each left by its
    /// write in the one leaf, with the leaf latched once for those it
    /// still holds (`BLinkTree::change_placed_run`, RULES.md D924), and
    /// each of the others as `stamp_placed` does; `note` is told of each
    /// whether it is a deletion, or that it could not be stamped.
    fn stamp_run(&self, tx: TxId, at: Timestamp, run: &[Stamp], note: &mut dyn FnMut(RowId, Result<bool>)) {
        let mut done = 0_usize;
        if let (Some(Stamp(_, placed)), true) = (run.first(), run.len() > 1) {
            let keys: Vec<_> = run.iter().map(|Stamp(id, _)| key(*id)).collect();
            // Counted as they are noted: a failure of the leaf past some
            // leaves those stamped, and the rest to be stamped alone.
            let _ = self.tree.change_placed_run(*placed, &keys, |n, bytes| {
                if let Some(Stamp(id, _)) = run.get(n) {
                    note(*id, stamp_in_place(bytes, tx, at).map(|stamped| stamped.unwrap_or(false)));
                    done = done.saturating_add(1);
                }
                Ok(())
            });
        }
        for Stamp(id, placed) in run.iter().skip(done) {
            note(*id, self.stamp_placed(tx, (*id, *placed), at));
        }
    }

    /// As `stamp`, starting at the leaf `placed` names: in place, with no
    /// descent, unless the leaf may have left the tree or no longer holds
    /// the row (RULES.md D843).
    fn stamp_placed(&self, tx: TxId, (id, placed): (RowId, Placed), at: Timestamp) -> Result<bool> {
        let k = key(id);
        match self.tree.change_placed(placed, &k, |bytes| stamp_in_place(bytes, tx, at))? {
            Some(stamped) => Ok(stamped.unwrap_or(false)),
            None => self.stamp(tx, id, at),
        }
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
pub(super) mod tests;
