//! A row table's scans: the rows a transaction sees, read through the table
//! they come from, so a scan owns what it reads and outlives the call that
//! started it (`TableStorage::scan`).

use std::iter;
use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::frozen::{Batched, Merged, Merging, Ordered};
use super::mvcc::{EVERY_COLUMN, Read, Reader, columns_mask};
use super::sealing::sealed;
use super::version::Version;
use super::{RowTable, key, row_id};
use crate::engine::tree::{BLinkTree, Direction, Scan, Visit, Visited, scan_of};
use crate::error::Result;
use crate::storage::{Batches, Conjunction, Near, Ramp, Row, RowId, Rows, ScanOrder, batches_of};
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

impl Ranged {
    /// Shows `f` the newest versions of its rows in order, as
    /// `Scan::visit` does, range after range.
    ///
    /// # Errors
    ///
    /// As `Scan::visit`.
    pub(super) fn visit(&mut self, mut f: impl FnMut(&[u8]) -> Result<Visit>) -> Result<Visited> {
        loop {
            let Some(current) = self.current.as_mut() else { return Ok(Visited::Done) };
            match current.visit(&mut f)? {
                Visited::Done => self.current = self.ranges.next().map(|range| RowTable::range(&self.table, range)),
                visited @ (Visited::More | Visited::Later(_)) => return Ok(visited),
            }
        }
    }
}

/// \[DESIGN\] The record of a row's newest version, `newest`, if a read by
/// row id decodes it where it lies in its leaf (RULES.md D869): `reader`
/// sees the version as it is stamped, it is no deletion, and its record is
/// not sealed. None of any other, which is copied out and read after, as a
/// scan by batch reads it (`rows/in_place.rs`): one whose maker is looked
/// up, one the reader goes behind, to the undo store, and one its
/// encodings open.
fn standing(newest: &[u8], reader: Reader) -> Result<Option<&[u8]>> {
    let version = Version::parse(newest)?;
    let read = reader.sees(version.maker) && !version.deleted && !sealed(version.record);
    Ok(read.then_some(version.record))
}

/// What a read by row id made of an entry where it lies.
enum Shown<T> {
    /// Its answer, decoded there.
    Read(T),
    /// The tree has no entry of the row.
    Absent,
    /// The entry is to be copied out and read after: one that did not
    /// stand for the reader (`standing`), or one with a value asked for
    /// kept out of line.
    Later,
}

/// What a read where the entry lies gave, `shown`, none of a tree with no
/// entry: an error raised for a value `outside` the record is a read to
/// make after, from a copy.
fn later<T>(shown: Result<Option<Shown<T>>>, outside: bool) -> Result<Shown<T>> {
    match shown {
        Ok(None) => Ok(Shown::Absent),
        Ok(Some(shown)) => Ok(shown),
        Err(e) => {
            if outside {
                Ok(Shown::Later)
            } else {
                Err(e)
            }
        }
    }
}

impl RowTable {
    /// Row `id` as `tx` sees it, `columns` decoded, or all.
    pub(super) fn get_of(&self, tx: TxId, id: RowId, columns: Option<&[usize]>, near: &mut Near) -> Result<Option<Row>> {
        // Held until its values out of line are fetched (RULES.md D1140).
        let _pinned = self.frozen.pinned();
        let reader = self.read_row(tx, (id, columns.map_or(EVERY_COLUMN, |c| columns_mask(c.iter().copied()))), near)?;
        near.gone = None;
        // Decoded where it lies in its leaf, if it can be (RULES.md D869).
        let mut outside = false;
        let shown = self.tree.get_near_with(&key(id), near, |newest| {
            let Some(record) = standing(newest, reader)? else { return Ok(Shown::Later) };
            self.decode_standing(record, columns, &mut outside).map(Shown::Read)
        });
        match later(shown, outside)? {
            Shown::Read(row) => return Ok(Some(row)),
            Shown::Absent => return self.frozen_seen(id, columns, reader),
            Shown::Later => {}
        }
        if !self.tree.get_near(&key(id), near)? {
            // Frozen, so committed before every snapshot (`rows/frozen.rs`),
            // unless a load made it (`frozen/stamped.rs`).
            return self.frozen_seen(id, columns, reader);
        }
        let seen = self.visible_in(&near.bytes, reader)?.map(|record| self.decode(id, &record, columns)).transpose()?;
        if seen.is_none() {
            near.gone = self.deletion_of(&near.bytes);
        }
        Ok(seen)
    }

