//! \[DESIGN\] Bounds on a point's distance to a prepared polygon from its
//! cell certificates (RULES.md D85): each cell's Chebyshev distance, in
//! cells, to the nearest cell edges may touch, by a two-pass chamfer over
//! the grid, exact for that metric (Rosenfeld and Pfaltz, 1966). A point's
//! distance to the polygon, as a point set, is at least k - 1 cells for k
//! its cell's distance: an edge point lies in a cell at least k away, and
//! the cells between are whole. It is at most k + 2 cells along a
//! diagonal: a marked cell holds an edge point or sits beside one that
//! does, as the columns an edge is said to touch are widened by less than
//! a cell (`grid::columns`). An interior cell's distance is zero. A point
//! beyond the grid is bounded through the cell nearest it, with how far
//! beyond the grid it lies, in cells, added: along the axis realizing its
//! distance to an edge's cell, at least that far more; along any, at
//! most the farther.

use std::sync::Arc;

use super::{BOUNDARY, INTERIOR, NEAR_INSIDE, NEAR_OUTSIDE, NEAR_SEVERAL};
use crate::geometry::grid::Grid;
use crate::geometry::shape::Point;
use crate::memory::{MemoryBudget, Reservation};

/// A cell at this distance, the most counted, may lie farther.
const FAR: u16 = u16::MAX;

/// Each cell's distance, in cells, to the nearest cell edges may touch.
pub(super) struct Within {
    distances: Vec<u16>,
    _held: Reservation,
}

impl Within {
    /// The distances of the cells of `grid` in `states`, if the budget
    /// grants the room.
    pub(super) fn new(grid: &Grid, states: &[u8], budget: &Arc<MemoryBudget>) -> Option<Box<Self>> {
        let cells = states.len();
        let held = budget.reserve(cells.checked_mul(size_of::<u16>())?, "a prepared polygon's cell distances").ok()?;
        let mut distances = Vec::new();
        distances.try_reserve_exact(cells).ok()?;
        distances.extend(states.iter().map(|&state| if marked(state) { 0 } else { FAR }));
        forward(grid, &mut distances)?;
        backward(grid, &mut distances)?;
        Some(Box::new(Self { distances, _held: held }))
    }

    /// Bounds on the distance from `point` to the polygon, as a point set,
    /// in its coordinates' units: none if the point's cell number is not
    /// exact. The upper bound is infinite past the farthest distance
    /// counted.
    pub(super) fn bounds(&self, grid: &Grid, states: &[u8], point: Point) -> Option<(f64, f64)> {
        let ((row, col), [beyond_x, beyond_y]) = grid.nearest(point)?;
        let at = row.checked_mul(grid.cols)?.checked_add(col)?;
        let inside = beyond_x <= 0.0_f64 && beyond_y <= 0.0_f64 && *states.get(at)? == INTERIOR;
        if inside {
            return Some((0.0_f64, 0.0_f64));
        }
        let counted = *self.distances.get(at)?;
        let cells = f64::from(counted);
        let (near, far) = (beyond_x.min(beyond_y), beyond_x.max(beyond_y));
        let lower = ((cells + near).max(far) - 1.0_f64).max(0.0_f64) * grid.side();
        let upper =
            if counted == FAR { f64::INFINITY } else { ((cells + far + 2.0_f64) * grid.side() * std::f64::consts::SQRT_2).next_up() };
        Some((lower, upper))
    }
}

/// Whether edges may touch a cell in state `state`.
fn marked(state: u8) -> bool {
    matches!(state, BOUNDARY | NEAR_OUTSIDE | NEAR_INSIDE | NEAR_SEVERAL)
}

/// The first pass, in raster order: each cell's distance is at most one
/// more than that of the cell left of it, or of the three in the row
/// below it.
fn forward(grid: &Grid, distances: &mut [u16]) -> Option<()> {
    let cols = grid.cols;
    for row in 0..grid.rows {
        let (done, rest) = distances.split_at_mut_checked(row.checked_mul(cols)?)?;
        let line = rest.get_mut(..cols)?;
        let below = row.checked_sub(1).and_then(|r| done.get(r.checked_mul(cols)?..));
        let mut before = FAR;
        for (col, cell) in line.iter_mut().enumerate() {
            let least = beside(below, col, cols, (*cell).min(before.saturating_add(1)));
            *cell = least;
            before = least;
        }
    }
    Some(())
}

/// The second pass, in reverse raster order: at most one more than the
/// cell right of it, or the three in the row above it.
fn backward(grid: &Grid, distances: &mut [u16]) -> Option<()> {
    let cols = grid.cols;
    for row in (0..grid.rows).rev() {
        let (_, rest) = distances.split_at_mut_checked(row.checked_mul(cols)?)?;
        let (line, after) = rest.split_at_mut_checked(cols)?;
        let above = after.get(..cols);
        let mut before = FAR;
        for (col, cell) in line.iter_mut().enumerate().rev() {
            let least = beside(above, col, cols, (*cell).min(before.saturating_add(1)));
            *cell = least;
            before = least;
        }
    }
    Some(())
}

/// `least`, or one more than the least of the cells of `near`, the row
/// beside, at column `col` and the two next to it, if that is less.
fn beside(near: Option<&[u16]>, col: usize, cols: usize, least: u16) -> u16 {
    let Some(near) = near else { return least };
    let around = near.get(col.saturating_sub(1)..col.saturating_add(2).min(cols)).unwrap_or_default();
    around.iter().fold(least, |least, &cell| least.min(cell.saturating_add(1)))
}
