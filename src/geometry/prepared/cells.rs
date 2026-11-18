//! \[DESIGN\] Exact cell certificates for repeated point location in a
//! polygon (RULES.md D85). A prepared polygon gets a grid of dyadic cells,
//! squares of side 2^k at multiples of 2^k: every cell bound is an exact
//! binary64 number, and a point's cell is found exactly by multiplying by
//! 2^-k. A cell a boundary edge may touch is a boundary cell. Marking is
//! conservative: each edge's columns in a row are widened by a bound on the
//! floating error of locating them. Every other cell holds no boundary
//! point, so it is wholly interior or wholly exterior, and the same for
//! each ring. The grid is swept a row at a time, from the edges that may
//! touch that row, and each row is labelled from the exact half-open
//! crossings of its centre line, every crossing placed in its column by
//! exact orientation. Each ring's parity is kept, and shells, holes and
//! parts combine as exact point location combines them, so a label never
//! differs from it, valid topology or not. A point in a labelled cell is
//! located by one lookup. A boundary cell only one ring's edges may touch
//! keeps that ring's side where the cell's right side meets the centre
//! line, and a point in it is located from the few edges near it
//! (`path.rs`); a cell several rings touch takes the exact edge tests.
//! Lucene precomputes a grid of cell relations for polygon queries
//! likewise (`GeoEncodingUtils.createComponentPredicate`), with
//! approximate rather than exact cells, and none for boundary cells.

use std::sync::Arc;

use super::tree::EdgeTree;
use crate::geometry::crossing::{Location, segments};
use crate::geometry::grid::Grid;
use crate::geometry::shape::{MULTIPOLYGON, Point, Shape};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;

mod path;

/// A cell no boundary point touches, outside every polygon.
const EXTERIOR: u8 = 0;
/// A cell no boundary point touches, inside a polygon.
const INTERIOR: u8 = 1;
/// A cell edges of several rings may touch.
const BOUNDARY: u8 = 2;
/// A cell only one ring's edges may touch, whose reference point lies
/// outside the polygon, or inside it.
const NEAR_OUTSIDE: u8 = 3;
const NEAR_INSIDE: u8 = 4;
/// A column no edge has touched yet in the row being marked, and one that
/// edges of several rings have.
const UNTOUCHED: usize = usize::MAX;
const SEVERAL: usize = usize::MAX.saturating_sub(1);
/// \[DESIGN\] Polygons with fewer edges are located as fast by a scan.
pub(super) const MIN_EDGES: usize = 32;
/// Cells per edge: fine enough that most cells hold no edge.
const CELLS_PER_EDGE: u32 = 64;
/// The sweep's room per edge: its record, its place among the edges of a
/// row, and a crossing of that row's centre line.
const SWEPT_BYTES: usize = size_of::<Edge>().saturating_add(size_of::<&Edge>()).saturating_add(size_of::<(usize, usize)>());

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

/// An edge of `ring`, and the first and last row it may touch.
struct Edge {
    a: Point,
    b: Point,
    ring: usize,
    first: usize,
    last: usize,
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
        let count = polygons.iter().copied().flatten().map(|ring| ring.len().saturating_sub(1)).sum::<usize>();
        if count < MIN_EDGES {
            return None;
        }
        let grid = Grid::new(bounds, count, CELLS_PER_EDGE)?;
        let cells = grid.rows.checked_mul(grid.cols)?;
        let scratch = count.checked_mul(SWEPT_BYTES)?.checked_add(grid.cols.checked_mul(size_of::<usize>())?)?;
        let mut held = budget.reserve(cells.checked_add(scratch)?, "a prepared polygon's cell certificates").ok()?;
        let mut edges = Vec::new();
        edges.try_reserve_exact(count).ok()?;
        for (ring, points) in polygons.iter().copied().flatten().enumerate() {
            for (a, b) in segments(points) {
                let rows = grid.rows_of((a, b))?;
                edges.push(Edge { a, b, ring, first: *rows.start(), last: *rows.end() });
            }
        }
        let mut states = Vec::new();
        states.try_reserve_exact(cells).ok()?;
        states.resize(cells, EXTERIOR);
        sweep(&grid, &ring_list(&polygons), &mut edges, &mut states)?;
        drop(edges);
        held.resize(states.capacity()).ok()?;
        Some(Box::new(Self { grid, states, _held: held }))
    }

    /// Where `point` lies in the polygon, if its cell decides it: by the
    /// label of a cell no edge touches, or, in a cell one ring's edges
    /// touch, by those of them near the point, found in `tree`.
    pub(super) fn decide<'a>(&self, point: Point, tree: impl FnOnce() -> Option<&'a EdgeTree>) -> Option<Location> {
        let (row, col) = self.grid.place(point)?;
        let at = row.checked_mul(self.grid.cols)?.checked_add(col)?;
        let inside = match *self.states.get(at)? {
            INTERIOR => return Some(Location::Interior),
            EXTERIOR => return Some(Location::Exterior),
            NEAR_INSIDE => true,
            NEAR_OUTSIDE => false,
            _ => return None,
        };
        path::locate(&self.grid, tree()?, point, (row, col), inside)
    }
}

