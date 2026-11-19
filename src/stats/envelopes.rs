//! \[DESIGN\] Statistics of a geometry column's envelopes (RULES.md D137),
//! gathered in the same pass as the column's other statistics:
//!
//! * a grid of `GRID_SIDE` × `GRID_SIDE` cells over the frame of every
//!   envelope seen, each holding the share of a uniform sample of the
//!   envelopes whose centres fall in it and their mean width and height,
//!   from which the share of rows a window meets is estimated without
//!   reading an index;
//! * the coordinates a value holds, on average, which decoding and testing
//!   it grow with;
//! * how closely rows next to each other in the table lie in space: one
//!   less the mean distance between the centres of rows read one after
//!   another over that between random pairs of the sample, 1 of a table
//!   laid out in space, near 0 of one in no spatial order.
//!
//! The sample is kept as Vitter's Algorithm R keeps one (`parts::Reservoir`),
//! envelopes alone, so a column of polygons of many vertices takes no more
//! memory to gather than one of points.

use super::parts::{count, xorshift};
use crate::geometry::Geometry;
use crate::spatial::Bounds;

/// \[DESIGN\] Cells along each side of the grid: 256 in all, few enough
/// that an estimate visits every cell in a few microseconds, enough that a
/// window of a sixteenth of the frame's side is placed within a cell.
pub const GRID_SIDE: usize = 16;
/// The seed of the sample's random choices, and of the random pairs.
const SEED: u64 = 0x5851_F42D_4C95_7F2D;

/// One cell of the grid: the share of the sampled envelopes whose centres
/// fall in it, and their mean extent.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    share: f64,
    width: f64,
    height: f64,
}

/// What a geometry column's envelopes are like.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeStats {
    /// The envelope of every nonempty value seen, if any.
    frame: Option<Bounds>,
    /// `GRID_SIDE` rows of `GRID_SIDE` cells, from the frame's lower left.
    cells: Vec<Cell>,
    /// The share of the non-null values that are empty.
    empty: f64,
    /// The coordinates a non-null value holds, on average.
    vertices: f64,
    /// How closely rows near in the table lie near in space, 0 to 1.
    clustering: f64,
}

impl EnvelopeStats {
    /// The share of the column's non-null values whose envelopes meet
    /// `window`: of each cell, the share of its centres, taken as spread
    /// evenly over it, that lie within the window grown by half the
    /// cell's mean extent. An empty value meets none.
    pub fn meeting(&self, window: Bounds) -> f64 {
        let Some(frame) = self.frame else { return 0.0_f64 };
        let (low, high) = (window.lower(), window.upper());
        let mut share = 0.0_f64;
        for (at, cell) in self.cells.iter().enumerate() {
            let (column, row) = (at.checked_rem(GRID_SIDE).unwrap_or_default(), at.checked_div(GRID_SIDE).unwrap_or_default());
            let (x, y) = (span(frame, 0, column), span(frame, 1, row));
            let (half_x, half_y) = (cell.width / 2.0_f64, cell.height / 2.0_f64);
            let across = along(x, (low[0] - half_x, high[0] + half_x));
            let up = along(y, (low[1] - half_y, high[1] + half_y));
            share += cell.share * across * up;
        }
        (share * (1.0_f64 - self.empty)).clamp(0.0, 1.0)
    }

    /// The share of `other`'s values a value of this column meets, on
    /// average (RULES.md D138): each cell's share of a window of its mean
    /// extent at its middle, as `meeting` estimates it, by the share of
    /// this column's values in the cell. Empty values meet nothing.
    pub fn joined(&self, other: &EnvelopeStats) -> f64 {
        let Some(frame) = self.frame else { return 0.0_f64 };
        let mut share = 0.0_f64;
        for (at, cell) in self.cells.iter().enumerate().filter(|(_, cell)| cell.share > 0.0_f64) {
            let (column, row) = (at.checked_rem(GRID_SIDE).unwrap_or_default(), at.checked_div(GRID_SIDE).unwrap_or_default());
            let (x, y) = (span(frame, 0, column), span(frame, 1, row));
            let middle = [f64::midpoint(x.0, x.1), f64::midpoint(y.0, y.1)];
            let (across, up) = (cell.width / 2.0_f64, cell.height / 2.0_f64);
            let Ok(window) = Bounds::new([middle[0] - across, middle[1] - up], [middle[0] + across, middle[1] + up]) else {
                continue;
            };
            share += cell.share * other.meeting(window);
        }
        (share * (1.0_f64 - self.empty)).clamp(0.0, 1.0)
    }

    /// The coordinates a non-null value holds, on average.
    pub fn vertices(&self) -> f64 {
        self.vertices
    }

    /// How closely rows near in the table lie near in space: 1 of a table
    /// laid out in space, 0 of one in no spatial order.
    pub fn clustering(&self) -> f64 {
        self.clustering
    }
}

/// The span of the cell `at` along `axis` of `frame`'s grid.
fn span(frame: Bounds, axis: usize, at: usize) -> (f64, f64) {
    let (low, high) = (frame.lower().get(axis).copied().unwrap_or_default(), frame.upper().get(axis).copied().unwrap_or_default());
    let side = count(u64::try_from(GRID_SIDE).unwrap_or(u64::MAX));
    let edge = |n: usize| low + (high - low) * count(u64::try_from(n).unwrap_or(u64::MAX)) / side;
    (edge(at), if at.saturating_add(1) >= GRID_SIDE { high } else { edge(at.saturating_add(1)) })
}

