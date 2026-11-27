//! \[DESIGN\] A row table's scan split into morsels (RULES.md D290): ranges
//! of row ids, together all of them. A table of the row store is cut
//! every so many ids, as many as hold the rows asked of a morsel on
//! average; one that holds groups of frozen rows is cut at the starts of
//! its groups, so no group is read by two morsels, and its tree's rows
//! past the last group as a row table's are. The split holds its reader,
//! the directory of groups as it was when split, and the zones a
//! condition may match, so each morsel reads what the whole scan would
//! (`rows/scan.rs`), on any thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::frozen::{Batched, Directory, Merged, Merging};
use super::mvcc::{Read, Reader};
use super::{RowTable, row_id};
use crate::error::Result;
use crate::storage::{Batches, Conjunct, Conjunction, Ramp, RowId, Rows, Split};
use crate::txn::TxId;

/// A row table's scan split at row ids.
pub(super) struct RowSplit {
    table: Arc<RowTable>,
    reader: Reader,
    /// The columns decoded: those asked for, then the condition's.
    columns: Vec<usize>,
    /// How many of `columns` were asked for, the batches' vectors.
    asked: usize,
    condition: Option<Conjunction>,
    /// The groups as they were when it was split, if it reads any.
    directory: Option<Arc<Directory>>,
    /// The ranges of the zones the condition may match (`rows/zones.rs`),
    /// as they were when it was split.
    zones: Vec<(RowId, Option<RowId>)>,
    /// The first row id of each morsel after the first, which begins at 0.
    cuts: Vec<RowId>,
}

impl RowTable {
    /// The rows `tx` sees, `columns` decoded, of only those `condition`
    /// matches if given, as `scan_rows` reads them, split into morsels of
    /// about `rows` rows each.
    ///
    /// # Errors
    ///
    /// As the reader, the read's note or the directory fail.
    pub(super) fn split_of(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<Conjunction>, rows: usize) -> Result<RowSplit> {
        let reader = self.reader(tx)?;
        self.note_read(tx, || Read::scan(condition.as_ref(), columns))?;
        let asked = columns.len();
        let mut columns = columns.to_vec();
        columns.extend(condition.iter().flat_map(Conjunction::columns));
        let directory = self.directory_for(reader)?;
        let reads_groups = self.frozen.columns().is_some() || !directory.groups.is_empty();
        let cuts = self.cuts(&directory, rows.max(1));
        // Judged once for every morsel, of a table read through its groups
        // too, whose tree's rows each morsel reads by them (RULES.md D1052).
        let zones = self.zones.ranges(condition.as_ref());
        let directory = reads_groups.then_some(directory);
        Ok(RowSplit { table: self, reader, columns, asked, condition, directory, zones, cuts })
    }

    /// Where morsels of about `rows` rows each begin, after the first: at
    /// the first group past each `rows` rows of groups, then every so many
    /// ids of the tree's rows past the groups, as many as hold `rows` rows
    /// of them on average, up to the next id to be given.
    fn cuts(&self, directory: &Directory, rows: usize) -> Vec<RowId> {
        let mut cuts = Vec::new();
        let mut held = 0_usize;
        for entry in &directory.groups {
            if held >= rows {
                cuts.push(entry.summary.first);
                held = 0;
            }
            held = held.saturating_add(usize::try_from(entry.summary.rows).unwrap_or(usize::MAX));
        }
        let from = directory.groups.last().map_or(0, |e| e.summary.last.saturating_add(1));
        let next = self.next.load(Ordering::Acquire);
        let frozen = usize::try_from(directory.rows()).unwrap_or(usize::MAX);
        let tree = self.counted.live.load(Ordering::Acquire).saturating_sub(frozen);
        let span = next.saturating_sub(from);
        let Some(morsels) = u64::try_from(tree.div_ceil(rows)).ok().filter(|&m| m > 1 && span > 0) else { return cuts };
        let width = span.div_ceil(morsels).max(1);
        let past = if held > 0 { Some(from) } else { None };
        cuts.extend(past.into_iter().chain((1..morsels).map(|m| from.saturating_add(m.saturating_mul(width)))).filter(|&c| c > 0));
        cuts.dedup();
        cuts
    }
}