    /// \[DESIGN\] Appends to `out` the values of `columns` of row `id` as
    /// `tx` sees it, in their order, and no other: whether it sees the
    /// row, `out` as it was if not. A read of a few columns makes no row
    /// of the table's width, and `near` and `out`, kept by the caller from
    /// read to read, are written into again, not made for each (RULES.md
    /// D862). The read is noted as `TableStorage::get_columns` notes it.
    ///
    /// # Errors
    ///
    /// As `TableStorage::get`.
    pub fn get_listed(&self, tx: TxId, id: RowId, columns: &[usize], (near, out): (&mut Near, &mut Row)) -> Result<bool> {
        // Held until its values out of line are fetched (RULES.md D1140).
        let _pinned = self.frozen.pinned();
        let reader = self.read_row(tx, (id, columns_mask(columns.iter().copied())), near)?;
        near.gone = None;
        // Decoded where it lies in its leaf, if it can be (RULES.md D869).
        let mut outside = false;
        let shown = self.tree.get_near_with(&key(id), near, |newest| {
            let Some(record) = standing(newest, reader)? else { return Ok(Shown::Later) };
            self.listed_standing(record, columns, (&mut *out, &mut outside)).map(Shown::Read)
        });
        let frozen = match later(shown, outside)? {
            Shown::Read(()) => return Ok(true),
            Shown::Absent => true,
            Shown::Later => false,
        };
        if frozen || !self.tree.get_near(&key(id), near)? {
            // Its values alone, no row of the table's width made (RULES.md D1091).
            return self.frozen_listed((id, columns), reader, out);
        }
        let Some(record) = self.visible_in(&near.bytes, reader)? else {
            let gone = self.deletion_of(&near.bytes);
            near.gone = gone;
            return Ok(false);
        };
        self.decode_listed((id, &record), columns, out)?;
        Ok(true)
    }

    /// Every entry of the table's tree, in row id order, read through
    /// `table`, which the scan holds.
    pub(super) fn entries(table: &Arc<Self>) -> Scan<TreeOf> {
        scan_of(TreeOf(Arc::clone(table)), Bound::Unbounded, Bound::Unbounded, Direction::Forward)
    }

    /// The tree's entries in `ranges` of row ids, in order, each from its
    /// first id up to, not including, its end if it has one; read through
    /// `table`, which the scan holds.
    pub(super) fn ranged(table: &Arc<Self>, ranges: Vec<(RowId, Option<RowId>)>) -> Ranged {
        let mut ranges = ranges.into_iter();
        let current = ranges.next().map(|range| Self::range(table, range));
        Ranged { table: Arc::clone(table), ranges, current }
    }

    /// The tree's entries from row `first` up to, not including, `end`.
    pub(super) fn range(table: &Arc<Self>, (first, end): (RowId, Option<RowId>)) -> Scan<TreeOf> {
        let (low, high) = (key(first), end.map(key));
        let upper = high.as_ref().map_or(Bound::Unbounded, |h| Bound::Excluded(&h[..]));
        scan_of(TreeOf(Arc::clone(table)), Bound::Included(&low[..]), upper, Direction::Forward)
    }

