//! \[DESIGN\] Exact cell certificates for repeated point location in a
//! polygon (RULES.md D85). A prepared polygon gets a grid of dyadic cells,
//! squares of side 2^k at multiples of 2^k: every cell bound is an exact
//! binary64 number, and a point's cell is found exactly by multiplying by
//! 2^-k. A cell a boundary edge may touch is a boundary cell. Marking is
//! conservative: each edge's columns in a row are widened by a bound on the
//! floating error of locating them. Every other cell holds no boundary
//! point, so it is wholly interior or wholly exterior, and the same for
//! each ring. It is labelled from the exact half-open crossings of its
//! row's centre line: each crossing lies in a marked cell of its edge, so
//! it is wholly left or right of an unmarked cell, and comparing columns
//! decides it. Each ring's parity is kept, and shells, holes and parts
//! combine as exact point location combines them, so a label never
//! differs from it, valid topology or not. A point in a labelled cell is
//! then located by one lookup; one in a boundary cell takes the exact
//! edge tests. Lucene precomputes a grid of cell relations for polygon
//! queries likewise (`GeoEncodingUtils.createComponentPredicate`), with
//! approximate rather than exact cells.

use std::sync::Arc;

use crate::geometry::crossing::segments;
use crate::geometry::shape::{MULTIPOLYGON, Point, Shape};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;

use crate::geometry::grid::Grid;

/// A cell no boundary point touches, outside every polygon.
const EXTERIOR: u8 = 0;
/// A cell no boundary point touches, inside a polygon.
const INTERIOR: u8 = 1;
/// A cell a boundary edge may touch.
const BOUNDARY: u8 = 2;
/// \[DESIGN\] Polygons with fewer edges are located as fast by a scan.
pub(super) const MIN_EDGES: usize = 32;
/// Cells per edge: fine enough that most cells hold no edge.
const CELLS_PER_EDGE: u32 = 64;

/// The edges of a polygon or multipolygon, none for another kind; counted
/// without allocating, as every prepared operand asks.
pub(super) fn edges(shape: &Shape) -> Option<usize> {
    let rings = |rings: &[Vec<Point>]| rings.iter().map(|ring| ring.len().saturating_sub(1)).sum::<usize>();
    match shape {
        Shape::Polygon(polygon) => Some(rings(polygon)),
        Shape::Multi(code, parts) => {
            if *code != MULTIPOLYGON {
                return None;
            }
            parts.iter().try_fold(0_usize, |sum, part| match part {
                Shape::Polygon(polygon) => sum.checked_add(rings(polygon)),
                Shape::Point(_) | Shape::Line(_) | Shape::Multi(_, _) => None,
            })
        }
        Shape::Point(_) | Shape::Line(_) => None,
    }
}

/// A ring of a polygon: its polygon, and whether it is that polygon's shell.
#[derive(Clone, Copy)]
struct Ring {
    polygon: usize,
    shell: bool,
}

/// A labelled grid over a prepared polygon's envelope.
pub(super) struct Cells {
    grid: Grid,
    states: Vec<u8>,
    _held: Reservation,
}

impl Cells {
    /// Certificates for `shape`, a polygon or multipolygon with envelope
    /// `bounds`, if it has enough edges, its coordinates allow an exact
    /// grid, and the budget grants the room.
    pub(super) fn new(shape: &Shape, bounds: Bounds, budget: &Arc<MemoryBudget>) -> Option<Box<Self>> {
        let polygons = polygons(shape)?;
        let edges = polygons.iter().copied().flatten().map(|ring| ring.len().saturating_sub(1)).sum::<usize>();
        if edges < MIN_EDGES {
            return None;
        }
        let grid = Grid::new(bounds, edges, CELLS_PER_EDGE)?;
        let rings = ring_list(&polygons);
        let crossings = polygons
            .iter()
            .copied()
            .flatten()
            .try_fold(0_usize, |sum, ring| segments(ring).try_fold(sum, |sum, edge| sum.checked_add(grid.rows_of(edge)?.count())))?;
        let cells = grid.rows.checked_mul(grid.cols)?;
        let bytes = cells.checked_add(crossings.checked_mul(size_of::<Crossing>())?)?;
        let mut held = budget.reserve(bytes, "a prepared polygon's cell certificates").ok()?;
        let mut states = Vec::new();
        states.try_reserve_exact(cells).ok()?;
        states.resize(cells, EXTERIOR);
        let mut found = Vec::new();
        found.try_reserve_exact(crossings).ok()?;
        for (ring, points) in polygons.iter().copied().flatten().enumerate() {
            for edge in segments(points) {
                mark(&grid, edge, ring, &mut states, &mut found)?;
            }
        }
        label(&grid, &rings, &mut states, &mut found)?;
        drop(found);
        held.resize(states.capacity()).ok()?;
        Some(Box::new(Self { grid, states, _held: held }))
    }

    /// Whether `point` is in the polygon's closed point set, if its cell
    /// decides it: inside an interior cell or outside in an exterior one.
    pub(super) fn decide(&self, point: Point) -> Option<bool> {
        let at = self.grid.cell(point)?;
        match *self.states.get(at)? {
            INTERIOR => Some(true),
            EXTERIOR => Some(false),
            _ => None,
        }
    }
}