/// Marks and labels the grid a row at a time, from the edges that may
/// touch that row, so only one row's crossings are ever held.
fn sweep(grid: &Grid, rings: &[Ring], edges: &mut [Edge], states: &mut [u8]) -> Option<()> {
    edges.sort_unstable_by_key(|edge| edge.first);
    let mut waiting: &[Edge] = edges;
    let mut active: Vec<&Edge> = Vec::new();
    active.try_reserve_exact(waiting.len()).ok()?;
    let mut row_of = Row { owners: Vec::new(), found: Vec::new() };
    row_of.found.try_reserve_exact(waiting.len()).ok()?;
    row_of.owners.try_reserve_exact(grid.cols).ok()?;
    row_of.owners.resize(grid.cols, UNTOUCHED);
    let polygons = rings.iter().map(|ring| ring.polygon.saturating_add(1)).max().unwrap_or(0);
    let mut parity = Parity::new(rings, polygons)?;
    for row in 0..grid.rows {
        let (entering, later) = waiting.split_at_checked(waiting.partition_point(|edge| edge.first <= row))?;
        waiting = later;
        active.extend(entering);
        active.retain(|edge| edge.last >= row);
        row_of.owners.fill(UNTOUCHED);
        row_of.found.clear();
        let base = row.checked_mul(grid.cols)?;
        let line = states.get_mut(base..base.checked_add(grid.cols)?)?;
        for edge in &active {
            row_of.mark(grid, edge, row, line)?;
        }
        row_of.found.sort_unstable();
        parity.label(line, &row_of.owners, &row_of.found)?;
    }
    Some(())
}

/// What marking a row notes: each column's owner, and the column and ring
/// of each crossing of the row's centre line.
struct Row {
    owners: Vec<usize>,
    found: Vec<(usize, usize)>,
}

impl Row {
    /// Marks the cells of row `row`, held in `line`, that an edge may
    /// touch, and notes its ring as each one's owner; if it crosses the
    /// row's centre line by the half-open rule exact point location uses,
    /// notes the column of that crossing too.
    fn mark(&mut self, grid: &Grid, edge: &Edge, row: usize, line: &mut [u8]) -> Option<()> {
        let (first, last) = grid.columns((edge.a, edge.b), row)?;
        for (state, owner) in line.get_mut(first..=last)?.iter_mut().zip(self.owners.get_mut(first..=last)?) {
            *state = BOUNDARY;
            *owner = if *owner == UNTOUCHED || *owner == edge.ring { edge.ring } else { SEVERAL };
        }
        let centre = grid.centre(row)?;
        if path::straddles(edge.a, edge.b, centre) {
            self.found.push((column(grid, edge, centre, first, last)?, edge.ring));
        }
        Some(())
    }
}

/// The column of the point where `edge` crosses the centre line at
/// `centre`, exactly: of the columns it may touch, `first` to `last`, the
/// last whose left side lies left of it. On a side, the crossing is placed
/// as it lies just above the line, by the edge's slope (`path.rs`).
fn column(grid: &Grid, edge: &Edge, centre: f64, first: usize, last: usize) -> Option<usize> {
    let (mut low, mut high) = (first, last);
    while low < high {
        let middle = high.checked_sub(high.checked_sub(low)?.checked_div(2)?)?;
        if path::beyond(edge.a, edge.b, [grid.column_start(middle)?, centre]) {
            low = middle;
        } else {
            high = middle.checked_sub(1)?;
        }
    }
    Some(low)
}

/// A cell decided near its points, whose reference point is inside or not.
fn near(inside: bool) -> u8 {
    if inside { NEAR_INSIDE } else { NEAR_OUTSIDE }
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

    /// Labels one row's cells, `line`, from right to left. A crossing is
    /// passed once the sweep is left of its column, flipping its ring's
    /// parity for every cell further left. An unmarked cell takes the
    /// polygons' state; a boundary cell is classified from the state at its
    /// reference point, where its right side meets the centre line.
    fn label(&mut self, line: &mut [u8], owners: &[usize], found: &[(usize, usize)]) -> Option<()> {
        self.clear();
        let mut pending = found.iter().rev().peekable();
        for (col, (state, &owner)) in line.iter_mut().zip(owners).enumerate().rev() {
            *state = if *state == BOUNDARY {
                self.classify(owner)?
            } else if self.inside() {
                INTERIOR
            } else {
                EXTERIOR
            };
            while let Some(&(_, ring)) = pending.next_if(|&&(at, _)| at >= col) {
                self.flip(ring)?;
            }
        }
        Some(())
    }

    /// The state of a boundary cell whose edges are all of ring `owner`,
    /// from the parities at its reference point. Rings without edges in the
    /// cell have the same parity at every point of it. So another polygon
    /// holding the reference point holds the cell, and a hole whose shell
    /// does not hold it leaves the cell outside. Otherwise the owner alone
    /// decides, unless another hole holds the reference point.
    fn classify(&self, owner: usize) -> Option<u8> {
        let Some(&Ring { polygon, shell }) = self.rings.get(owner) else { return Some(BOUNDARY) };
        let (shell_odd, odd_holes) = *self.polygons.get(polygon)?;
        let odd = *self.odd.get(owner)?;
        if self.inside > usize::from(shell_odd && odd_holes == 0) {
            return Some(INTERIOR);
        }
        let state = if shell {
            if odd_holes == 0 { near(odd) } else { BOUNDARY }
        } else if shell_odd {
            if odd_holes == usize::from(odd) { near(!odd) } else { BOUNDARY }
        } else {
            EXTERIOR
        };
        Some(state)
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
