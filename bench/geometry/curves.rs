//! The curve, surface and format engines of ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593): linearizing and measuring curves, triangulating a TIN,
//! and the well-known text and GML of a compound curve.

use std::hint::black_box;

use super::harness::{Bench, Case};
use turborel::geometry::model::Dims;
use turborel::spatial::curves::geodesy::Planar;
use turborel::spatial::curves::linearize::Tolerance;
use turborel::spatial::curves::observe::{curve_to_line, length, point_at_distance};
use turborel::spatial::curves::piece::Metric;
use turborel::spatial::curves::tin::{Constraint, ElementKind, triangulate};
use turborel::spatial::curves::types::{Clothoid, Coord, Curve, Placement, Vector};
use turborel::spatial::formats::tree::{Body, Geometry};
use turborel::spatial::formats::{gml, wkt};

/// Repetitions of each curve operation.
const ROUNDS: u64 = 2_000;
/// The last point of the circular string, of 32 arcs.
const ARC_POINTS: i32 = 64;
/// Points the TIN is built from, on a side.
const SIDE: u32 = 30;

pub fn run(bench: &Bench) {
    let arcs: Vec<Coord> = (0_i32..=ARC_POINTS).map(|n| Coord::xy(f64::from(n), if n % 2 == 0 { 0.0 } else { 1.0 })).collect();
    let string = Curve::CircularString(arcs);
    let clothoid = Curve::Clothoid(Some(Clothoid {
        placement: Placement {
            // At the string's end, so the compound curve below is contiguous.
            location: Coord::xy(f64::from(ARC_POINTS), 0.0),
            directions: vec![Vector { x: 1.0, y: 0.0, z: 0.0 }, Vector { x: 0.0, y: 1.0, z: 0.0 }],
        },
        scale: 10.0,
        start: 0.0,
        end: 20.0,
        measures: None,
    }));
    bench.measure(Case::new("32-arc circular string, linearize", "curves"), || {
        for _ in 0..ROUNDS {
            black_box(curve_to_line(black_box(&string), &Tolerance::DEFAULT, &Planar).unwrap());
        }
        ROUNDS
    });
    bench.measure(Case::new("clothoid, length and point at distance", "curves"), || {
        for _ in 0..ROUNDS {
            black_box(length(black_box(&clothoid), Metric::Plane, &Planar).unwrap());
            black_box(point_at_distance(black_box(&clothoid), 7.5, Metric::Plane, &Planar).unwrap());
        }
        ROUNDS
    });
    let spots: Vec<Vec<Coord>> = (0..SIDE * SIDE)
        .map(|n| vec![Coord::xyz(f64::from(n % SIDE), f64::from(n / SIDE) + f64::from(n % 7) * 0.01, f64::from(n % 5))])
        .collect();
    let points = Constraint { kind: ElementKind::RandomPoints, parts: spots, has_z: true };
    let line =
        Constraint { kind: ElementKind::Breakline, parts: vec![vec![Coord::xyz(0.5, 0.5, 9.0), Coord::xyz(28.5, 27.5, 9.0)]], has_z: true };
    bench.measure(Case::new("900-point TIN with a breakline, triangulate", "TINs"), || {
        black_box(triangulate(black_box(&[points.clone(), line.clone()]), None).unwrap());
        1
    });
    let value = Geometry { dims: Dims::Xy, body: Body::Curve(Curve::Compound(vec![string.clone(), clothoid.clone()])) };
    let text = wkt::write(&value);
    let document = gml::write(&value, None);
    bench.measure(Case::new("compound curve, WKT write and read", "values").bytes(text.len()), || {
        for _ in 0..ROUNDS {
            black_box(wkt::read(&wkt::write(black_box(&value))).unwrap());
        }
        ROUNDS
    });
    bench.measure(Case::new("compound curve, GML write and read", "values").bytes(document.len()), || {
        for _ in 0..ROUNDS {
            black_box(gml::read(&gml::write(black_box(&value), None)).unwrap());
        }
        ROUNDS
    });
}
