//! Geometry candidates use envelope projections, including a harmless
//! origin projection for empty values. Mismatched query references scan
//! every entry so SQL, not planning or pruning, decides whether an error
//! is reached. No function callback occurs under a tree latch. Distance
//! comparisons search a window around the geometry (`geometry/within.rs`).

use std::sync::Arc;

use super::{MOST_COUNTED, RECTANGLE_COLUMNS, probe, unsupported};
use crate::engine::rtree::RTree;
use crate::error::{Code, Result, SqlError};
use crate::geometry::{Geometry, SpatialOperation};
use crate::index::{Entries, Key, KeyProjection, NearestQuery, NeighbourEntries, Predicate, Probe};
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::spatial::window::{self, Radius};
use crate::storage::keyed_envelope;
use crate::value::Value;

mod within;

const ARGUMENTS: usize = 2;

pub(super) fn supports_nearest(query: &NearestQuery) -> bool {
    query.column == 0
        && query.argument < ARGUMENTS
        && query.function.0.deterministic()
        && query.function.0.spatial_operation() == Some(SpatialOperation::Distance)
}

pub(super) fn nearest(tree: &RTree, query: &NearestQuery, srid: i32, budget: &Arc<MemoryBudget>) -> Result<Option<NeighbourEntries>> {
    if !supports_nearest(query) {
        return Ok(None);
    }
    // A malformed or mismatched operand must still be skipped when SQL
    // evaluates no rows. Declining leaves that decision to ordinary calls.
    let Ok(geometry) = Geometry::from_value(&query.value) else { return Ok(None) };
    if geometry.srid() != srid {
        return Ok(None);
    }
    let Some(bounds) = geometry.bounds() else { return Ok(None) };
    let Some(entries) = tree.nearest_candidates(bounds, crate::geometry::distance_domain()?, budget)? else { return Ok(None) };
    let entries = entries.map(|entry| entry.map(|(distance, envelope, row)| (distance, probe::key(envelope, RECTANGLE_COLUMNS), row)));
    Ok(Some(Box::new(entries)))
}

struct Projection {
    srid: i32,
}

impl KeyProjection for Projection {
    fn project(&self, key: &Key) -> Result<Key> {
        if matches!(key.0.as_slice(), [Value::Null]) {
            return Ok(key.clone());
        }
        bounds(key, self.srid).map(|bounds| probe::key(bounds, RECTANGLE_COLUMNS))
    }
}

pub(super) fn projection(srid: i32) -> Arc<dyn KeyProjection> {
    Arc::new(Projection { srid })
}

/// A window of a relationship true only of meeting operands, ST_Relate's
/// of a pattern it reads, or a distance comparison's (`spatial::window`).
pub(super) fn supports(predicate: &Predicate) -> bool {
    let operation = predicate.function.0.spatial_operation();
    let windowed = operation.is_some_and(SpatialOperation::windowed);
    let read = matches!(operation, Some(SpatialOperation::Relate | SpatialOperation::Distance));
    predicate.column == 0 && predicate.argument.is_some_and(|a| a < ARGUMENTS) && predicate.function.0.deterministic() && (windowed || read)
}

pub(super) fn bounds(key: &Key, srid: i32) -> Result<Bounds> {
    let [value] = key.0.as_slice() else { return Err(unsupported("received an invalid geometry key")) };
    let geometry = Geometry::from_value(value)?;
    if geometry.srid() != srid {
        return Err(SqlError::new(Code::Data, "[EXTENSION] the geometry index key has a different SRID from its column"));
    }
    keyed_envelope(&geometry)
}

enum Search {
    Empty,
    All,
    Window(Bounds),
    /// Entries whose envelopes may be within a radius of these bounds,
    /// if the tree certifies error-free distances; else every entry.
    Near(Bounds, Radius),
}

fn search(probe: &Probe, srid: i32) -> Result<Search> {
    let Probe::Predicate { predicate, value } = probe else {
        return if matches!(probe, Probe::All) { Ok(Search::All) } else { Err(unsupported("cannot compare projected geometry keys")) };
    };
    if !supports(predicate) {
        return Err(unsupported("does not support that spatial function"));
    }
    match predicate.function.0.spatial_operation() {
        Some(SpatialOperation::Intersects | SpatialOperation::Meets) => windowed(value, srid, Search::Empty),
        // ST_Equals holds of two empty sets, which an envelope window misses.
        Some(SpatialOperation::Equals) => windowed(value, srid, Search::All),
        Some(SpatialOperation::Relate) => related(value, srid),
        Some(SpatialOperation::Distance) => within::search(value, srid),
        Some(SpatialOperation::IsEmpty) | None => Err(unsupported("does not support that spatial function")),
    }
}

