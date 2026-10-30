//! \[DESIGN\] Freezing a table's rows into column groups (RULES.md D31), on
//! the database's maintenance thread (`TableStorage::maintain`), so no
//! transaction's end waits for it: when COLUMNSTORE.FREEZE_ROWS rows past
//! its last group are committed before every snapshot, and
//! COLUMNSTORE.FREEZE_DELAY seconds ago, a group of up to
//! COLUMNSTORE.GROUP_ROWS of them is written, published in a new directory,
//! and their entries taken out of the tree once no reader holds a
//! directory from before it. Split from frozen.rs (RULES.md R-SIZE-2).
//!
//! Only a version stamped committed is frozen; its entry is taken out
//! only if it is still the one frozen, so a row changed meanwhile keeps
//! its newer version in the tree, which readers take before the group's.

use std::cmp;
use std::collections::VecDeque;
use std::mem;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Directory, Entry, Retiring};
use crate::engine::columns::group::Group;
use crate::engine::rows::options::{Columns, Places};
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::sync::{lock_builtin, read, write};
use crate::txn::Timestamp;
use crate::value::sort_cmp;

/// \[DESIGN\] How long a freeze that found too few rows waits before it is
/// tried again: a second.
const RETRY: Duration = Duration::from_secs(1);
/// \[DESIGN\] The most groups one call freezes, so a checkpoint, which
/// waits for it, is not held long.
const MOST_GROUPS: usize = 4;
/// Commits noted within each 64th of the delay are taken as the first.
const STEPS: u32 = 64;

/// Notes commit `at` in `commits`, for a delay of `delay` seconds: at most
/// one in each 64th of it, a later commit then taken as earlier, which
/// only delays its freeze.
pub(super) fn noted(commits: &mut VecDeque<(Timestamp, Instant)>, at: Timestamp, delay: u32) {
    let now = Instant::now();
    let step = Duration::from_secs(u64::from(delay)).checked_div(STEPS).unwrap_or_default();
    if commits.back().is_some_and(|&(_, when)| now.saturating_duration_since(when) < step) {
        return;
    }
    commits.push_back((at, now));
}

/// The latest commit noted at least `delay` seconds ago, 0 if none,
/// forgetting those before it.
fn delayed(commits: &mut VecDeque<(Timestamp, Instant)>, delay: u32) -> Timestamp {
    let Some(cut) = Instant::now().checked_sub(Duration::from_secs(u64::from(delay))) else { return 0 };
    while commits.get(1).is_some_and(|&(_, when)| when <= cut) {
        commits.pop_front();
    }
    commits.front().filter(|&&(_, when)| when <= cut).map_or(0, |&(at, _)| at)
}

/// `rows`, each with its id, sorted by the columns of `order`, ascending,
/// as ORDER BY sorts them, their ids breaking ties: their ids and rows.
pub(super) fn ordered(order: &Places, mut rows: Vec<(RowId, Row)>) -> (Vec<RowId>, Vec<Row>) {
    if !order.places().is_empty() {
        rows.sort_by(|(a_id, a), (b_id, b)| {
            let by = |&c: &usize| a.get(c).zip(b.get(c)).map_or(cmp::Ordering::Equal, |(x, y)| sort_cmp(x, y));
            order.places().iter().map(by).find(|o| o.is_ne()).unwrap_or_else(|| a_id.cmp(b_id))
        });
    }
    rows.into_iter().unzip()
}

/// A freeze under way, noted from a row id on until it is dropped.
pub(super) struct Freezing<'f>(&'f AtomicU64);

impl<'f> Freezing<'f> {
    /// Notes in `from` a freeze under way of the rows from `first` on.
    pub(super) fn from(from: &'f AtomicU64, first: RowId) -> Freezing<'f> {
        from.store(first, Ordering::SeqCst);
        Freezing(from)
    }
}

impl Drop for Freezing<'_> {
    fn drop(&mut self) {
        self.0.store(super::NOT_FREEZING, Ordering::SeqCst);
    }
}

