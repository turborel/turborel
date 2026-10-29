//! \[DESIGN\] Freezing a table's rows into column groups (RULES.md D31), as
//! a transaction's end collects its garbage (`TableStorage::gc`): when
//! COLUMNS.FREEZE_ROWS rows past its last group are committed before every
//! snapshot, and COLUMNS.FREEZE_DELAY seconds ago, a group of up to
//! COLUMNS.GROUP_ROWS of them is written, published in a new directory,
//! and their entries taken out of the tree once no reader holds a
//! directory from before it. Split from frozen.rs (RULES.md R-SIZE-2).
//!
//! Only a version stamped committed is frozen; its entry is taken out
//! only if it is still the one frozen, so a row changed meanwhile keeps
//! its newer version in the tree, which readers take before the group's.

use std::collections::VecDeque;
use std::mem;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::{Directory, Entry, Retiring};
use crate::engine::columns::group::Group;
use crate::engine::rows::options::Columns;
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::sync::{lock_builtin, read, write};
use crate::txn::Timestamp;

/// \[DESIGN\] How long a freeze that found too few rows waits before it is
/// tried again: a second.
const RETRY: Duration = Duration::from_secs(1);
/// \[DESIGN\] The most groups one transaction's end freezes, so it is not
/// held long.
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

impl RowTable {
    /// Freezes the rows due, if any, committed at or before `horizon`, and
    /// writes again the groups due (`rewrite.rs`); and takes out the
    /// entries of rows frozen before that no reader needs.
    pub(in crate::engine::rows) fn freeze_due(&self, horizon: Timestamp) -> Result<()> {
        let Some(columns) = *read(&self.frozen.columns)? else { return Ok(()) };
        let Ok(_one) = self.frozen.freezing.try_lock() else { return Ok(()) };
        self.retire_unread()?;
        let cutoff = match columns.freeze_delay {
            0 => horizon,
            delay => horizon.min(delayed(&mut lock_builtin(&self.frozen.commits), delay)),
        };
        // Not while a checkpoint is captured (`TableFiles::freezes`).
        let _capturing = read(&self.freezes)?;
        self.freeze_rows(columns, cutoff)?;
        self.rewrite_due(columns, cutoff)
    }

    /// Freezes rows committed at or before `cutoff` past the last group, if
    /// as many as `columns` says are.
    fn freeze_rows(&self, columns: Columns, cutoff: Timestamp) -> Result<()> {
        let due = columns.due();
        let frozen = self.frozen.directory()?.rows();
        let live = u64::try_from(self.live.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let waited = lock_builtin(&self.frozen.tried).is_none_or(|at| at.elapsed() >= RETRY);
        if live.saturating_sub(frozen) < due || !waited {
            return Ok(());
        }
        let fewest = usize::try_from(due.min(u64::from(columns.group_rows))).unwrap_or(usize::MAX);
        let most = usize::try_from(columns.group_rows).unwrap_or(usize::MAX);
        let mut froze = false;
        for _ in 0..MOST_GROUPS {
            let entries = self.eligible(self.frozen.directory()?.last(), cutoff, most)?;
            if entries.len() < fewest {
                break;
            }
            self.freeze(entries)?;
            froze = true;
        }
        *lock_builtin(&self.frozen.tried) = (!froze).then(Instant::now);
        Ok(())
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

    /// Writes `entries`' rows as a group and publishes it.
    fn freeze(&self, entries: Vec<(RowId, Vec<u8>)>) -> Result<()> {
        let mut ids = Vec::with_capacity(entries.len());
        let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
        for (id, bytes) in &entries {
            rows.push(self.decode(*id, Version::parse(bytes)?.record, None)?);
            ids.push(*id);
        }
        let group = Group::write(&self.group_pages()?, self.format.types(), &ids, &rows)?;
        self.publish((None, Some(group)), entries)
    }

    /// Puts `group`, if any, in the directory in the place of `old`, if
    /// any, the directory stored, and the tree's `entries` of its rows to
    /// be taken out; `old`'s pages freed once no reader holds it.
    pub(super) fn publish(&self, (old, group): (Option<&Arc<Entry>>, Option<Group>), entries: Vec<(RowId, Vec<u8>)>) -> Result<()> {
        let made = group.map(|g| self.entry(g));
        let before = {
            let mut held = write(&self.frozen.directory)?;
            let mut groups: Vec<Arc<Entry>> = held.groups.iter().filter(|e| old.is_none_or(|o| !Arc::ptr_eq(e, o))).cloned().collect();
            groups.extend(made.iter().cloned());
            groups.sort_by_key(|e| e.group.first);
            let next = Arc::new(Directory { groups });
            if let Err(e) = self.store_directory(&next) {
                // Freed as it goes: it was never published.
                if let Some(m) = &made {
                    m.retired.store(true, Ordering::Release);
                }
                return Err(e);
            }
            if let Some(o) = old {
                o.retired.store(true, Ordering::Release);
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
