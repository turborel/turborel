//! The planar engine's kernels on their own, without SQL values:
//! predicates, relate, the overlay operations, buffer, hull, validity
//! and distance, on overlapping regular polygons.

use std::hint::black_box;

use turborel::spatial::planar::{self, Geometry, Point};

use super::harness::{Bench, Case};

/// Edges of the larger operands.
const EDGES: u32 = 512;
/// Calls per measured batch for the cheaper kernels.
const ROUNDS: u64 = 8;
/// Points for the predicate and hull runs.
const POINTS: u32 = 100_000;

fn ring(edges: u32, centre: Point, radius: f64) -> Vec<Point> {
    let [cx, cy] = centre;
    let mut ring: Vec<Point> = (0..edges)
        .map(|n| {
            let angle = std::f64::consts::TAU * f64::from(n) / f64::from(edges);
            [cx + radius * angle.cos(), cy + radius * angle.sin()]
        })
        .collect();
    ring.push(ring[0]);
    ring
}

fn scattered(count: u32) -> Vec<Point> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..count)
        .map(|_| {
            let mut next = || {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                f64::from((state >> 40_i32) as u32) / f64::from(1_u32 << 24_i32)
            };
            [next(), next()]
        })
        .collect()
}

pub fn run(bench: &Bench) {
    let points = scattered(POINTS);
    bench.measure(Case::new("planar orient2d", "tests"), || {
        for triple in points.windows(3) {
            black_box(planar::orient2d(triple[0], triple[1], triple[2]));
        }
        u64::from(POINTS - 2)
    });
    bench.measure(Case::new("planar orient2d, collinear", "tests"), || {
        for n in 0..POINTS {
            let x = f64::from(n) * 0.1_f64;
            black_box(planar::orient2d([0.1_f64, 0.1_f64], [x, x], black_box([0.3_f64, 0.3_f64])));
        }
        u64::from(POINTS)
    });
    bench.measure(Case::new("planar incircle", "tests"), || {
        for quad in points.windows(4) {
            black_box(planar::incircle(quad[0], quad[1], quad[2], quad[3]));
        }
        u64::from(POINTS - 3)
    });
    let cloud = Geometry::MultiPoint(points.clone());
    bench.measure(Case::new("planar convex hull, 100k points", "hulls"), || {
        black_box(planar::convex_hull(&cloud));
        1
    });
    for edges in [64, EDGES] {
        pairs(bench, edges);
    }
}

fn pairs(bench: &Bench, edges: u32) {
    let first = Geometry::Polygon(vec![ring(edges, [0.0_f64, 0.0_f64], 1.0)]);
    let second = Geometry::Polygon(vec![ring(edges, [0.7_f64, 0.3_f64], 1.0)]);
    let name = format!("{edges}-edge polygons");
    bench.measure(Case::new(format!("planar relate, {name}"), "pairs"), || {
        for _ in 0..ROUNDS {
            black_box(planar::relate(&first, &second).unwrap());
        }
        ROUNDS
    });
    for (label, operation) in [
        ("intersection", planar::Operation::Intersection),
        ("union", planar::Operation::Union),
        ("difference", planar::Operation::Difference),
        ("symdifference", planar::Operation::SymDifference),
    ] {
        bench.measure(Case::new(format!("planar {label}, {name}"), "pairs"), || {
            for _ in 0..ROUNDS {
                black_box(planar::overlay(&first, &second, operation).unwrap());
            }
            ROUNDS
        });
    }
    bench.measure(Case::new(format!("planar buffer, {edges}-edge polygon"), "buffers"), || {
        black_box(planar::buffer(&first, 0.1).unwrap());
        1
    });
    bench.measure(Case::new(format!("planar is valid, {edges}-edge polygon"), "checks"), || {
        for _ in 0..ROUNDS {
            black_box(planar::is_valid(&first).unwrap());
        }
        ROUNDS
    });
    let far = Geometry::Polygon(vec![ring(edges, [3.0_f64, 0.5_f64], 1.0)]);
    bench.measure(Case::new(format!("planar distance, {name}"), "pairs"), || {
        for _ in 0..ROUNDS {
            black_box(planar::distance(&first, &far));
        }
        ROUNDS
    });
}