/// The share of `(low, high)` that `(from, to)` covers; of a point, 1 if
/// it lies within, else 0.
fn along((low, high): (f64, f64), (from, to): (f64, f64)) -> f64 {
    if high > low {
        ((to.min(high) - from.max(low)) / (high - low)).clamp(0.0, 1.0)
    } else if from <= low && low <= to {
        1.0_f64
    } else {
        0.0_f64
    }
}

/// The centre of `bounds`, halved before it is summed so it cannot
/// overflow.
fn centre(bounds: Bounds) -> [f64; 2] {
    let (low, high) = (bounds.lower(), bounds.upper());
    [low[0] / 2.0_f64 + high[0] / 2.0_f64, low[1] / 2.0_f64 + high[1] / 2.0_f64]
}

/// The distance between two points, along each axis summed.
fn apart(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs()
}

/// \[DESIGN\] Gathers a geometry column's envelope statistics, in one pass
/// in the table's order.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeBuilder {
    geometries: u64,
    empty: u64,
    vertices: u64,
    frame: Option<Bounds>,
    sample: Vec<Bounds>,
    most: usize,
    random: u64,
    /// The centre of the last nonempty value, and the distances between
    /// each and the one before, summed, and counted.
    last: Option<[f64; 2]>,
    steps: f64,
    stepped: u64,
}

impl EnvelopeBuilder {
    /// A builder sampling at most `most` envelopes.
    pub fn keeping(most: usize) -> EnvelopeBuilder {
        EnvelopeBuilder { most, random: SEED, ..EnvelopeBuilder::default() }
    }

    /// Adds `geometry`, a non-null value of the column, the next in the
    /// table's order.
    pub fn add(&mut self, geometry: &Geometry) {
        self.geometries = self.geometries.saturating_add(1);
        self.vertices = self.vertices.saturating_add(u64::try_from(geometry.vertices()).unwrap_or(u64::MAX));
        let Some(bounds) = geometry.bounds() else {
            self.empty = self.empty.saturating_add(1);
            return;
        };
        self.frame = Some(self.frame.map_or(bounds, |frame| frame.union(bounds)));
        let here = centre(bounds);
        if let Some(last) = self.last.replace(here) {
            self.steps += apart(last, here);
            self.stepped = self.stepped.saturating_add(1);
        }
        let seen = self.geometries.saturating_sub(self.empty);
        if self.sample.len() < self.most {
            self.sample.push(bounds);
            return;
        }
        // Algorithm R: the n-th envelope replaces a kept one with chance
        // `most` / n.
        let pick = xorshift(&mut self.random).checked_rem(seen).unwrap_or_default();
        if let Some(kept) = usize::try_from(pick).ok().and_then(|i| self.sample.get_mut(i)) {
            *kept = bounds;
        }
    }

    /// The statistics of the values added, if any was.
    pub fn finish(self) -> Option<EnvelopeStats> {
        let geometries = count(self.geometries);
        if geometries <= 0.0_f64 {
            return None;
        }
        let (empty, vertices) = (count(self.empty) / geometries, count(self.vertices) / geometries);
        let clustering = self.clustering();
        let frame = self.frame.filter(|f| f.upper().iter().zip(f.lower()).all(|(h, l)| (h - l).is_finite()));
        let cells = frame.map(|f| cells(f, &self.sample)).unwrap_or_default();
        Some(EnvelopeStats { frame, cells, empty, vertices, clustering })
    }

    /// One less the mean step between rows in the table's order over the
    /// mean distance between random pairs of the sample, within 0 to 1;
    /// 0 if there is no step, or no distance.
    fn clustering(&self) -> f64 {
        let length = u64::try_from(self.sample.len()).unwrap_or(u64::MAX);
        if self.stepped == 0 || length == 0 {
            return 0.0_f64;
        }
        let mut random = SEED.rotate_left(17);
        let paired: f64 = self
            .sample
            .iter()
            .filter_map(|&a| {
                let at = usize::try_from(xorshift(&mut random).checked_rem(length)?).ok()?;
                Some(apart(centre(a), centre(*self.sample.get(at)?)))
            })
            .sum();
        let (step, pair) = (self.steps / count(self.stepped), paired / count(length));
        let ratio = 1.0_f64 - step / pair;
        if ratio.is_finite() { ratio.clamp(0.0, 1.0) } else { 0.0_f64 }
    }
}

/// The grid of `sample`'s envelopes over `frame`.
fn cells(frame: Bounds, sample: &[Bounds]) -> Vec<Cell> {
    let mut cells = vec![Cell::default(); GRID_SIDE.saturating_mul(GRID_SIDE)];
    let mut counts = vec![0_u64; cells.len()];
    // The cell along an axis is the count of the inner edges at or below
    // the centre, so a centre on the frame's upper edge is in the last.
    let placed = |axis: usize, at: f64| (1..GRID_SIDE).filter(|&n| at >= span(frame, axis, n).0).count();
    for bounds in sample {
        let here = centre(*bounds);
        let at = placed(1, here[1]).saturating_mul(GRID_SIDE).saturating_add(placed(0, here[0]));
        let (low, high) = (bounds.lower(), bounds.upper());
        if let (Some(cell), Some(n)) = (cells.get_mut(at), counts.get_mut(at)) {
            cell.width += high[0] - low[0];
            cell.height += high[1] - low[1];
            *n = n.saturating_add(1);
        }
    }
    let total = count(u64::try_from(sample.len()).unwrap_or(u64::MAX)).max(1.0_f64);
    for (cell, n) in cells.iter_mut().zip(counts) {
        let n = count(n);
        if n > 0.0_f64 {
            *cell = Cell { share: n / total, width: cell.width / n, height: cell.height / n };
        }
    }
    cells
}

#[cfg(test)]
mod tests;
