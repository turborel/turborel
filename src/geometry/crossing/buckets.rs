//! Whether any edge of one set of chains meets any edge of another, by
//! buckets of a dyadic grid (RULES.md D85). Only the two envelopes' overlap
//! can hold a meeting point, so the grid covers it alone. The first set's
//! edges are filed under every cell they may touch; each edge of the
//! second set is tested only against the edges filed under the cells it
//! may touch. Both covers are supersets of the cells an edge touches
//! (`grid.rs`), and two closed segments that meet share a point, so they
//! share a cell: no meeting pair is skipped, and every test is the exact
//! segment test the nested loops use. Where the grid cannot be exact the
//! caller keeps its nested loops.

use super::segments_intersect;
use crate::geometry::grid::Grid;
use crate::geometry::shape::Point;
use crate::spatial::Bounds;

/// Grid cells per filed edge: enough to separate edges, few enough that
/// filing them costs less than the pairs it saves.
const CELLS_PER_EDGE: u32 = 2;

/// Whether an edge of `left` meets an edge of `right`, or none if the
/// grid cannot be built exactly or its arithmetic declines.
#[cfg(test)]
pub(super) fn any_crossing(left: &[&[Point]], right: &[&[Point]]) -> Option<bool> {
    let (Some(of_left), Some(of_right)) = (envelope(left), envelope(right)) else { return Some(false) };
    let Some(overlap) = of_left.intersection(of_right) else { return Some(false) };
    let filed = edges(left, overlap);
    let probes = edges(right, overlap);
    if filed == 0 || probes == 0 {
        return Some(false);
    }
    Filed::new(left, overlap, filed.checked_add(probes)?)?.crosses(right)
}

/// One set's edges, those whose envelopes meet a region, filed in a grid
/// over that region, for the edges of other sets to be tested against.
/// A prepared polygon keeps one over its envelope for many operands.
pub(in crate::geometry) struct Filed {
    grid: Grid,
    buckets: Buckets,
    within: Bounds,
}

impl Filed {
    /// Files the edges of `chains` that meet `within`, in a grid sized for
    /// `edges` edges; none if the grid cannot be exact there.
    pub(in crate::geometry) fn new(chains: &[&[Point]], within: Bounds, edges: usize) -> Option<Self> {
        let grid = Grid::new(region(within)?, edges, CELLS_PER_EDGE)?;
        let buckets = Buckets::new(&grid, chains, within)?;
        Some(Self { grid, buckets, within })
    }

    /// Whether an edge of `chains` meets a filed edge, or none if the
    /// grid's arithmetic declines. An edge missing the region cannot meet
    /// a filed edge and is not tested.
    pub(in crate::geometry) fn crosses(&self, chains: &[&[Point]]) -> Option<bool> {
        let mut found = false;
        for chain in chains {
            for (c, d) in super::segments(chain) {
                if !touches(c, d, self.within) {
                    continue;
                }
                let covered = self.grid.cover((c, d), |row, first, last| {
                    let filed = self.buckets.within(&self.grid, row, first, last)?;
                    let hit = filed.iter().any(|&(a, b)| segments_intersect(a, b, c, d));
                    found |= hit;
                    (!hit).then_some(())
                });
                if found {
                    return Some(true);
                }
                covered?;
            }
        }
        Some(false)
    }

    /// The bytes its cells and filed edges hold.
    pub(in crate::geometry) fn bytes(&self) -> Option<usize> {
        let starts = self.buckets.starts.capacity().checked_mul(size_of::<usize>())?;
        starts.checked_add(self.buckets.edges.capacity().checked_mul(size_of::<(Point, Point)>())?)
    }
}

/// The first set's edges filed by cell: each cell's edges contiguous, in
/// the order of the cells, row by row.
struct Buckets {
    starts: Vec<usize>,
    edges: Vec<(Point, Point)>,
}