impl RowTable {
    /// Whether a freeze, a rewrite or entries to take out may be due: rows
    /// enough past the last group, and none found too few a moment ago;
    /// frozen rows enough given entries; a group sorted by other columns
    /// than COLUMNSTORE.ORDER names now, or keeping Bloom filters of others
    /// than COLUMNSTORE.BLOOM names; or rows frozen whose entries remain.
    pub(in crate::engine::rows) fn freeze_wanted(&self) -> bool {
        let Ok(Some(columns)) = read(&self.frozen.columns).map(|c| *c) else { return false };
        let frozen = self.frozen.directory().map_or(0, |d| d.rows());
        let live = u64::try_from(self.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let waited = lock_builtin(&self.frozen.tried).is_none_or(|at| at.elapsed() >= RETRY);
        let shadowed = usize::try_from(self.frozen.shadowed.load(Ordering::Acquire)).unwrap_or(usize::MAX);
        (waited && live.saturating_sub(frozen) >= columns.due())
            || super::rewrite::due(shadowed, columns.group_rows, &columns)
            || self.frozen.directory().is_ok_and(|d| super::rewrite::stale(&d.groups, &columns))
            || !lock_builtin(&self.frozen.retiring).is_empty()
    }

    /// Freezes the rows due, if any, committed at or before `horizon`, and
    /// writes again the groups due (`rewrite.rs`); and takes out the
    /// entries of rows frozen before that no reader needs: whether it froze
    /// or wrote a group.
    pub(in crate::engine::rows) fn freeze_due(&self, horizon: Timestamp) -> Result<bool> {
        let Some(columns) = *read(&self.frozen.columns)? else { return Ok(false) };
        let Ok(_one) = self.frozen.freezing.try_lock() else { return Ok(false) };
        // Not while a checkpoint is captured (`TableFiles::freezes`).
        let _capturing = read(&self.freezes)?;
        self.retire_unread()?;
        let cutoff = match columns.freeze_delay {
            0 => horizon,
            delay => horizon.min(delayed(&mut lock_builtin(&self.frozen.commits), delay)),
        };
        let froze = self.freeze_rows(columns, cutoff)?;
        Ok(self.rewrite_due(columns, cutoff)? || froze)
    }

    /// Freezes rows committed at or before `cutoff` past the last group, if
    /// as many as `columns` says are: whether it froze any.
    fn freeze_rows(&self, columns: Columns, cutoff: Timestamp) -> Result<bool> {
        let due = columns.due();
        let frozen = self.frozen.directory()?.rows();
        let live = u64::try_from(self.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let waited = lock_builtin(&self.frozen.tried).is_none_or(|at| at.elapsed() >= RETRY);
        if live.saturating_sub(frozen) < due || !waited {
            return Ok(false);
        }
        let fewest = usize::try_from(due.min(u64::from(columns.group_rows))).unwrap_or(usize::MAX);
        let most = usize::try_from(columns.group_rows).unwrap_or(usize::MAX);
        // Noted before a row is read: a deletion committed after it was,
        // and collected before its group is published, keeps its entry, so
        // the row is not seen again (`mvcc/gc.rs`).
        let _noted = Freezing::from(&self.frozen.freezing_from, self.frozen.directory()?.last().map_or(0, |l| l.saturating_add(1)));
        let mut froze = false;
        for _ in 0..MOST_GROUPS {
            let entries = self.eligible(self.frozen.directory()?.last(), cutoff, most)?;
            if entries.len() < fewest {
                break;
            }
            self.freeze(entries, &columns)?;
            froze = true;
        }
        *lock_builtin(&self.frozen.tried) = (!froze).then(Instant::now);
        Ok(froze)
    }

    /// The entries of the tree after row `after`, up to `most` of them,
    /// whose newest version is a row stamped committed at or before
    /// `cutoff`.
    fn eligible(&self, after: Option<RowId>, cutoff: Timestamp, most: usize) -> Result<Vec<(RowId, Vec<u8>)>> {
        let from = after.map(key);
        let lower = from.as_ref().map_or(Bound::Unbounded, |k| Bound::Excluded(&k[..]));
        let mut found = Vec::new();
        for entry in self.tree.scan(lower, Bound::Unbounded, Direction::Forward) {
            let (k, bytes) = entry?;
            let version = Version::parse(&bytes)?;
            if let (Maker::Committed(at), false) = (version.maker, version.deleted)
                && at <= cutoff
            {
                found.push((row_id(&k)?, bytes));
                if found.len() >= most {
                    break;
                }
            }
        }
        Ok(found)
    }

    /// Writes `entries`' rows as a group, sorted, and with Bloom filters,
    /// as `columns` says, and publishes it.
    fn freeze(&self, entries: Vec<(RowId, Vec<u8>)>, columns: &Columns) -> Result<()> {
        let mut rows: Vec<(RowId, Row)> = Vec::with_capacity(entries.len());
        for (id, bytes) in &entries {
            rows.push((*id, self.decode(*id, Version::parse(bytes)?.record, None)?));
        }
        let (ids, rows) = ordered(&columns.order, rows);
        let places = (columns.order.places(), columns.bloom.places());
        let group = Group::write(&self.group_pages()?, self.format.types(), (&ids, &rows), places)?;
        self.publish((None, Some(group)), entries)
    }

    /// Puts `group`, if any, in the directory in the place of `old`, if
    /// any, the directory stored, and the tree's `entries` of its rows to
    /// be taken out; `old`'s pages freed once no reader holds it.
    pub(super) fn publish(&self, (old, group): (Option<&Arc<Entry>>, Option<Group>), entries: Vec<(RowId, Vec<u8>)>) -> Result<()> {
        let old_pages = old.map(|o| self.group_of(o).map(|g| g.pages())).transpose()?;
        let made_pages = group.as_ref().map(Group::pages);
        let made = group.map(|g| self.entry_of(g)).transpose()?;
        let before = {
            let mut held = write(&self.frozen.directory)?;
            let mut groups: Vec<Arc<Entry>> = held.groups.iter().filter(|e| old.is_none_or(|o| !Arc::ptr_eq(e, o))).cloned().collect();
            groups.extend(made.iter().cloned());
            groups.sort_by_key(|e| e.summary.first);
            let next = Arc::new(Directory { groups });
            if let Err(e) = self.store_directory(&next) {
                // Freed as it goes: it was never published.
                if let (Some(m), Some(pages)) = (&made, made_pages) {
                    m.retire(pages);
                }
                return Err(e);
            }
            if let (Some(o), Some(pages)) = (old, old_pages) {
                o.retire(pages);
            }
            mem::replace(&mut *held, next)
        };
        lock_builtin(&self.frozen.retiring).push(Retiring { before: Arc::downgrade(&before), entries });
        drop(before);
        self.retire_unread()
    }

    /// Takes out the tree's entries of rows frozen, in the order frozen,
    /// once no reader holds a directory from before their group; each only
    /// if it is still the version frozen.
    pub(in crate::engine::rows) fn retire_unread(&self) -> Result<()> {
        let ready = {
            let mut retiring = lock_builtin(&self.frozen.retiring);
            let unread = retiring.iter().take_while(|r| r.before.strong_count() == 0).count();
            retiring.drain(..unread).collect::<Vec<_>>()
        };
        for retired in ready {
            for (id, bytes) in retired.entries {
                if self.tree.delete_if(&key(id), &bytes)? {
                    self.free_values(id, Version::parse(&bytes)?.record);
                }
            }
        }
        Ok(())
    }
}
