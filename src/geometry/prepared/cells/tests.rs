//! Cell certificates checked against exact point location: every point a
//! cell decides, by its label or the edges near the point, must be located
//! exactly as location locates it, and the certificates must decide most
//! points, even where teeth are finer than the cells.

use super::*;
use crate::geometry::crossing::{self, Location};
use crate::geometry::tests::common::{dense_ring, subdivided_ring};

/// A comb: a base with `teeth` narrow teeth, so horizontal rays cross many
/// edges, the hard case for interval trees.
fn comb(teeth: u32) -> Shape {
    let mut ring = vec![[0.0_f64, 0.0_f64]];
    for t in 0..teeth {
        let x = f64::from(t) * 4.0_f64;
        ring.extend([[x + 1.0_f64, 1.0_f64], [x + 1.0_f64, 9.0_f64], [x + 3.0_f64, 9.0_f64], [x + 3.0_f64, 1.0_f64]]);
    }
    let end = f64::from(teeth) * 4.0_f64;
    ring.extend([[end, 1.0_f64], [end, 0.0_f64], [0.0_f64, 0.0_f64]]);
    Shape::Polygon(vec![ring])
}

/// The ring of a comb over [0, 10] x [0, 10] whose `teeth` are narrower
/// than a cell, so nearly every cell of its teeth is a boundary cell.
fn fine_comb(teeth: u32) -> Vec<Point> {
    let period = 10.0_f64 / f64::from(teeth);
    let mut ring = vec![[0.0_f64, 0.0_f64]];
    for t in 0..teeth {
        let x = f64::from(t) * period;
        ring.extend([[x, 1.0_f64], [x, 9.7_f64], [x + period * 0.5_f64, 9.7_f64], [x + period * 0.5_f64, 1.0_f64]]);
    }
    ring.extend([[10.0_f64, 1.0_f64], [10.0_f64, 0.0_f64], [0.0_f64, 0.0_f64]]);
    ring
}

/// A star of `points` spikes about (0, 0), with irrational-looking radii.
fn star(points: u32) -> Shape {
    let mut ring = Vec::new();
    for k in 0..points.saturating_mul(2) {
        let angle = f64::from(k) * std::f64::consts::PI / f64::from(points);
        let radius = if k % 2 == 0 { 10.0_f64 } else { 3.7_f64 };
        ring.push([radius * angle.cos(), radius * angle.sin()]);
    }
    ring.push(ring[0]);
    Shape::Polygon(vec![ring])
}

fn bounds(shape: &Shape) -> Bounds {
    shape.bounds().unwrap().unwrap()
}

fn name(location: Location) -> &'static str {
    match location {
        Location::Exterior => "exterior",
        Location::Boundary => "boundary",
        Location::Interior => "interior",
    }
}

/// Every decision on a grid of `steps` points a side, on the shape's
/// vertices and edge midpoints, and on `extra` points, agrees with exact
/// location; returns the share decided.
fn agree_at(shape: &Shape, steps: u32, extra: &[Point]) -> f64 {
    let budget = MemoryBudget::new(usize::MAX);
    let cells = Cells::new(shape, bounds(shape), &budget).expect("certificates for the shape");
    let tree = EdgeTree::new(shape, edges(shape).unwrap(), &budget).unwrap();
    let [x0, y0] = bounds(shape).lower();
    let [x1, y1] = bounds(shape).upper();
    let (mut decided, mut total) = (0_u32, 0_u32);
    let mut check = |point: Point| {
        let decision = cells.decide(point, || Some(&*tree));
        if let Some(location) = decision {
            let exact = crossing::locate(point, shape);
            assert!(location == exact, "{point:?} is {}, not {}", name(exact), name(location));
        }
        total = total.saturating_add(1);
        decided = decided.saturating_add(u32::from(decision.is_some()));
    };
    for i in 0..=steps {
        for j in 0..=steps {
            let (u, v) = (f64::from(i) / f64::from(steps), f64::from(j) / f64::from(steps));
            check([x0 + (x1 - x0) * u, y0 + (y1 - y0) * v]);
        }
    }
    for ring in polygons(shape).unwrap().into_iter().flatten() {
        for (a, b) in segments(ring) {
            check(a);
            check([f64::midpoint(a[0], b[0]), f64::midpoint(a[1], b[1])]);
        }
    }
    extra.iter().copied().for_each(&mut check);
    f64::from(decided) / f64::from(total)
}

