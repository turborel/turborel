//! \[DESIGN\] The rows of the versions awaiting collection, read as they
//! are pulled (RULES.md D29, D38): for an index build, the versions it
//! needs entries for, and for a checkpoint, those whose entries recovery
//! takes back. The garbage lists are copied a segment at a time, the lock
//! on them taken for each, so commits adding garbage wait for one segment
//! at most; and each version is read as its row is pulled. The reader's
//! caller holds collection of the table back meanwhile (`TableLock`), so
//! no list it reads is removed.

use std::collections::VecDeque;
use std::sync::Arc;

use super::Dead;
use super::lists::decode_dead;
use crate::engine::rows::RowTable;
use crate::engine::rows::frozen::{Merged, Merging};
use crate::engine::rows::ids::row_id;
use crate::error::Result;
use crate::hash::xxhash::Xxh3Set;
use crate::storage::{Rows, Superseded, SupersededRows, shares_entry};
use crate::sync::lock_builtin;
use crate::txn::Timestamp;

/// Which versions a reader takes: given when each became garbage, and
/// whether its transaction made it itself.
type Keep = Box<dyn Fn(Timestamp, bool) -> bool + Send>;

/// The reader: the timestamps of the garbage lists it reads, where it is
/// among them, and the versions of the segment last copied.
struct Pending {
    table: Arc<RowTable>,
    keep: Keep,
    columns: Option<Vec<usize>>,
    timestamps: Vec<Timestamp>,
    at: usize,
    segment: usize,
    buffer: VecDeque<Dead>,
}

impl RowTable {
    /// The rows of the versions awaiting collection that `keep` takes, as
    /// they are pulled, `columns` decoded, or all, each with the columns
    /// the change over it kept.
    pub(in crate::engine::rows) fn pending(self: &Arc<Self>, keep: Keep, columns: Option<&[usize]>) -> SupersededRows {
        let timestamps = lock_builtin(&self.versions.garbage).keys().copied().collect();
        let columns = columns.map(<[usize]>::to_vec);
        Box::new(Pending { table: Arc::clone(self), keep, columns, timestamps, at: 0, segment: 0, buffer: VecDeque::new() })
    }
}

impl RowTable {
    /// `TableStorage::versions`: the versions awaiting collection no
    /// reader reads, the frozen rows, and each row's versions readers may
    /// read, but those whose entry the version after shares (RULES.md
    /// D750).
    pub(in crate::engine::rows) fn index_versions(self: Arc<Self>, (horizon, as_of): (Timestamp, Timestamp), columns: &[usize]) -> Rows {
        let unreadable = self.pending(Box::new(move |at, own| if own { at <= as_of } else { at <= horizon }), Some(columns));
        let reads = columns.to_vec();
        let unreadable = unreadable
            .filter(move |r| r.as_ref().map_or(true, |(_, _, kept)| !shares_entry(&reads, kept)))
            .map(|r| r.map(|(id, row, _)| (id, row)));
        let shared = match self.shared_older(columns) {
            Ok(shared) => shared,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let frozen = match self.frozen.directory() {
            Ok(directory) => Merged::new(Arc::clone(&self), (Merging::FrozenOnly, directory), columns.to_vec(), None),
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let columns = columns.to_vec();
        // Held to the end, so no entry of a row frozen meanwhile is taken
        // out before it is read.
        let held = self.frozen.directory();
        Box::new(unreadable.chain(frozen).chain(RowTable::entries(&self).flat_map(move |entry| {
            let _held = &held;
            let found = entry.and_then(|(key, newest)| self.versions_of(row_id(&key)?, newest, (horizon, as_of), (&columns, &shared)));
            match found {
                Ok(found) => found.into_iter().map(Ok).collect::<Vec<_>>(),
                Err(e) => vec![Err(e)],
            }
        })))
    }

    /// Where the committed versions awaiting collection were appended to
    /// the undo store, of those whose entry in an index whose key reads
    /// `reads` the version after shares (`shares_entry`): an index build
    /// gives them none, as collection takes none back.
    pub(in crate::engine::rows) fn shared_older(&self, reads: &[usize]) -> Result<Xxh3Set<u64>> {
        let mut shared = Xxh3Set::default();
        let timestamps: Vec<Timestamp> = lock_builtin(&self.versions.garbage).keys().copied().collect();
        for at in timestamps {
            let mut segment = 0_usize;
            while let Some(items) = self.garbage_segment(at, segment)? {
                segment = segment.saturating_add(1);
                shared.extend(items.into_iter().filter_map(|item| match item {
                    Dead::Older(_, appended, kept) => shares_entry(reads, &kept).then_some(appended.0),
                    Dead::Own(..) | Dead::Deletion(_) | Dead::Undone(_) | Dead::Objects(_) => None,
                }));
            }
        }
        Ok(shared)
    }

    /// The large objects awaiting collection (`Dead::Objects`), each list's
    /// segments copied one at a time.
    pub(in crate::engine::rows) fn pending_objects(&self) -> Result<Vec<u64>> {
        let mut ids = Vec::new();
        if self.lobs.columns().is_empty() {
            return Ok(ids);
        }
        let timestamps: Vec<Timestamp> = lock_builtin(&self.versions.garbage).keys().copied().collect();
        for at in timestamps {
            let mut segment = 0_usize;
            while let Some(items) = self.garbage_segment(at, segment)? {
                segment = segment.saturating_add(1);
                ids.extend(
                    items.into_iter().filter_map(|item| if let Dead::Objects(dropped) = item { Some(dropped) } else { None }).flatten(),
                );
            }
        }
        Ok(ids)
    }

    /// Segment `segment` of the garbage list of `at`, copied; none past
    /// its last, or if it is gone.
    fn garbage_segment(&self, at: Timestamp, segment: usize) -> Result<Option<Vec<Dead>>> {
        let mut garbage = lock_builtin(&self.versions.garbage);
        let Some(list) = garbage.get_mut(&at).filter(|list| segment < list.segments()) else { return Ok(None) };
        list.segment(segment, &decode_dead).map(Some)
    }
}

impl Pending {
    /// Copies the next segment with a version it takes; whether one was.
    fn fill(&mut self) -> Result<bool> {
        while let Some(&at) = self.timestamps.get(self.at) {
            let items = {
                let mut garbage = lock_builtin(&self.table.versions.garbage);
                let Some(list) = garbage.get_mut(&at).filter(|list| self.segment < list.segments()) else {
                    (self.at, self.segment) = (self.at.saturating_add(1), 0);
                    continue;
                };
                list.segment(self.segment, &decode_dead)?
            };
            self.segment = self.segment.saturating_add(1);
            let keep = &self.keep;
            self.buffer.extend(items.into_iter().filter(|item| own(item).is_some_and(|own| keep(at, own))));
            if !self.buffer.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Iterator for Pending {
    type Item = Result<Superseded>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.buffer.pop_front() {
                match self.table.garbage_row(item, self.columns.as_deref()) {
                    Ok(Some(row)) => return Some(Ok(row)),
                    Ok(None) => continue,
                    Err(e) => return Some(Err(e)),
                }
            }
            match self.fill() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    self.at = usize::MAX;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Whether the version garbage `item` supersedes was its transaction's
/// own; none if it is no version.
fn own(item: &Dead) -> Option<bool> {
    match item {
        Dead::Own(..) => Some(true),
        Dead::Older(..) => Some(false),
        Dead::Deletion(_) | Dead::Undone(_) | Dead::Objects(_) => None,
    }
}
