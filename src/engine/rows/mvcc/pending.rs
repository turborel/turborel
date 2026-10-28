//! \[DESIGN\] The rows of the versions awaiting collection, read as they
//! are pulled (RULES.md D29, D38): for an index build, the versions it
//! needs entries for, and for a checkpoint, those whose entries recovery
//! takes back. The garbage lists are copied a segment at a time, the lock
//! on them taken for each, so commits adding garbage wait for one segment
//! at most; and each version is read as its row is pulled. The reader's
//! caller holds collection of the table back meanwhile (`TableLock`), so
//! no list it reads is removed. Split from engine/rows/mvcc.rs (RULES.md
//! R-SIZE-2).

use std::collections::VecDeque;
use std::sync::Arc;

use super::Dead;
use super::lists::decode_dead;
use crate::engine::rows::RowTable;
use crate::error::Result;
use crate::storage::{Row, RowId, Rows};
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
    /// they are pulled, `columns` decoded, or all.
    pub(in crate::engine::rows) fn pending(self: &Arc<Self>, keep: Keep, columns: Option<&[usize]>) -> Rows {
        let timestamps = lock_builtin(&self.versions.garbage).keys().copied().collect();
        let columns = columns.map(<[usize]>::to_vec);
        Box::new(Pending { table: Arc::clone(self), keep, columns, timestamps, at: 0, segment: 0, buffer: VecDeque::new() })
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
    type Item = Result<(RowId, Row)>;

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
        Dead::Deletion(_) | Dead::Undone(_) => None,
    }
}
