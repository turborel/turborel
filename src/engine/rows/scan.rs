//! A row table's scans: the rows a transaction sees, read through the table
//! they come from, so a scan owns what it reads and outlives the call that
//! started it (`TableStorage::scan`). Split from rows.rs (RULES.md
//! R-SIZE-2).

use std::iter;
use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::frozen::{Batched, Merged, Merging, Ordered};
use super::gathered::Gathering;
use super::mvcc::Read;
use super::{RowTable, key, row_id};
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::storage::{BATCH_ROWS, Batches, Conjunction, RowId, Rows, batches_of};
use crate::txn::TxId;

/// A table's tree, reached through the table, so a scan that owns the
/// table owns the tree it reads.
pub(super) struct TreeOf(Arc<RowTable>);

impl Deref for TreeOf {
    type Target = BLinkTree;

    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// A table's tree's entries in ranges of row ids (`RowTable::ranged`).
pub(super) struct Ranged {
    table: Arc<RowTable>,
    ranges: std::vec::IntoIter<(RowId, Option<RowId>)>,
    current: Option<Scan<TreeOf>>,
}

impl Iterator for Ranged {
    type Item = <Scan<TreeOf> as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.current.as_mut()?.next() {
                return Some(entry);
            }
            self.current = self.ranges.next().map(|range| RowTable::range(&self.table, range));
        }
    }
}

impl RowTable {
    /// Every entry of the table's tree, in row id order, read through
    /// `table`, which the scan holds.
    pub(super) fn entries(table: &Arc<Self>) -> Scan<TreeOf> {
        scan_of(TreeOf(Arc::clone(table)), Bound::Unbounded, Bound::Unbounded, Direction::Forward)
    }

    /// The tree's entries in `ranges` of row ids, in order, each from its
    /// first id up to, not including, its end if it has one; read through
    /// `table`, which the scan holds.
    fn ranged(table: &Arc<Self>, ranges: Vec<(RowId, Option<RowId>)>) -> Ranged {
        let mut ranges = ranges.into_iter();
        let current = ranges.next().map(|range| Self::range(table, range));
        Ranged { table: Arc::clone(table), ranges, current }
    }

    /// The tree's entries from row `first` up to, not including, `end`.
    fn range(table: &Arc<Self>, (first, end): (RowId, Option<RowId>)) -> Scan<TreeOf> {
        let (low, high) = (key(first), end.map(key));
        let upper = high.as_ref().map_or(Bound::Unbounded, |h| Bound::Excluded(&h[..]));
        scan_of(TreeOf(Arc::clone(table)), Bound::Included(&low[..]), upper, Direction::Forward)
    }

    /// The rows `tx` sees, `columns` decoded, and only those `condition`
    /// matches, if given, which is then what counts as read.
    pub(super) fn scan_rows(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<Conjunction>) -> Rows {
        let read = condition.clone().map_or(Read::All, Read::Condition);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        // The condition's columns are decoded too, to check it.
        let mut columns = columns.to_vec();
        columns.extend(condition.iter().flat_map(Conjunction::columns));
        // Its groups as they are before its tree is read (`rows/frozen.rs`).
        let directory = match self.frozen.directory() {
            Ok(directory) => directory,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        // A table that freezes rows, or still holds groups, reads through
        // the directory it holds, so the entries of rows frozen as it reads
        // are kept for it.
        if self.frozen.columns().is_some() || !directory.groups.is_empty() {
            return Box::new(Merged::new(Arc::clone(&self), (Merging::Seen(reader), directory), columns, condition));
        }
        // Only the zones whose rows the conditions may match (`rows/zones.rs`);
        // their rows not judged here, the filter judging every row given:
        // judging twice cost more than it saved.
        let ranges = self.zones.ranges(condition.as_ref());
        Box::new(Self::ranged(&self, ranges).filter_map(move |entry| {
            let found = entry.and_then(|(key, newest)| {
                let id = row_id(&key)?;
                let Some(record) = self.visible_from(newest, reader)? else { return Ok(None) };
                Ok(Some((id, self.decode(id, &record, Some(&columns))?)))
            });
            found.transpose()
        }))
    }

    /// The rows `tx` sees, `columns` decoded, and only those `condition`
    /// matches, if given, in the order of `keys` (`frozen/ordered.rs`).
    pub(super) fn scan_ordered_of(
        self: Arc<Self>,
        tx: TxId,
        (columns, keys): (&[usize], &[usize]),
        condition: Option<Conjunction>,
    ) -> Rows {
        let read = condition.clone().map_or(Read::All, Read::Condition);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        // The keys and the condition's columns are decoded too.
        let mut decoded = columns.to_vec();
        decoded.extend(keys);
        decoded.extend(condition.iter().flat_map(Conjunction::columns));
        match self.frozen.directory() {
            Ok(directory) => Box::new(Ordered::new(Arc::clone(&self), (reader, directory), (decoded, keys.to_vec()), condition)),
            Err(e) => Box::new(iter::once(Err(e))),
        }
    }

    /// The rows `tx` sees by batch, of `columns`, only those `condition`
    /// matches if given, which is then what counts as read: a table that
    /// freezes rows gives its frozen rows straight from their chunks
    /// (`rows/frozen/batches.rs`), any other its tree's records, each
    /// column decoded straight into its vector (`gathered.rs`); one that
    /// no longer freezes but still holds groups its scan's rows, gathered.
    pub(super) fn scan_batches_of(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<Conjunction>) -> Batches {
        let freezes = self.frozen.columns().is_some();
        if !freezes && self.frozen.directory().is_ok_and(|directory| directory.groups.is_empty()) {
            return self.gathered(tx, columns, condition.as_ref());
        }
        if !freezes {
            let rows = Arc::clone(&self).scan_rows(tx, columns, condition);
            return batches_of(rows, columns);
        }
        let read = condition.clone().map_or(Read::All, Read::Condition);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        match self.frozen.directory() {
            Ok(directory) => Box::new(Batched::new(Arc::clone(&self), (reader, directory), columns.to_vec(), condition)),
            Err(e) => Box::new(iter::once(Err(e))),
        }
    }

    /// The batches of the rows `tx` sees, of `columns`, of only the zones
    /// `condition` may match, if given, as `scan_rows` reads a table that
    /// holds no group, leaving every row to the filter: each record's
    /// columns decoded straight into their vectors.
    fn gathered(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<&Conjunction>) -> Batches {
        let read = condition.cloned().map_or(Read::All, Read::Condition);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        let mut entries = Self::ranged(&self, self.zones.ranges(condition));
        let mut gathering = Gathering::new(self.format.types(), columns);
        Box::new(iter::from_fn(move || {
            while gathering.rows() < BATCH_ROWS {
                let Some(entry) = entries.next() else { break };
                let gathered = entry.and_then(|(key, newest)| match self.visible_from(newest, reader)? {
                    Some(record) => self.gather_record((row_id(&key)?, &record), &mut gathering),
                    None => Ok(()),
                });
                if let Err(e) = gathered {
                    // The rows gathered are given up with the scan.
                    gathering.take();
                    return Some(Err(e));
                }
            }
            (gathering.rows() > 0).then(|| Ok(gathering.take()))
        }))
    }
}
