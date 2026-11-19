//! Distance comparisons' candidates (RULES.md D135): `ST_Distance(g, q)`
//! compared `<` or `<=` with a limit is true only of a row whose envelope
//! lower bound from `q`'s is within the limit's radius, as a geometry's
//! computed distance is at least that bound of its envelopes (D85). The
//! window is pruned only where the tree certifies, as a nearest search
//! does, that no distance it would skip could overflow; elsewhere every
//! entry is read, so the complete condition raises what a scan would.

use std::sync::Arc;

use super::{RECTANGLE_COLUMNS, Search, probe, scanned};
use crate::engine::rtree::RTree;
use crate::error::Result;
use crate::geometry::{Geometry, distance_domain};
use crate::index::Entries;
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::spatial::window::{self, Radius};
use crate::value::Value;

/// Relative room the window adds around the radius, far above the few
/// roundings between it and the lower bound it must contain.
const WINDOW_SLACK: f64 = 1.0e-9;

/// The search of a probe's ROW of geometry, limit and unit scale. A null
/// or empty geometry makes every distance null, raising nothing; one of
/// another SRID raises at the first row, so every entry is read then.
pub(super) fn search(value: &Value, srid: i32) -> Result<Search> {
    let Some([geometry, limit, scale]) = window::fields(value) else { return Ok(Search::All) };
    if geometry.is_null() {
        return Ok(Search::Empty);
    }
    let geometry = Geometry::from_value(geometry)?;
    if geometry.srid() != srid {
        return Ok(Search::All);
    }
    let Some(bounds) = geometry.bounds() else { return Ok(Search::Empty) };
    Ok(match window::radius(limit, scale) {
        Radius::All => Search::All,
        radius @ (Radius::None | Radius::Of(_)) => Search::Near(bounds, radius),
    })
}

/// The entries within `radius` of `bounds`, if the tree certifies them;
/// of a radius no distance compares true within, none. Without the
/// certificate, every entry. Grouped by row id for their rows' reads in
/// `budget`'s room, if a budget is given.
pub(super) fn near(tree: &RTree, (bounds, radius): (Bounds, Radius), budget: Option<&Arc<MemoryBudget>>) -> Entries {
    let (window, radius) = match read(tree, (bounds, radius)) {
        Err(error) => return Box::new(std::iter::once(Err(error))),
        Ok(Read::Every) => return scanned(tree, None, budget),
        Ok(Read::Nothing) => return Box::new(std::iter::empty()),
        Ok(Read::Window(window, radius)) => (window, radius),
    };
    let scan =
        tree.scan(Some(window)).filter(move |entry| !entry.as_ref().is_ok_and(|(found, _)| found.distance_lower_bound(bounds) > radius));
    let buffered = super::super::clustered::Clustered::new(scan, budget);
    Box::new(buffered.map(|entry| entry.map(|(found, row)| (probe::key(found, RECTANGLE_COLUMNS), row))))
}

/// What a distance search reads.
enum Read {
    /// Every entry: the tree cannot certify a window, or the radius is
    /// unbounded or too large for one.
    Every,
    /// No entry: the comparison is true of no distance.
    Nothing,
    /// The entries of the widened window within the radius.
    Window(Bounds, f64),
}

/// What the search of `radius` about `bounds` reads, as `near` does.
fn read(tree: &RTree, (bounds, radius): (Bounds, Radius)) -> Result<Read> {
    let certified = distance_domain().and_then(|domain| tree.within_domain(bounds, domain))?;
    Ok(match (certified, radius) {
        (false, _) | (true, Radius::All) => Read::Every,
        (true, Radius::None) => Read::Nothing,
        (true, Radius::Of(radius)) => expanded(bounds, radius).map_or(Read::Every, |window| Read::Window(window, radius)),
    })
}

/// The envelopes a distance search reads, for a serializable read
/// (`Index::read_region`, RULES.md D134): its widened window, none of a
/// search of no entry, or none at all where it reads every entry or fails.
pub(super) fn region(tree: &RTree, (bounds, radius): (Bounds, Radius)) -> Option<Vec<Bounds>> {
    match read(tree, (bounds, radius)).ok()? {
        Read::Every => None,
        Read::Nothing => Some(Vec::new()),
        Read::Window(window, _) => Some(vec![window]),
    }
}

/// `bounds` grown by `radius` on every side, rounded outward and held to
/// finite coordinates; none if the result is not a rectangle.
pub(super) fn expanded(bounds: Bounds, radius: f64) -> Option<Bounds> {
    let pad = radius * (1.0_f64 + WINDOW_SLACK);
    let low = bounds.lower().map(|v| (v - pad).next_down().max(-f64::MAX));
    let high = bounds.upper().map(|v| (v + pad).next_up().min(f64::MAX));
    Bounds::new(low, high).ok()
}