fn windowed(value: &Value, srid: i32, empty: Search) -> Result<Search> {
    if value.is_null() {
        return Ok(Search::Empty);
    }
    let geometry = Geometry::from_value(value)?;
    if geometry.srid() != srid {
        return Ok(Search::All);
    }
    Ok(geometry.bounds().map_or(empty, Search::Window))
}

/// The envelopes `matching` reads for `probe` (`Index::read_region`):
/// none of a search of every entry, or one that fails, which then reads as
/// the whole table. A distance window's is the widened window it scans,
/// or none where the tree reads every entry (D135).
pub(super) fn region(tree: &RTree, probe: &Probe, srid: i32) -> Option<Vec<Bounds>> {
    match search(probe, srid).ok()? {
        Search::Empty => Some(Vec::new()),
        Search::All => None,
        Search::Window(bounds) => Some(vec![bounds]),
        Search::Near(bounds, radius) => within::region(tree, (bounds, radius)),
    }
}

/// ST_Relate is null of a null argument, raising nothing. A pattern that
/// does not imply meeting may hold of disjoint geometries, so it reads
/// every entry; one that does reads the window ST_Intersects would.
fn related(value: &Value, srid: i32) -> Result<Search> {
    let Some([geometry, pattern]) = window::fields(value) else { return Ok(Search::All) };
    if geometry.is_null() || pattern.is_null() {
        return Ok(Search::Empty);
    }
    if !window::pattern_meets(pattern) {
        return Ok(Search::All);
    }
    windowed(geometry, srid, Search::Empty)
}

pub(super) fn matching(tree: &RTree, probe: &Probe, srid: i32, budget: &Arc<MemoryBudget>) -> Entries {
    let window = match search(probe, srid) {
        Ok(Search::Empty) => return Box::new(std::iter::empty()),
        Ok(Search::All) => None,
        Ok(Search::Window(bounds)) => Some(bounds),
        Ok(Search::Near(bounds, radius)) => return within::near(tree, (bounds, radius), budget),
        Err(error) => return Box::new(std::iter::once(Err(error))),
    };
    scanned(tree, window, budget)
}

/// The entries meeting `window`, or every entry, grouped by row id.
fn scanned(tree: &RTree, window: Option<Bounds>, budget: &Arc<MemoryBudget>) -> Entries {
    let scan = tree.scan(window);
    let buffered = super::clustered::Clustered::new(scan, window.map(|_| budget));
    Box::new(buffered.map(|entry| entry.map(|(bounds, row)| (probe::key(bounds, RECTANGLE_COLUMNS), row))))
}

pub(super) fn equal(tree: &RTree, key: &Key, srid: i32) -> Result<Entries> {
    if matches!(key.0.as_slice(), [Value::Null]) {
        return Ok(Box::new(std::iter::empty()));
    }
    let wanted = bounds(key, srid)?;
    Ok(Box::new(tree.scan(Some(wanted)).filter_map(move |entry| {
        entry.map(|(bounds, row)| (bounds == wanted).then(|| (probe::key(bounds, RECTANGLE_COLUMNS), row))).transpose()
    })))
}

pub(super) fn estimate(tree: &RTree, probe: &Probe, srid: i32) -> Option<usize> {
    let window = match search(probe, srid).ok()? {
        Search::Empty | Search::Near(_, Radius::None) => return Some(0),
        Search::All | Search::Near(_, Radius::All) => return None,
        Search::Window(bounds) => bounds,
        Search::Near(bounds, Radius::Of(radius)) => within::expanded(bounds, radius)?,
    };
    let mut count = 0_usize;
    for (at, entry) in tree.scan(Some(window)).enumerate() {
        if at >= MOST_COUNTED {
            return tree.estimated_matches(window).map(|estimated| estimated.max(count));
        }
        entry.ok()?;
        count = count.saturating_add(1);
    }
    Some(count)
}
