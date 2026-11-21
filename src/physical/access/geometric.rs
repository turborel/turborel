//! \[DESIGN\] What a spatial conjunct costs beyond the rows it reads, and
//! how many rows a spatial probe's window finds, by the geometry column's
//! statistics (RULES.md D137):
//!
//! * a window's rows are the share of the column's envelopes its grid
//!   estimates the window to meet (`stats::EnvelopeStats::meeting`), so a
//!   broad window is estimated without reading the tree; a window the grid
//!   puts at no more than `COUNTED_ROWS` is counted by the index, exactly,
//!   as cheaply;
//! * each row a path reads evaluates the windowed conjuncts, a call each
//!   and the column's geometry decoded, a vertex at a time, so a scan pays
//!   it of every row and a probe of the rows it finds; the exact test of a
//!   row whose envelope meets the window, the same on either path, is not
//!   priced;
//! * a probe's rows lie near each other in the table as far as the column
//!   measures clustered, and each of those costs `CLUSTERED_ROWS` rows of
//!   a scan instead of a fetch.
//!
//! These choose a plan; none of them skips a row or certifies a result.

use crate::geometry::Geometry;
use crate::logical::Cond;
use crate::physical::estimate::{Known, conjunction};
use crate::physical::spatial::{Beside, recognized};
use crate::spatial::Bounds;
use crate::spatial::window::{self, Radius};
use crate::value::Value;

/// \[DESIGN\] What a spatial call on a row costs beyond its vertices, in
/// rows of a scan: the call made, its value's header read and its
/// envelope compared. Measured, a point decoded in 40 ns against a row
/// scanned in 229 ns (`bench/geometry_fetch.rs`, its weights).
const CALL_ROWS: f64 = 0.2;
/// \[DESIGN\] What a vertex of the column's value costs a call, in rows
/// of a scan: decoded. Measured, 6.2 ns a vertex of a polygon of 129
/// (`bench/geometry_fetch.rs`, its weights).
const VERTEX_ROWS: f64 = 0.03;
/// \[DESIGN\] What a row fetched next to the one before costs, in rows of
/// a scan: an entry at a time, its leaf found by the hint the last fetch
/// left, as an index-only scan's entry costs (`ENTRY_SCAN_ROWS`).
const CLUSTERED_ROWS: f64 = 2.0;
/// \[DESIGN\] The rows up to which the index's own count of a window is
/// taken: the R-tree counts that many entries exactly before it would
/// estimate (`index/rtree.rs`, `MOST_COUNTED`).
const COUNTED_ROWS: f64 = 1000.0;

/// What a geometry column's statistics tell of a spatial probe: the rows
/// its window meets, and how closely rows near in space lie in the table.
#[derive(Clone, Copy, Debug)]
pub(super) struct Stated {
    pub(super) rows: f64,
    pub(super) clustering: f64,
}

impl Stated {
    /// The rows a probe is taken to find by the statistics: none of a
    /// window few enough rows meet that the index counts them.
    pub(super) fn rows(stated: Option<&Stated>) -> Option<f64> {
        stated.map(|s| s.rows).filter(|rows| *rows > COUNTED_ROWS)
    }
}

/// What the statistics tell of the probe `value` of a windowed conjunct,
/// with `beside` its geometry, on `column`, of SRID `srid`, in a table of
/// `all` rows: none if they do not know the column's envelopes, or the
/// window is no rectangle the statistics can place.
pub(super) fn stated(value: &Value, beside: &Beside<'_>, (column, srid): (usize, i32), (known, all): (Known<'_>, f64)) -> Option<Stated> {
    let stats = known.stats?;
    let envelopes = stats.envelopes(column)?;
    let window = window_of(value, beside, srid)?;
    let present = 1.0_f64 - stats.nulls(column).unwrap_or(0.0_f64);
    Some(Stated { rows: all * present * envelopes.meeting(window), clustering: envelopes.clustering() })
}

