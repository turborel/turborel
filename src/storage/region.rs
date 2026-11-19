//! \[DESIGN\] The envelopes a spatial read chose its rows from
//! (`TableStorage::read_by_region`, RULES.md D134), which a serializable
//! engine counts as read in place of the whole table.

use super::Row;
use crate::geometry::Geometry;
use crate::spatial::{Bounds, DIMENSIONS};
use crate::value::Value;

/// \[DESIGN\] Where a geometry index keys a geometry with no coordinate,
/// an empty one: the origin, so an empty value is a candidate of every
/// window holding it, never a row a window misses (RULES.md D85).
pub(crate) const EMPTY_AT: [f64; DIMENSIONS] = [0.0_f64; DIMENSIONS];

/// \[DESIGN\] The rows a read chose by their geometry's envelope: those
/// whose value of `column` has an envelope meeting one of `envelopes`.
/// A row whose value is null is none of them, as no index holds it and no
/// relationship holds of it. Matching never calls a plug-in, as storage
/// may hold its locks (R-ERR-4).
#[derive(Debug, Clone)]
pub struct Region {
    column: usize,
    envelopes: Vec<Bounds>,
}

impl Region {
    /// The rows whose value of `column`, in the table's order, has an
    /// envelope meeting one of `envelopes`. A plan that reads a widened
    /// window, as a distance search reads a window grown by its distance,
    /// gives that window.
    pub fn new(column: usize, envelopes: Vec<Bounds>) -> Region {
        Region { column, envelopes }
    }

    /// The column whose values are matched.
    pub fn column(&self) -> usize {
        self.column
    }

    /// The envelopes read.
    pub fn envelopes(&self) -> &[Bounds] {
        &self.envelopes
    }

    /// Whether `row` may be one the read chose: its value of the column
    /// not null, and its envelope, where an index keys it, meeting one of
    /// the region's. A value that is not a readable geometry may be any
    /// row, so it matches: never narrower than the read.
    pub fn matches(&self, row: &Row) -> bool {
        let Some(value) = row.get(self.column) else { return true };
        if value.is_null() {
            return false;
        }
        envelope_of(value).is_none_or(|at| self.envelopes.iter().any(|e| e.intersects(at)))
    }

    /// The smallest rectangle holding every envelope, none of no envelope.
    pub fn hull(&self) -> Option<Bounds> {
        self.envelopes.iter().copied().reduce(Bounds::union)
    }
}

/// The envelope a geometry index keys `geometry` by: its own, or the
/// origin's for an empty one (`EMPTY_AT`).
///
/// # Errors
///
/// None in practice: the origin is a finite point.
pub(crate) fn keyed_envelope(geometry: &Geometry) -> crate::error::Result<Bounds> {
    geometry.bounds().map_or_else(|| Bounds::point(EMPTY_AT), Ok)
}

/// The envelope `value` is keyed by, if it is a readable geometry.
fn envelope_of(value: &Value) -> Option<Bounds> {
    Geometry::from_value(value).ok().and_then(|g| keyed_envelope(&g).ok())
}

/// The regions `regions` merged, of each column one of the hull of its
/// envelopes: a read of more than was read, which may refuse a commit it
/// need not, never one it must. Their count is then at most the columns'.
pub(crate) fn merged(regions: Vec<Region>) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    for region in regions {
        let Some(hull) = region.hull() else { continue };
        match out.iter_mut().find(|r| r.column == region.column) {
            Some(kept) => kept.envelopes = kept.hull().map_or_else(|| vec![hull], |h| vec![h.union(hull)]),
            None => out.push(Region { column: region.column, envelopes: vec![hull] }),
        }
    }
    out
}

// Unit tests: rows a region matches, and merging (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