fn agree(shape: &Shape) -> f64 {
    agree_at(shape, 200, &[])
}

/// Verifies: [DESIGN] D85: every point a cell certificate decides, by
/// its label or by the edges near it, it locates as exact location does,
/// for shells with holes in either direction, multipolygons, combs and
/// stars, its vertices and edge midpoints included; nearly all points are
/// decided.
#[test]
fn certificates_agree_with_exact_location() {
    let mut hole = dense_ring(2.0_f64, 8.0_f64);
    hole.reverse();
    let holed = Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), hole]);
    let multi = Shape::Multi(MULTIPOLYGON, vec![holed.clone(), Shape::Polygon(vec![dense_ring(12.0_f64, 14.0_f64)])]);
    for (shape, least) in [(holed, 0.95_f64), (multi, 0.95_f64), (comb(64), 0.95_f64), (star(40), 0.95_f64)] {
        let share = agree(&shape);
        assert!(share >= least, "decided only {share} of {shape:?}");
    }
}

/// Verifies: [DESIGN] D85: teeth narrower than a cell leave nearly every
/// cell of a comb a boundary cell; one ring's edges near a point still
/// decide it, as exact location does, in a comb, a comb with a hole, and
/// one part of a multipolygon of combs.
#[test]
fn edges_near_a_point_decide_cells_finer_teeth_cross() {
    let comb = Shape::Polygon(vec![fine_comb(200)]);
    let mut hole = subdivided_ring(&[[2.0_f64, 0.2_f64], [8.0_f64, 0.2_f64], [8.0_f64, 0.8_f64], [2.0_f64, 0.2_f64]]);
    hole.reverse();
    let holed = Shape::Polygon(vec![fine_comb(200), hole]);
    let far = Shape::Polygon(vec![dense_ring(20.0_f64, 21.0_f64)]);
    let parts = Shape::Multi(MULTIPOLYGON, vec![comb.clone(), far]);
    for shape in [comb, holed, parts] {
        let share = agree_at(&shape, 120, &[]);
        assert!(share >= 0.99_f64, "decided only {share}");
    }
}

/// Verifies: [DESIGN] D85: vertices on cells' right sides and centre
/// lines, edges through the corners of a point's path and along a cell's
/// side are placed as they lie just off the path: every point is decided,
/// and as exact location decides it.
#[test]
fn ties_on_a_points_path_are_placed_by_slope() {
    let shell = dense_ring(0.0_f64, 64.0_f64);
    // The shell's 64 edges and the hole's 40 fix the grid before the hole
    // is drawn on it.
    let grid = Grid::new(bounds(&Shape::Polygon(vec![shell.clone()])), 104, CELLS_PER_EDGE).unwrap();
    let side = |col: usize| grid.column_start(col).unwrap();
    let mut hole = Vec::new();
    // Pairs on one side make edges along it, between centre lines.
    for (i, col) in (10..70_usize).step_by(6).flat_map(|col| [col, col]).enumerate() {
        hole.push([side(col), grid.centre(if i.is_multiple_of(2) { 20 } else { 27 }).unwrap()]);
    }
    for (i, col) in (11..71_usize).step_by(3).enumerate().rev() {
        hole.push([side(col), grid.row_start(if i.is_multiple_of(2) { 60 } else { 65 }).unwrap()]);
    }
    hole.push(hole[0]);
    let shape = Shape::Polygon(vec![shell, hole.clone()]);
    assert_eq!(edges(&shape), Some(104));
    let step = side(1) - side(0);
    let mut extra = Vec::new();
    for [x, y] in hole {
        for dx in [-step / 3.0_f64, -step / 1024.0_f64, 0.0_f64, step / 3.0_f64] {
            for dy in [-step / 4.0_f64, 0.0_f64, step / 4.0_f64, step / 2.0_f64] {
                extra.push([x + dx, y + dy]);
            }
        }
    }
    let decided = agree_at(&shape, 100, &extra);
    assert!(decided >= 1.0_f64, "decided only {decided}");
}

