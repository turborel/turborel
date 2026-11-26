//! \[DESIGN\] A row table's scan split into morsels (RULES.md D290): ranges
//! of row ids, together all of them. A table of the row store is cut
//! every so many ids, as many as hold the rows asked of a morsel on
//! average; one that holds groups of frozen rows is cut at the starts of
//! its groups, so no group is read by two morsels, and its tree's rows
//! past the last group as a row table's are. The split holds its reader,
//! the directory of groups as it was when split, and the zones a
//! condition may match, so each morsel reads what the whole scan would
//! (`rows/scan.rs`), on any thread.

use std::cmp::Ordering as Order;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::frozen::{Batched, Directory, Entry, Merged, Merging};
use super::mvcc::{Read, Reader};
use super::{RowTable, row_id};
use crate::error::Result;
use crate::storage::{Batches, Conjunction, Ramp, RowId, Rows, Split};
use crate::txn::TxId;
use crate::value::{Value, compare};

/// \[DESIGN\] The most rows of its groups for each row of its tree a table
/// has whose morsels are bounded (`Split::bounds`): its tree's rows are
/// read once more to bound them, so a table frozen but for few rows.
const TREE_SHARE: u64 = 8;

/// `bounds` widened to hold `least` and `greatest`; none if they do not
/// compare with its own.
fn widened(bounds: Option<(Value, Value)>, (least, greatest): (&Value, &Value)) -> Option<(Value, Value)> {
    let Some((lo, hi)) = bounds else { return Some((least.clone(), greatest.clone())) };
    let lo = if compare(least, &lo)? == Order::Less { least.clone() } else { lo };
    let hi = if compare(greatest, &hi)? == Order::Greater { greatest.clone() } else { hi };
    Some((lo, hi))
}

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

    /// The groups of the directory within `range`, in order, if it reads
    /// any.
    fn inside(&self, (first, end): (RowId, Option<RowId>)) -> Vec<&Arc<Entry>> {
        let within = |e: &&Arc<Entry>| e.summary.first >= first && end.is_none_or(|end| e.summary.last < end);
        self.directory.iter().flat_map(|d| d.groups.iter().filter(within)).collect()
    }

    /// The ranges of row ids in `range` over which the tree may hold
    /// entries its groups `inside`, in order, do not hold whole: between
    /// them or past them, and where their shadow bits say it may
    /// (`frozen/shadow_bits.rs`).
    fn unfrozen(&self, (first, end): (RowId, Option<RowId>), inside: &[&Arc<Entry>]) -> Vec<(RowId, Option<RowId>)> {
        let mut ranges: Vec<(RowId, Option<RowId>)> = Vec::new();
        let mut from = first;
        for entry in inside {
            if from < entry.summary.first {
                ranges.push((from, Some(entry.summary.first)));
            }
            ranges.extend(self.table.shadow_ranges(entry).into_iter().map(|(a, b)| (a, Some(b.saturating_add(1)))));
            from = entry.summary.last.saturating_add(1);
        }
        if end.is_none_or(|end| from < end) {
            ranges.push((from, end));
        }
        ranges
    }

    /// Widens `bounds` by column `column` of the rows of the tree in
    /// `ranges` that its reader sees, each version it would read decoded,
    /// as `tree_rows` reads them; none if an entry cannot be read, or two
    /// values do not compare.
    fn tree_bounds(&self, ranges: Vec<(RowId, Option<RowId>)>, column: usize, bounds: &mut Option<(Value, Value)>) -> Option<()> {
        for entry in RowTable::ranged(&self.table, ranges) {
            let (key, newest) = entry.ok()?;
            let id = row_id(&key).ok()?;
            let Some(record) = self.table.visible_from(newest, self.reader).ok()? else { continue };
            let row = self.table.decode(id, &record, Some(&[column])).ok()?;
            let value = row.get(column)?;
            if !value.is_null() {
                *bounds = Some(widened(bounds.take(), (value, value))?);
            }
        }
        Some(())
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
        let groups = self.inside(range).into_iter().cloned().collect();
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
        let part = Arc::new(Directory::part(directory, self.inside(range).into_iter().cloned().collect()));
        let what = (asked, self.condition.clone());
        Some(Box::new(Batched::within(Arc::clone(&self.table), (self.reader, part), what, self.zones_within(range)).with_late(late)))
    }

    /// \[DESIGN\] (RULES.md D1121) The bounds in `column` of a morsel's
    /// groups' zones, widened by the values of the rows the tree holds in
    /// its range, read and decoded, those past its groups and those
    /// written since they were frozen. The tree is read only of a table
    /// whose tree holds at most `1 / TREE_SHARE` as many rows as its
    /// groups, so bounding every morsel reads little; none else, nor of a
    /// table of no groups.
    fn bounds(&self, n: usize, column: usize) -> Option<Option<(Value, Value)>> {
        let range = self.range(n)?;
        let directory = self.directory.as_ref()?;
        let frozen = directory.rows();
        let tree = self.table.counted.live.load(Ordering::Acquire).saturating_sub(usize::try_from(frozen).unwrap_or(usize::MAX));
        if u64::try_from(tree).unwrap_or(u64::MAX).saturating_mul(TREE_SHARE) > frozen {
            return None;
        }
        let inside = self.inside(range);
        let ranges = self.unfrozen(range, &inside);
        let mut bounds: Option<(Value, Value)> = None;
        for entry in &inside {
            let Some((least, greatest)) = &entry.summary.columns.get(column)?.bounds else { continue };
            bounds = Some(widened(bounds, (least, greatest))?);
        }
        if !ranges.is_empty() {
            self.tree_bounds(ranges, column, &mut bounds)?;
        }
        Some(bounds)
    }
}