/// The window a probe's value searches: its geometry's envelope, grown by
/// a distance's radius; none of a geometry of another SRID, an empty one,
/// or a radius of no rows or of all, which the index answers itself.
fn window_of(value: &Value, beside: &Beside<'_>, srid: i32) -> Option<Bounds> {
    let (geometry, radius) = match beside {
        Beside::Nothing => (value, None),
        Beside::Pattern(_) => (window::fields(value)?.first()?, None),
        Beside::Within(..) => {
            let [geometry, limit, scale] = window::fields(value)? else { return None };
            (geometry, Some(window::radius(limit, scale)))
        }
    };
    let bounds = Geometry::from_value(geometry).ok().filter(|g| g.srid() == srid)?.bounds()?;
    match radius {
        None => Some(bounds),
        Some(Radius::Of(r)) => Bounds::new(bounds.lower().map(|v| v - r), bounds.upper().map(|v| v + r)).ok(),
        Some(Radius::None | Radius::All) => None,
    }
}

/// What evaluating a table's windowed spatial conjuncts costs a row read,
/// and the vertices they decode.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Evaluation {
    /// In rows of a scan.
    pub(super) per_row: f64,
    /// Of the column's values, on average.
    pub(super) vertices: f64,
}

/// What evaluating the windowed spatial conjuncts of `conds` costs a row:
/// a call each, and a vertex of the column's values, on average. A
/// conjunct of a column whose envelopes the statistics do not know is not
/// priced, as before they are gathered.
pub(super) fn evaluation(conds: &[Cond], known: Known<'_>) -> Evaluation {
    let mut evaluation = Evaluation::default();
    for found in conds.iter().filter_map(recognized) {
        let Some((column, _, _)) = found.split() else { continue };
        let Some(stored) = known.stats.and_then(|s| s.envelopes(column)) else { continue };
        let vertices = stored.vertices();
        evaluation.per_row += CALL_ROWS + VERTEX_ROWS * vertices;
        evaluation.vertices += vertices;
    }
    evaluation
}

/// The share of a table access's rows a filter on `conds` keeps: the
/// conjunction's (`estimate::conjunction`), but of the windowed spatial conjuncts together
/// the share `windowed` the spatial probe priced is taken to keep, so the
/// filter's estimate is the probe's and not a guess.
pub(super) fn kept(conds: &[Cond], known: Known<'_>, windowed: Option<f64>) -> f64 {
    let is_windowed = |c: &&Cond| windowed.is_some() && recognized(c).is_some_and(|found| found.split().is_some());
    let (spatial, rest): (Vec<&Cond>, Vec<&Cond>) = conds.iter().partition(is_windowed);
    let share = if spatial.is_empty() { 1.0_f64 } else { windowed.unwrap_or(1.0_f64).clamp(0.0, 1.0) };
    let rest: Vec<Cond> = rest.into_iter().cloned().collect();
    share * conjunction(&rest, known)
}

/// What `est` rows fetched by a probe cost less than a fetch each, `fetch`,
/// the share `clustering` of them lying next to the one before, each then
/// `CLUSTERED_ROWS` rows of a scan, a row of which costs `row`.
pub(super) fn clustered_saving(est: f64, clustering: f64, (fetch, row): (f64, f64)) -> f64 {
    est * clustering.clamp(0.0, 1.0) * (fetch - CLUSTERED_ROWS * row).max(0.0_f64)
}

/// What EXPLAIN shows of a spatial probe priced against a scan: its rows
/// and where they were estimated, the vertices a row's evaluation tests,
/// the clustering, and the two costs.
#[derive(Clone, Copy, Debug)]
pub(super) struct Priced {
    pub(super) est: f64,
    pub(super) stated: bool,
    pub(super) clustering: f64,
    pub(super) probe: f64,
}

impl Priced {
    /// Its text, beside the scan's cost `scan` and the `evaluation`.
    pub(super) fn text(&self, scan: f64, evaluation: Evaluation) -> String {
        let by = if self.stated { "statistics" } else { "the index" };
        format!(
            "[spatial: est. rows={:.0} by {by}, vertices={:.0}, clustering={:.2}, probe cost={:.1}, scan cost={scan:.1}]",
            self.est, evaluation.vertices, self.clustering, self.probe
        )
    }
}
