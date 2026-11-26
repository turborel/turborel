//! \[DESIGN\] A table that holds groups counted from its directory
//! (RULES.md D1180, `TableStorage::count`), the rows a scan by batch of no
//! column would give (`batches.rs`), with none of the group's values read.
//! The directory is taken before the tree is read, as the scan's is. Each
//! group the reader sees adds the rows its summary holds. Where its shadow
//! bits say the tree may have entries, the tree is read over those ids
//! alone: each entry the reader sees adds a row, and each of the group's
//! rows such an entry stands for, found by the group's row ids, takes one
//! away. A group with no bit set costs an addition. Then the tree's
//! entries outside every group's range are read, each the reader sees
//! adding a row. The zones' counts of rows settled are not used: a freeze
//! takes out entries of rows it froze without a change of their zones.

use std::ops::Bound;
use std::sync::Arc;

use super::{Directory, Entry};
use crate::engine::rows::mvcc::{EVERY_COLUMN, Read, Reader};
use crate::engine::rows::scan::Ranged;
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{BATCH_ROWS, Counts, RowId};
use crate::txn::TxId;

/// \[DESIGN\] The counts of a table that holds groups, a group's at a time,
/// then the tree's rows outside them, a batch's worth of entries at a time.
struct Grouped {
    table: Arc<RowTable>,
    reader: Reader,
    directory: Arc<Directory>,
    next_group: usize,
    /// The tree's entries outside every group's range, once the groups
    /// are counted.
    delta: Option<Ranged>,
}

impl RowTable {
    /// The counts of the rows `tx` sees of a table that holds or freezes
    /// groups, its read noted first, of the whole table, as a scan's with
    /// no condition is (`rows/mvcc.rs`).
    pub(in crate::engine::rows) fn grouped_counts(self: Arc<Self>, tx: TxId) -> Counts {
        let opened = self.reader(tx).and_then(|reader| self.note_read(tx, || Read::All(EVERY_COLUMN)).map(|()| reader));
        match opened.and_then(|reader| self.directory_for(reader).map(|directory| (reader, directory))) {
            Ok((reader, directory)) => Box::new(Grouped { table: self, reader, directory, next_group: 0, delta: None }),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
}

impl Grouped {
    /// The rows the reader sees of `entry`'s group: its own, less those the
    /// tree's entries stand for, and those entries it sees.
    fn group_count(&self, entry: &Entry) -> Result<u64> {
        let rows = u64::from(entry.summary.rows);
        let (shadowed, seen) = self.shadowing(entry)?;
        if shadowed.is_empty() {
            return Ok(rows);
        }
        let group = self.table.group_of(entry)?;
        let ids = group.row_ids(&self.table.pages_for(&group)?)?;
        let replaced = ids.iter().filter(|id| shadowed.binary_search(id).is_ok()).count();
        Ok(rows.saturating_sub(as_count(replaced)).saturating_add(seen))
    }

    /// The ids of the tree's entries over `entry`'s shadow bits, in order,
    /// and how many of them the reader sees.
    fn shadowing(&self, entry: &Entry) -> Result<(Vec<RowId>, u64)> {
        let (mut shadowed, mut seen) = (Vec::new(), 0_u64);
        for (first, last) in self.table.shadow_ranges(entry) {
            let (low, high) = (key(first), key(last));
            for found in self.table.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
                let (k, newest) = found?;
                shadowed.push(row_id(&k)?);
                if self.table.visible_from(newest, self.reader)?.is_some() {
                    seen = seen.saturating_add(1);
                }
            }
        }
        Ok((shadowed, seen))
    }

    /// The rows the reader sees of up to a batch's worth of the tree's
    /// entries outside the groups; none once they are all read.
    fn delta_count(&mut self) -> Option<Result<u64>> {
        let ranges = outside(&self.directory);
        let table = Arc::clone(&self.table);
        let delta = self.delta.get_or_insert_with(|| RowTable::ranged(&table, ranges));
        let (mut entries, mut seen) = (0_usize, 0_u64);
        while entries < BATCH_ROWS {
            let (_, newest) = match delta.next() {
                Some(Ok(found)) => found,
                Some(Err(e)) => return Some(Err(e)),
                None if entries == 0 => return None,
                None => break,
            };
            entries = entries.saturating_add(1);
            match table.visible_from(newest, self.reader) {
                Ok(Some(_)) => seen = seen.saturating_add(1),
                Ok(None) => {}
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Ok(seen))
    }
}

impl Iterator for Grouped {
    type Item = Result<u64>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(entry) = self.directory.groups.get(self.next_group).cloned() {
            self.next_group = self.next_group.saturating_add(1);
            return Some(self.group_count(&entry));
        }
        self.delta_count()
    }
}

/// The ranges of row ids outside every group of `directory`, each to its
/// end if it has one, in order.
pub(super) fn outside(directory: &Directory) -> Vec<(RowId, Option<RowId>)> {
    let mut ranges = Vec::new();
    let mut from: RowId = 0;
    for entry in &directory.groups {
        if entry.summary.first > from {
            ranges.push((from, Some(entry.summary.first)));
        }
        from = entry.summary.last.saturating_add(1);
    }
    ranges.push((from, None));
    ranges
}

/// `n` rows as a count.
fn as_count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