impl Buckets {
    fn new(grid: &Grid, chains: &[&[Point]], overlap: Bounds) -> Option<Self> {
        let cells = grid.rows.checked_mul(grid.cols)?;
        let mut counts = vec![0_usize; cells.checked_add(1)?];
        file(grid, chains, overlap, |cell, _| {
            let count = counts.get_mut(cell)?;
            *count = count.checked_add(1)?;
            Some(())
        })?;
        let mut total = 0_usize;
        for count in &mut counts {
            let here = *count;
            *count = total;
            total = total.checked_add(here)?;
        }
        let mut next = counts.clone();
        let mut edges = vec![([0.0_f64; 2], [0.0_f64; 2]); total];
        file(grid, chains, overlap, |cell, edge| {
            let at = next.get_mut(cell)?;
            *edges.get_mut(*at)? = edge;
            *at = at.checked_add(1)?;
            Some(())
        })?;
        Some(Self { starts: counts, edges })
    }

    /// The edges filed under the cells `first..=last` of `row`.
    fn within(&self, grid: &Grid, row: usize, first: usize, last: usize) -> Option<&[(Point, Point)]> {
        let base = row.checked_mul(grid.cols)?;
        let from = *self.starts.get(base.checked_add(first)?)?;
        let to = *self.starts.get(base.checked_add(last)?.checked_add(1)?)?;
        self.edges.get(from..to)
    }
}

/// Calls `put` with each cell an edge of `chains` in the overlap may
/// touch, and the edge.
fn file(grid: &Grid, chains: &[&[Point]], overlap: Bounds, mut put: impl FnMut(usize, (Point, Point)) -> Option<()>) -> Option<()> {
    for chain in chains {
        for (a, b) in super::segments(chain) {
            if !touches(a, b, overlap) {
                continue;
            }
            grid.cover((a, b), |row, first, last| {
                let base = row.checked_mul(grid.cols)?;
                for col in first..=last {
                    put(base.checked_add(col)?, (a, b))?;
                }
                Some(())
            })?;
        }
    }
    Some(())
}

/// A region with area covering the overlap: the overlap itself, or, where
/// chains touch along a horizontal or vertical line, a square on it.
fn region(overlap: Bounds) -> Option<Bounds> {
    let ([x0, y0], [x1, y1]) = (overlap.lower(), overlap.upper());
    let (width, height) = (x1 - x0, y1 - y0);
    match (width > 0.0_f64, height > 0.0_f64) {
        (true, true) => Some(overlap),
        (true, false) => Bounds::new([x0, y0], [x1, y0 + width]).ok(),
        (false, true) => Bounds::new([x0, y0], [x0 + height, y1]).ok(),
        (false, false) => None,
    }
}

/// Whether a segment's envelope meets the overlap.
fn touches([ax, ay]: Point, [bx, by]: Point, overlap: Bounds) -> bool {
    let [x0, y0] = overlap.lower();
    let [x1, y1] = overlap.upper();
    ax.min(bx) <= x1 && ax.max(bx) >= x0 && ay.min(by) <= y1 && ay.max(by) >= y0
}

/// The edges of `chains` whose envelopes meet the overlap.
#[cfg(test)]
fn edges(chains: &[&[Point]], overlap: Bounds) -> usize {
    chains.iter().map(|chain| super::segments(chain).filter(|&(a, b)| touches(a, b, overlap)).count()).sum()
}

/// The envelope of the chains' points, none for no points.
#[cfg(test)]
fn envelope(chains: &[&[Point]]) -> Option<Bounds> {
    let mut points = chains.iter().copied().flatten();
    let &[x, y] = points.next()?;
    let (low, high) = points.fold(([x, y], [x, y]), |([lx, ly], [hx, hy]), &[px, py]| ([lx.min(px), ly.min(py)], [hx.max(px), hy.max(py)]));
    Bounds::new(low, high).ok()
}

#[cfg(test)]
mod tests;
