//! Geometry candidates use envelope projections, including a harmless
//! origin projection for empty values. Mismatched query references scan
//! every entry so SQL, not planning or pruning, decides whether an error
//! is reached. No function callback occurs under a tree latch.

use std::sync::Arc;

use super::{MOST_COUNTED, RECTANGLE_COLUMNS, probe, unsupported};
use crate::engine::rtree::RTree;
use crate::error::{Code, Result, SqlError};
use crate::geometry::{Geometry, SpatialOperation};
use crate::index::{Entries, Key, KeyProjection, NearestQuery, NeighbourEntries, Predicate, Probe};
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::value::Value;

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

pub(super) fn supports(predicate: &Predicate) -> bool {
    predicate.column == 0
        && predicate.argument < ARGUMENTS
        && predicate.function.0.deterministic()
        && predicate.function.0.spatial_operation() == Some(SpatialOperation::Intersects)
}

pub(super) fn bounds(key: &Key, srid: i32) -> Result<Bounds> {
    let [value] = key.0.as_slice() else { return Err(unsupported("received an invalid geometry key")) };
    let geometry = Geometry::from_value(value)?;
    if geometry.srid() != srid {
        return Err(SqlError::new(Code::Data, "[EXTENSION] the geometry index key has a different SRID from its column"));
    }
    geometry.bounds().map_or_else(|| Bounds::point([0.0_f64, 0.0_f64]), Ok)
}

enum Search {
    Empty,
    All,
    Window(Bounds),
}

fn search(probe: &Probe, srid: i32) -> Result<Search> {
    let Probe::Predicate { predicate, value } = probe else {
        return if matches!(probe, Probe::All) { Ok(Search::All) } else { Err(unsupported("cannot compare projected geometry keys")) };
    };
    if !supports(predicate) {
        return Err(unsupported("does not support that spatial function"));
    }
    if value.is_null() {
        return Ok(Search::Empty);
    }
    let geometry = Geometry::from_value(value)?;
    if geometry.srid() != srid {
        return Ok(Search::All);
    }
    Ok(geometry.bounds().map_or(Search::Empty, Search::Window))
}

pub(super) fn matching(tree: &RTree, probe: &Probe, srid: i32) -> Entries {
    let window = match search(probe, srid) {
        Ok(Search::Empty) => return Box::new(std::iter::empty()),
        Ok(Search::All) => None,
        Ok(Search::Window(bounds)) => Some(bounds),
        Err(error) => return Box::new(std::iter::once(Err(error))),
    };
    Box::new(tree.scan(window).map(|entry| entry.map(|(bounds, row)| (probe::key(bounds, RECTANGLE_COLUMNS), row))))
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
        Search::Empty => return Some(0),
        Search::All => return None,
        Search::Window(bounds) => bounds,
    };
    let mut count = 0_usize;
    for (at, entry) in tree.scan(Some(window)).enumerate() {
        if at >= MOST_COUNTED {
            return None;
        }
        entry.ok()?;
        count = count.saturating_add(1);
    }
    Some(count)
}