/// Marks the cells an edge of `ring` may touch, and notes each row whose
/// centre line it crosses, by the half-open rule exact point location
/// uses, with the first column it was marked in there.
fn mark(grid: &Grid, (a, b): (Point, Point), ring: usize, states: &mut [u8], found: &mut Vec<Crossing>) -> Option<()> {
    let ([_, ay], [_, by]) = (a, b);
    grid.cover((a, b), |row, first, last| {
        let base = row.checked_mul(grid.cols)?;
        for state in states.get_mut(base.checked_add(first)?..=base.checked_add(last)?)? {
            *state = BOUNDARY;
        }
        let centre = grid.centre(row)?;
        if (ay > centre) != (by > centre) {
            found.push(Crossing { row, start: first, ring });
        }
        Some(())
    })
}

/// A crossing of a row's centre line by an edge of `ring`, whose cells in
/// that row start at column `start`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Crossing {
    row: usize,
    start: usize,
    ring: usize,
}

/// Labels every unmarked cell. Each row is swept from right to left; a
/// crossing whose cells start right of a column is right of that column's
/// centre, so passing it flips its ring's parity for every cell to its
/// left. Polygons combine as exact location combines them: inside a
/// polygon is inside its shell and no hole, and inside any polygon is
/// inside.
fn label(grid: &Grid, rings: &[Ring], states: &mut [u8], crossings: &mut [Crossing]) -> Option<()> {
    crossings.sort_unstable();
    let polygons = rings.iter().map(|ring| ring.polygon.saturating_add(1)).max().unwrap_or(0);
    let mut parity = Parity::new(rings, polygons)?;
    let mut rest: &[Crossing] = crossings;
    for row in 0..grid.rows {
        let split = rest.partition_point(|c| c.row == row);
        let (here, after) = rest.split_at_checked(split)?;
        rest = after;
        parity.clear();
        let mut pending = here.iter().rev().peekable();
        for col in (0..grid.cols).rev() {
            while let Some(crossing) = pending.next_if(|c| c.start > col) {
                parity.flip(crossing.ring)?;
            }
            let state = states.get_mut(row.checked_mul(grid.cols)?.checked_add(col)?)?;
            if *state != BOUNDARY {
                *state = if parity.inside() { INTERIOR } else { EXTERIOR };
            }
        }
    }
    Some(())
}

/// The parity of each ring's crossings right of a point, and the polygons
/// it puts the point inside.
struct Parity<'a> {
    rings: &'a [Ring],
    odd: Vec<bool>,
    /// Per polygon: its shell's parity and its holes with odd parity.
    polygons: Vec<(bool, usize)>,
    inside: usize,
}

impl<'a> Parity<'a> {
    fn new(rings: &'a [Ring], polygons: usize) -> Option<Self> {
        let mut odd = Vec::new();
        odd.try_reserve_exact(rings.len()).ok()?;
        odd.resize(rings.len(), false);
        let mut each = Vec::new();
        each.try_reserve_exact(polygons).ok()?;
        each.resize(polygons, (false, 0));
        Some(Self { rings, odd, polygons: each, inside: 0 })
    }

    fn clear(&mut self) {
        self.odd.fill(false);
        self.polygons.fill((false, 0));
        self.inside = 0;
    }

    fn inside(&self) -> bool {
        self.inside > 0
    }

    fn flip(&mut self, ring: usize) -> Option<()> {
        let Ring { polygon, shell } = *self.rings.get(ring)?;
        let odd = self.odd.get_mut(ring)?;
        *odd = !*odd;
        let now = *odd;
        let state = self.polygons.get_mut(polygon)?;
        let was = state.0 && state.1 == 0;
        if shell {
            state.0 = now;
        } else if now {
            state.1 = state.1.checked_add(1)?;
        } else {
            state.1 = state.1.checked_sub(1)?;
        }
        let is = state.0 && state.1 == 0;
        self.inside = match (was, is) {
            (false, true) => self.inside.checked_add(1)?,
            (true, false) => self.inside.checked_sub(1)?,
            (false, false) | (true, true) => self.inside,
        };
        Some(())
    }
}

/// Each ring's polygon, and whether it is a shell.
fn ring_list(polygons: &[&[Vec<Point>]]) -> Vec<Ring> {
    let mut rings = Vec::new();
    for (polygon, parts) in polygons.iter().enumerate() {
        for at in 0..parts.len() {
            rings.push(Ring { polygon, shell: at == 0 });
        }
    }
    rings
}

/// The rings of each polygon of a polygon or multipolygon.
fn polygons(shape: &Shape) -> Option<Vec<&[Vec<Point>]>> {
    match shape {
        Shape::Polygon(rings) => Some(vec![rings.as_slice()]),
        Shape::Multi(code, parts) => {
            if *code != MULTIPOLYGON {
                return None;
            }
            parts
                .iter()
                .map(|part| match part {
                    Shape::Polygon(rings) => Some(rings.as_slice()),
                    Shape::Point(_) | Shape::Line(_) | Shape::Multi(_, _) => None,
                })
                .collect()
        }
        Shape::Point(_) | Shape::Line(_) => None,
    }
}

#[cfg(test)]
mod tests;
