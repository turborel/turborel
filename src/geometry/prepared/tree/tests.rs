//! The edge hierarchy returns exactly the edges whose envelopes meet a
//! window, holds its room until dropped, and declines a refused one.

use super::*;
use crate::geometry::distance::separated;
use crate::geometry::prepared::cells;
use crate::geometry::shape::MULTIPOLYGON;
use crate::geometry::tests::common::dense_ring;

/// A comb: a base with `teeth` teeth whose centres all share one height,
/// the case alternate-axis splits leave in wide leaves.
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

/// A star of `points` spikes about (0, 0), whose long edges' envelopes
/// overlap near its centre.
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

fn tree(shape: &Shape, budget: &Arc<MemoryBudget>) -> EdgeTree {
    *EdgeTree::new(shape, cells::edges(shape).unwrap(), budget).unwrap()
}

/// Edges as bits, sorted, so two lists compare as multisets.
fn sorted(edges: Vec<(Point, Point)>) -> Vec<[u64; 4]> {
    let mut bits: Vec<[u64; 4]> = edges.into_iter().map(|([ax, ay], [bx, by])| [ax, ay, bx, by].map(f64::to_bits)).collect();
    bits.sort_unstable();
    bits
}

/// Every edge whose envelope meets `window`, by testing each.
fn expected(shape: &Shape, window: Window) -> Vec<[u64; 4]> {
    sorted(rings(shape).unwrap().into_iter().flat_map(segments).filter(|&(a, b)| meets(envelope(a, b), window)).collect())
}

fn found(tree: &EdgeTree, window: Window) -> Vec<[u64; 4]> {
    let mut edges = Vec::new();
    tree.within(window, |a, b, _| {
        edges.push((a, b));
        true
    })
    .unwrap();
    sorted(edges)
}

/// Verifies: [DESIGN] D85: the hierarchy returns each edge whose
/// envelope meets a closed window once, and no other, for combs, stars,
/// holes and parts, over windows from whole envelopes down to single
/// points on vertices; a visitor that stops ends the walk.
#[test]
fn windows_find_exactly_the_edges_whose_envelopes_meet_them() {
    let budget = MemoryBudget::new(usize::MAX);
    let holed = Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), dense_ring(2.0_f64, 8.0_f64)]);
    let multi = Shape::Multi(MULTIPOLYGON, vec![holed.clone(), Shape::Polygon(vec![dense_ring(12.0_f64, 14.0_f64)])]);
    for shape in [comb(300), star(97), holed, multi] {
        let tree = tree(&shape, &budget);
        let bounds = shape.bounds().unwrap().unwrap();
        let ([x0, y0], [x1, y1]) = (bounds.lower(), bounds.upper());
        for size in [0.0_f64, 0.37_f64, 1.0_f64, 5.0_f64, 1000.0_f64] {
            for i in 0..=20_u32 {
                for j in 0..=20_u32 {
                    let (x, y) = (x0 + (x1 - x0) * f64::from(i) / 20.0_f64, y0 + (y1 - y0) * f64::from(j) / 20.0_f64);
                    let window = [x, y, x + size, y + size];
                    assert_eq!(found(&tree, window), expected(&shape, window), "{window:?}");
                }
            }
        }
        for [x, y] in rings(&shape).unwrap().into_iter().flatten().step_by(7) {
            assert_eq!(found(&tree, [*x, *y, *x, *y]), expected(&shape, [*x, *y, *x, *y]));
        }
        let mut seen = 0_u32;
        tree.within([x0, y0, x1, y1], |_, _, _| {
            seen = seen.saturating_add(1);
            seen < 3
        })
        .unwrap();
        assert_eq!(seen, 3);
    }
}

/// Verifies: [DESIGN] D38: the hierarchy reserves its edges and nodes,
/// gives them back when dropped, and is declined when the budget refuses.
#[test]
fn hierarchies_hold_their_room_until_dropped() {
    let budget = MemoryBudget::new(usize::MAX);
    let shape = comb(64);
    let built = tree(&shape, &budget);
    assert!(budget.reserved() >= cells::edges(&shape).unwrap() * size_of::<(Point, Point)>());
    drop(built);
    assert_eq!(budget.reserved(), 0);
    assert!(EdgeTree::new(&shape, cells::edges(&shape).unwrap(), &MemoryBudget::new(64)).is_none());
    assert_eq!(budget.reserved(), 0);
}

/// The scan's minimum measure from `point` to every edge of `shape`.
fn scanned(point: Point, shape: &Shape) -> f64 {
    separated(&Shape::Point(Some(point)), shape)
}

/// Verifies: [DESIGN] D85: the nearest-edge search returns, to the bit,
/// the minimum a scan of every edge takes, for points inside, on, near and
/// far from combs, stars, holes and parts, at integer, fractional, tiny and
/// huge scales.
#[test]
fn nearest_edges_measure_as_the_scan_does_to_the_bit() {
    let budget = MemoryBudget::new(usize::MAX);
    let holed = Shape::Polygon(vec![dense_ring(0.0_f64, 10.0_f64), dense_ring(2.0_f64, 8.0_f64)]);
    let multi = Shape::Multi(MULTIPOLYGON, vec![holed.clone(), Shape::Polygon(vec![dense_ring(12.0_f64, 14.0_f64)])]);
    let scaled = |shape: &Shape, by: f64| {
        let rings: Vec<Vec<Point>> =
            rings(shape).unwrap().into_iter().map(|ring| ring.iter().map(|&[x, y]| [x * by, y * by]).collect()).collect();
        Shape::Polygon(rings)
    };
    for shape in
        [comb(300), star(97), holed.clone(), multi, scaled(&star(31), 1.0e-9_f64), scaled(&holed, 1.0e12_f64), scaled(&comb(40), 0.37_f64)]
    {
        let tree = tree(&shape, &budget);
        let bounds = shape.bounds().unwrap().unwrap();
        let ([x0, y0], [x1, y1]) = (bounds.lower(), bounds.upper());
        let (w, h) = (x1 - x0, y1 - y0);
        let mut points: Vec<Point> = rings(&shape).unwrap().into_iter().flatten().copied().collect();
        for i in 0..=30_u32 {
            for j in 0..=30_u32 {
                let (u, v) = (f64::from(i) / 10.0_f64 - 1.0_f64, f64::from(j) / 10.0_f64 - 1.0_f64);
                points.push([x0 + w * u, y0 + h * v]);
            }
        }
        points.extend([[x0 - w * 1.0e6_f64, y1 + h * 3.0e5_f64], [x1 + w * 0.01_f64, y0 - h * 0.02_f64]]);
        for point in points {
            assert_eq!(tree.nearest(point).unwrap().to_bits(), scanned(point, &shape).to_bits(), "{point:?}");
        }
    }
}
