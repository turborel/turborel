//! Cell certificates checked against exact point location: a labelled
//! cell must give exactly what location gives, a boundary point must never
//! be labelled, and the certificates must decide most points.

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

/// Every decision on a grid of points, and on the shape's own vertices and
/// edge midpoints, agrees with exact location; returns the share decided.
fn agree(shape: &Shape) -> f64 {
    let cells = Cells::new(shape, bounds(shape), &MemoryBudget::new(usize::MAX)).expect("certificates for the shape");
    let [x0, y0] = bounds(shape).lower();
    let [x1, y1] = bounds(shape).upper();
    let (mut decided, mut total) = (0_u32, 0_u32);
    let mut check = |point: Point| {
        let decision = cells.decide(point);
        match crossing::locate(point, shape) {
            Location::Interior => assert!(decision != Some(false), "{point:?} is inside"),
            Location::Exterior => assert!(decision != Some(true), "{point:?} is outside"),
            Location::Boundary => assert_eq!(decision, None, "{point:?} is on the boundary"),
        }
        total = total.saturating_add(1);
        decided = decided.saturating_add(u32::from(decision.is_some()));
    };
    for i in 0..=200_u32 {
        for j in 0..=200_u32 {
            let (u, v) = (f64::from(i) / 200.0_f64, f64::from(j) / 200.0_f64);
            check([x0 + (x1 - x0) * u, y0 + (y1 - y0) * v]);
        }
    }
    for ring in polygons(shape).unwrap().into_iter().flatten() {
        for (a, b) in segments(ring) {
            check(a);
            check([f64::midpoint(a[0], b[0]), f64::midpoint(a[1], b[1])]);
        }
    }
    f64::from(decided) / f64::from(total)
}

/// Verifies: [DESIGN] D85: every point a cell certificate decides, it
/// decides as exact location does, for shells with holes in either
/// direction, multipolygons, combs and stars; a point on the boundary,
/// its vertices and edge midpoints included, is never decided; most
/// points are.
#[test]
fn certificates_agree_with_exact_location() {
    let mut hole = dense_ring(2.0_f64, 8.0_f64);
    hole.reverse();
    let holed = Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), hole]);
    let multi = Shape::Multi(MULTIPOLYGON, vec![holed.clone(), Shape::Polygon(vec![dense_ring(12.0_f64, 14.0_f64)])]);
    for (shape, least) in [(holed, 0.8_f64), (multi, 0.8_f64), (comb(64), 0.6_f64), (star(40), 0.5_f64)] {
        let share = agree(&shape);
        assert!(share >= least, "decided only {share} of {shape:?}");
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