    /// The rows `tx` sees, `columns` decoded, and only those `condition`
    /// matches, if given, which is then what counts as read; of a table
    /// that holds no group, every row of the zones it may match.
    pub(super) fn scan_rows(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<Conjunction>) -> Rows {
        let read = || Read::scan(condition.as_ref(), columns);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        // The condition's columns are decoded too, to check it.
        let mut columns = columns.to_vec();
        columns.extend(condition.iter().flat_map(Conjunction::columns));
        // Its groups as they are before its tree is read (`rows/frozen.rs`),
        // those it sees (`frozen/stamped.rs`).
        let directory = match self.directory_for(reader) {
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
    /// matches, if given, in the order of `keys`, or backward, the first
    /// `most` alone if only those are asked for
    /// (`frozen/ordered.rs`).
    pub(super) fn scan_ordered_of(
        self: Arc<Self>,
        tx: TxId,
        (columns, order): (&[usize], ScanOrder<'_>),
        condition: Option<Conjunction>,
    ) -> Rows {
        match self.ordered(tx, (columns, order), condition) {
            Ok(ordered) => Box::new(ordered),
            Err(e) => Box::new(iter::once(Err(e))),
        }
    }

    /// As `scan_ordered_of`, by batch, of `columns` alone.
    pub(super) fn scan_ordered_batches_of(
        self: Arc<Self>,
        tx: TxId,
        (columns, order): (&[usize], ScanOrder<'_>),
        condition: Option<Conjunction>,
    ) -> Batches {
        match self.ordered(tx, (columns, order), condition) {
            Ok(ordered) => ordered.batches(),
            Err(e) => Box::new(iter::once(Err(e))),
        }
    }

    /// The merge `scan_ordered_of` reads, the read noted.
    fn ordered(self: Arc<Self>, tx: TxId, (columns, order): (&[usize], ScanOrder<'_>), condition: Option<Conjunction>) -> Result<Ordered> {
        // The order's columns are consumed too: they chose which rows come
        // first (RULES.md D448).
        let read = || Read::scan(condition.as_ref(), &[columns, order.0].concat());
        let reader = self.reader(tx)?;
        self.note_read(tx, read)?;
        let directory = self.directory_for(reader)?;
        Ok(Ordered::new(Arc::clone(&self), (reader, directory), (columns, order), condition))
    }

    /// The rows `tx` sees by batch, of `columns`, only those `condition`
    /// matches if given, which is then what counts as read: a table that
    /// freezes rows gives its frozen rows straight from their chunks
    /// (`rows/frozen/batches.rs`), any other its tree's records, each
    /// column decoded straight into its vector (`gathered.rs`); one that
    /// no longer freezes but still holds groups its scan's rows, gathered.
    /// The frozen rows' columns of `asked.0` are given late (RULES.md D361),
    /// and those of `asked.1` measured (RULES.md D1267).
    pub(super) fn scan_batches_of(
        self: Arc<Self>,
        tx: TxId,
        columns: &[usize],
        condition: Option<Conjunction>,
        asked: (&[usize], &[usize]),
    ) -> Batches {
        let freezes = self.frozen.columns().is_some();
        if !freezes && self.frozen.directory().is_ok_and(|directory| directory.groups.is_empty()) {
            return self.gathered(tx, columns, condition.as_ref());
        }
        if !freezes {
            let rows = Arc::clone(&self).scan_rows(tx, columns, condition);
            return batches_of(rows, columns);
        }
        let read = || Read::scan(condition.as_ref(), columns);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        match self.directory_for(reader) {
            Ok(directory) => Box::new(Batched::new(Arc::clone(&self), (reader, directory), columns.to_vec(), condition).asked(asked)),
            Err(e) => Box::new(iter::once(Err(e))),
        }
    }

    /// The batches of the rows `tx` sees, of `columns`, of only the zones
    /// `condition` may match, if given, as `scan_rows` reads a table that
    /// holds no group, leaving every row to the filter. Each record is read
    /// in place where it can be, its columns decoded straight into their
    /// vectors (`rows/in_place.rs`).
    fn gathered(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<&Conjunction>) -> Batches {
        let read = || Read::scan(condition, columns);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        let ranges = self.zones.ranges(condition);
        self.gathered_in((reader, Ramp::default()), columns, ranges)
    }
}

// Unit tests: what a read that finds no row tells (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
