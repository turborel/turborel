//! What a transaction read of a table, for its commit's check (RULES.md D29,
//! D38, D134): rows, conditions, or the whole table, which past
//! TRANSACTIONS.TRACKED_ROWS they become, and spatial regions, which past
//! it merge into a hull of each column; and what a transaction changed,
//! which another's reads are checked against.
//!
//! \[DESIGN\] RULES.md D448: each read carries the columns it consumed, as
//! a mask, and a write conflicts with a read only if the columns it
//! changed meet them: a read that consumed no changed column read the
//! same values from both versions, so there is no dependency to order
//! (Adya 1999, §3). An insertion or a deletion changes every column, and
//! a read that cannot say which columns it consumed read every column.

use std::cmp::Ordering;

use crate::hash::murmur::MurmurMap;
use crate::spill::row_bytes;
use crate::storage::{Conjunction, Region, Row, RowId, merged, same};
use crate::value::{Value, compare};

/// \[DESIGN\] Every column, and the row's being there: the columns of a
/// read that cannot say which it consumed, and those an insertion or a
/// deletion changes (RULES.md D448).
pub(in crate::engine::rows) const EVERY_COLUMN: u64 = u64::MAX;

/// \[DESIGN\] The mask of `columns`: the bit of each, bit 63 standing for
/// every column past 62; every column if none, as a read of no column
/// still reads that the row is there.
pub(in crate::engine::rows) fn columns_mask(columns: impl IntoIterator<Item = usize>) -> u64 {
    match columns.into_iter().fold(0, |mask, c| mask | bit(c)) {
        0 => EVERY_COLUMN,
        mask => mask,
    }
}

/// The bit of column `c` in a mask of `columns_mask`.
fn bit(c: usize) -> u64 {
    1_u64.checked_shl(u32::try_from(c.min(63)).unwrap_or(63)).unwrap_or(EVERY_COLUMN)
}

/// The columns `before` and `after`, two versions of a row, hold
/// different values in, as `columns_mask` has them; every column if
/// either is none, an insertion or a deletion.
pub(super) fn changed_columns(before: Option<&Row>, after: Option<&Row>) -> u64 {
    let (Some(before), Some(after)) = (before, after) else { return EVERY_COLUMN };
    (0..before.len().max(after.len()))
        .filter(|&c| !before.get(c).zip(after.get(c)).is_some_and(|(b, a)| same(b, a)))
        .fold(0, |mask, c| mask | bit(c))
}

/// What a transaction read, with the columns it consumed (`columns_mask`).
pub(in crate::engine::rows) enum Read {
    /// A row, by its id.
    Row(RowId, u64),
    /// The rows a scan or an index chose by conditions.
    Condition(Conjunction, u64),
    /// The rows a spatial index chose by their envelopes.
    Region(Region),
    /// Every row: a scan.
    All(u64),
}

impl Read {
    /// A scan's read of `columns` of the rows `condition` chose, if given,
    /// else of every row: it consumed those columns and the condition's.
    pub(in crate::engine::rows) fn scan(condition: Option<&Conjunction>, columns: &[usize]) -> Read {
        let consumed = columns.iter().copied().chain(condition.iter().flat_map(|c| c.columns()));
        let mask = columns_mask(consumed);
        condition.map_or(Read::All(mask), |c| Read::Condition(c.clone(), mask))
    }
}

/// What a transaction has read of a table: past the rows tracked, the
/// whole table.
#[derive(Default)]
pub(super) struct Reads {
    pub(super) rows: MurmurMap<RowId, u64>,
    pub(super) conditions: Vec<(Conjunction, u64)>,
    pub(super) regions: Vec<Region>,
    /// The envelopes of `regions`, counted as they are added.
    envelopes: usize,
    /// The columns a read of every row consumed, if one was made.
    pub(super) all: Option<u64>,
}

impl Reads {
    /// Adds `read`, then counts every row read if more than `tracked` rows
    /// or conditions are, and merges its regions into a hull of each column
    /// if more than `tracked` envelopes are: a read of more than it read,
    /// which may refuse a commit it need not, never one it must.
    pub(super) fn add(&mut self, read: Read, tracked: usize) {
        match read {
            Read::Row(id, columns) => *self.rows.entry(id).or_default() |= columns,
            Read::Condition(condition, columns) => self.conditions.push((condition, columns)),
            Read::Region(region) => self.add_region(region, tracked),
            Read::All(columns) => self.all = Some(self.all.unwrap_or_default() | columns),
        }
        if self.rows.len() > tracked || self.conditions.len() > tracked {
            *self = Reads { all: Some(self.columns()), ..Reads::default() };
        }
    }