impl RowSplit {
    /// The row ids of morsel `n`: from its first up to, not including, its
    /// end, if it has one; none past the last.
    fn range(&self, n: usize) -> Option<(RowId, Option<RowId>)> {
        let first = if n == 0 { 0 } else { *self.cuts.get(n.checked_sub(1)?)? };
        Some((first, self.cuts.get(n).copied()))
    }

    /// The ranges of the zones the condition may match within `range`.
    fn zones_within(&self, (first, end): (RowId, Option<RowId>)) -> Vec<(RowId, Option<RowId>)> {
        let within = |(from, to): &(RowId, Option<RowId>)| {
            let from = (*from).max(first);
            let to = match (*to, end) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            to.is_none_or(|t| from < t).then_some((from, to))
        };
        self.zones.iter().filter_map(within).collect()
    }

    /// The rows of the tree in `range` that its reader sees, of the zones
    /// the condition may match.
    fn tree_rows(&self, range: (RowId, Option<RowId>)) -> Rows {
        let ranges = self.zones_within(range);
        let (table, reader, columns) = (Arc::clone(&self.table), self.reader, self.columns.clone());
        Box::new(RowTable::ranged(&self.table, ranges).filter_map(move |entry| {
            let found = entry.and_then(|(key, newest)| {
                let id = row_id(&key)?;
                let Some(record) = table.visible_from(newest, reader)? else { return Ok(None) };
                Ok(Some((id, table.decode(id, &record, Some(&columns))?)))
            });
            found.transpose()
        }))
    }
}

impl Split for RowSplit {
    fn morsels(&self) -> usize {
        self.cuts.len().saturating_add(1)
    }

    fn morsel(&self, n: usize) -> Rows {
        let Some(range) = self.range(n) else { return Box::new(std::iter::empty()) };
        let Some(directory) = &self.directory else { return self.tree_rows(range) };
        let inside = |e: &&Arc<super::frozen::Entry>| e.summary.first >= range.0 && range.1.is_none_or(|end| e.summary.last < end);
        let groups = directory.groups.iter().filter(inside).cloned().collect();
        let part = Arc::new(Directory::part(directory, groups));
        let what = (self.columns.clone(), self.condition.clone());
        Box::new(Merged::within(Arc::clone(&self.table), (Merging::Seen(self.reader), part), what, range))
    }

    /// Of a table of the row store, its tree's records in the morsel's
    /// range decoded straight into vectors, as its scan by batch reads them
    /// (`rows/scan.rs`); of the column store, its groups within the range
    /// read a vector at a time from their chunks, then its tree's rows of
    /// the range (`frozen/batches.rs`), its columns of `late` as their
    /// places (`frozen/late.rs`).
    fn morsel_batches(&self, n: usize, late: &[usize]) -> Option<Batches> {
        let range = self.range(n)?;
        let asked = self.columns.get(..self.asked)?.to_vec();
        let Some(directory) = &self.directory else {
            return Some(Arc::clone(&self.table).gathered_in((self.reader, Ramp::whole()), &asked, self.zones_within(range)));
        };
        let inside = |e: &&Arc<super::frozen::Entry>| e.summary.first >= range.0 && range.1.is_none_or(|end| e.summary.last < end);
        let part = Arc::new(Directory::part(directory, directory.groups.iter().filter(inside).cloned().collect()));
        let what = (asked, self.condition.clone());
        Some(Box::new(Batched::within(Arc::clone(&self.table), (self.reader, part), what, self.zones_within(range)).with_late(late)))
    }

    /// Its morsels as they are cut, their scans told `more` after the
    /// condition, its column decoded after those asked for as the
    /// condition's are. The zones are those the condition alone may
    /// match, which hold every row `more` is true of.
    fn told(&self, more: Conjunct) -> Option<Arc<dyn Split>> {
        let mut columns = self.columns.clone();
        columns.extend(more.columns());
        let mut condition = self.condition.clone().unwrap_or_else(|| Conjunction(Vec::new()));
        condition.0.push(more);
        Some(Arc::new(RowSplit {
            table: Arc::clone(&self.table),
            reader: self.reader,
            columns,
            asked: self.asked,
            condition: Some(condition),
            directory: self.directory.clone(),
            zones: self.zones.clone(),
            cuts: self.cuts.clone(),
        }))
    }
}