/// A polygon whose right side lies one unit in the last place left of a
/// cell side at a negative coordinate, where subtracting the grid's shift
/// rounds, with a zigzag left side; or the same with its axes swapped, so
/// the side is its top.
fn below_a_side(side: f64, swapped: bool) -> Shape {
    let mut ring = vec![[side, -32.0_f64], [side, 32.0_f64]];
    for k in 0..=32_u32 {
        let x = if k.is_multiple_of(2) { -63.0_f64 } else { -62.0_f64 };
        ring.push([x, 32.0_f64 - 2.0_f64 * f64::from(k)]);
    }
    ring.push([side, -32.0_f64]);
    if swapped {
        ring = ring.into_iter().map(|[x, y]| [y, x]).collect();
    }
    Shape::Polygon(vec![ring])
}

/// Verifies: [DESIGN] D85: a point's cell is found exactly where
/// subtracting the grid's shift from a negative coordinate rounds: points
/// on, just left of and just right of a side one unit in the last place
/// below a cell's side, and the same across a top, are located as exact
/// location locates them.
#[test]
fn cells_are_found_exactly_below_negative_sides() {
    // -(1 - 2^-8 + 2^-53): its difference with 2^-8 is a tie that rounds
    // to -1.
    let side = -(0.996_093_75_f64 + f64::EPSILON / 2.0_f64);
    for swapped in [false, true] {
        let shape = below_a_side(side, swapped);
        let mut extra = Vec::new();
        for y in [-31.5_f64, -7.25_f64, 0.5_f64, 13.0_f64, 31.5_f64] {
            for x in [side.next_down(), side, side.next_up()] {
                extra.push(if swapped { [y, x] } else { [x, y] });
            }
        }
        agree_at(&shape, 60, &extra);
    }
}

/// Verifies: [DESIGN] D85: preparation never changes a result, even for
/// topology the predicates require to be valid: a hole outside its shell
/// and overlapping parts are labelled as exact location reads them.
#[test]
fn invalid_topology_is_labelled_as_location_reads_it() {
    let outside_hole = Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), dense_ring(20.0_f64, 30.0_f64)]);
    let overlapping = Shape::Multi(
        MULTIPOLYGON,
        vec![Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64)]), Shape::Polygon(vec![dense_ring(5.0_f64, 15.0_f64)])],
    );
    let bowtie = Shape::Polygon(vec![subdivided_ring(&[
        [0.0_f64, 0.0_f64],
        [10.0_f64, 10.0_f64],
        [10.0_f64, 0.0_f64],
        [0.0_f64, 10.0_f64],
        [0.0_f64, 0.0_f64],
    ])]);
    for shape in [outside_hole, overlapping, bowtie] {
        agree(&shape);
    }
}

/// Verifies: [DESIGN] D38, D85: certificates are refused, leaving exact
/// location, for too few edges, a refused reservation, and coordinates
/// whose units in the last place approach a cell; they still agree with
/// location near the largest finite coordinates they accept.
#[test]
fn certificates_decline_what_they_cannot_make_exactly() {
    let budget = MemoryBudget::new(usize::MAX);
    let small = Shape::Polygon(vec![vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64]]]);
    assert!(Cells::new(&small, bounds(&small), &budget).is_none(), "too few edges");
    let shape = comb(64);
    assert!(Cells::new(&shape, bounds(&shape), &MemoryBudget::new(16)).is_none(), "refused room");
    let far = Shape::Polygon(vec![dense_ring(1.0e15_f64, 1.0e15_f64 + 8.0_f64)]);
    assert!(Cells::new(&far, bounds(&far), &budget).is_none(), "cells near a coordinate's last place");
    let huge = Shape::Polygon(vec![dense_ring(-1.0e300_f64, 1.0e300_f64)]);
    agree(&huge);
    let tiny = Shape::Polygon(vec![dense_ring(-1.0e-300_f64, 1.0e-300_f64)]);
    assert!(Cells::new(&tiny, bounds(&tiny), &budget).is_none(), "cells too small to be exact");
}

/// Verifies: [DESIGN] D38: certificates reserve their cells from the
/// budget and give them back when dropped.
#[test]
fn certificates_hold_their_room_until_dropped() {
    let budget = MemoryBudget::new(usize::MAX);
    let shape = comb(64);
    let cells = Cells::new(&shape, bounds(&shape), &budget).unwrap();
    assert!(budget.reserved() >= cells.states.len());
    drop(cells);
    assert_eq!(budget.reserved(), 0);
}