    /// The columns any of its reads consumed.
    fn columns(&self) -> u64 {
        let regions = if self.regions.is_empty() { 0 } else { EVERY_COLUMN };
        let rows = self.rows.values().chain(self.conditions.iter().map(|(_, columns)| columns));
        rows.fold(self.all.unwrap_or_default() | regions, |mask, columns| mask | columns)
    }

    /// Adds `region`, unless it has no envelope and so read no row, then
    /// merges the regions past `tracked` envelopes.
    fn add_region(&mut self, region: Region, tracked: usize) {
        if region.envelopes().is_empty() {
            return;
        }
        self.envelopes = self.envelopes.saturating_add(region.envelopes().len());
        self.regions.push(region);
        if self.envelopes > tracked {
            self.regions = merged(std::mem::take(&mut self.regions));
            self.envelopes = self.regions.iter().map(|r| r.envelopes().len()).sum();
        }
    }

    /// Whether it read anything.
    pub(super) fn any(&self) -> bool {
        self.all.is_some() || !self.rows.is_empty() || !self.conditions.is_empty() || !self.regions.is_empty()
    }
}

/// \[DESIGN\] What a validated transaction changed, decoded the first time
/// a reader's conditions, regions or scans are checked against it
/// (RULES.md D444, D448): the rows of its versions and those before them,
/// and the columns any of its changes changed.
#[derive(Default)]
pub(super) struct Images {
    pub(super) rows: Changed,
    pub(super) columns: u64,
}

/// The rows of a validated transaction's changes, the versions it made and
/// those before them, which a condition another read is checked against:
/// the rows while their bytes are within the bytes held, then each
/// column's least and greatest value, which a condition may match more
/// often than a row does.
pub(super) enum Changed {
    Rows(Vec<Row>, usize),
    Bounds(Vec<Option<(Value, Value)>>),
}

impl Default for Changed {
    fn default() -> Changed {
        Changed::Rows(Vec::new(), 0)
    }
}

impl Changed {
    /// Adds `row`, the rows held then made bounds if they pass `held`
    /// bytes.
    pub(super) fn add(&mut self, row: Row, held: usize) {
        match self {
            Changed::Rows(rows, bytes) => {
                *bytes = bytes.saturating_add(row_bytes(&row));
                rows.push(row);
                if *bytes > held {
                    let mut bounds = Vec::new();
                    for kept in rows.drain(..) {
                        widen(&mut bounds, &kept);
                    }
                    *self = Changed::Bounds(bounds);
                }
            }
            Changed::Bounds(bounds) => widen(bounds, &row),
        }
    }

    /// Whether one of `conditions` or `regions` matches a row of it, or
    /// may. Bounds say nothing of a geometry's envelope, so any region may.
    /// A condition whose columns meet none `changed` is left out: no change
    /// moved a row into or out of it, nor changed a value it consumed.
    pub(super) fn matched_by(&self, conditions: &[(Conjunction, u64)], changed: u64, regions: &[Region]) -> bool {
        let mut met = conditions.iter().filter(|(_, columns)| columns & changed != 0).map(|(c, _)| c);
        match self {
            Changed::Rows(rows, _) => rows.iter().any(|row| met.clone().any(|c| c.matches(row)) || regions.iter().any(|r| r.matches(row))),
            Changed::Bounds(bounds) => !regions.is_empty() || met.any(|c| c.may_match(bounds)),
        }
    }
}

/// `bounds` widened to hold each value of `row` not null.
fn widen(bounds: &mut Vec<Option<(Value, Value)>>, row: &Row) {
    if bounds.len() < row.len() {
        bounds.resize(row.len(), None);
    }
    for (bound, v) in bounds.iter_mut().zip(row) {
        if v.is_null() {
            continue;
        }
        match bound {
            Some((least, greatest)) => {
                if compare(v, least) == Some(Ordering::Less) {
                    *least = v.clone();
                }
                if compare(v, greatest) == Some(Ordering::Greater) {
                    *greatest = v.clone();
                }
            }
            None => *bound = Some((v.clone(), v.clone())),
        }
    }
}
