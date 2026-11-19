//! Unit tests of a geometry column's envelope statistics: the shares the
//! grid estimates, the coordinates counted and the clustering measured
//! (RULES.md R-TEST-1, R-TEST-2).

use super::*;

/// The square of side `side` at `(x, y)`, its corners and closure five
/// coordinates.
fn square(x: f64, y: f64, side: f64) -> Geometry {
    let (r, t) = (x + side, y + side);
    Geometry::from_wkt(&format!("POLYGON (({x} {y}, {r} {y}, {r} {t}, {x} {t}, {x} {y}))"), 0).unwrap()
}

fn window(low: [f64; 2], high: [f64; 2]) -> Bounds {
    Bounds::new(low, high).unwrap()
}

/// Points on a 100 × 100 lattice, row by row (`ordered`) or scattered.
fn lattice(ordered: bool) -> EnvelopeBuilder {
    let mut builder = EnvelopeBuilder::keeping(10_000);
    for n in 0..10_000_u32 {
        let at = if ordered { n } else { n.wrapping_mul(7_919) % 10_000 };
        builder.add(&Geometry::point(f64::from(at % 100), f64::from(at.div_euclid(100)), 0).unwrap());
    }
    builder
}

/// Verifies: [DESIGN] RULES.md D137: of points spread evenly, a window
/// is estimated to meet the share of the frame it covers, near the share
/// it does; one beside the frame meets none and one holding it all.
#[test]
fn a_window_meets_the_share_of_points_it_covers() {
    let stats = lattice(true).finish().unwrap();
    let quarter = stats.meeting(window([0.0_f64, 0.0_f64], [49.5_f64, 49.5_f64]));
    assert!((quarter - 0.25_f64).abs() < 0.03_f64, "{quarter}");
    let strip = stats.meeting(window([0.0_f64, 0.0_f64], [99.0_f64, 9.5_f64]));
    assert!((strip - 0.1_f64).abs() < 0.02_f64, "{strip}");
    assert!(stats.meeting(window([200.0_f64, 200.0_f64], [300.0_f64, 300.0_f64])) < 1.0e-9_f64);
    assert!((stats.meeting(window([-1.0_f64, -1.0_f64], [100.0_f64, 100.0_f64])) - 1.0_f64).abs() < 1.0e-9_f64);
}

/// Verifies: [DESIGN] RULES.md D137: an envelope's extent widens the
/// window it is estimated to meet by half its mean width and height, so
/// squares beside a window but reaching into it count; an empty value
/// meets none; and the coordinates are counted, closures included.
#[test]
fn extents_widen_the_window_and_empties_meet_none() {
    let mut builder = EnvelopeBuilder::keeping(1_000);
    for n in 0..100_u32 {
        builder.add(&square(f64::from(n % 10) * 10.0_f64, f64::from(n.div_euclid(10)) * 10.0_f64, 8.0_f64));
    }
    let squares = builder.clone().finish().unwrap();
    assert!((squares.vertices() - 5.0_f64).abs() < 1.0e-9_f64);
    // A thin window along x = 4 meets a column of ten squares, 0 to 8.
    let column = squares.meeting(window([4.0_f64, 0.0_f64], [4.0_f64, 98.0_f64]));
    assert!((column - 0.1_f64).abs() < 0.05_f64, "{column}");
    for _ in 0..100_u32 {
        builder.add(&Geometry::from_wkt("POINT EMPTY", 0).unwrap());
    }
    let half = builder.finish().unwrap();
    assert!((half.meeting(window([-1.0_f64, -1.0_f64], [100.0_f64, 100.0_f64])) - 0.5_f64).abs() < 1.0e-9_f64);
    assert!((half.vertices() - 2.5_f64).abs() < 1.0e-9_f64);
}

/// Verifies: [DESIGN] RULES.md D137: rows laid out in space measure near
/// 1 of clustering, the same rows in no spatial order near 0; a column of
/// one point, every distance 0, measures 0, as does one of no step.
#[test]
fn clustering_tells_a_table_laid_out_in_space() {
    let ordered = lattice(true).finish().unwrap().clustering();
    let scattered = lattice(false).finish().unwrap().clustering();
    assert!(ordered > 0.9_f64, "{ordered}");
    assert!(scattered < 0.2_f64, "{scattered}");
    let mut same = EnvelopeBuilder::keeping(10);
    let mut one = EnvelopeBuilder::keeping(10);
    one.add(&Geometry::point(1.0_f64, 1.0_f64, 0).unwrap());
    for _ in 0..10_u32 {
        same.add(&Geometry::point(1.0_f64, 1.0_f64, 0).unwrap());
    }
    assert!(same.finish().unwrap().clustering().abs() < 1.0e-12_f64);
    assert!(one.finish().unwrap().clustering().abs() < 1.0e-12_f64);
}

/// Verifies: [DESIGN] RULES.md D137: the sample keeps at most its bound,
/// the estimate still near the share; no geometry gives no statistics;
/// only empties give a frame of none, meeting nothing; a frame too wide
/// for its extent to be finite keeps no grid, meeting nothing.
#[test]
fn bounded_samples_and_degenerate_frames() {
    let mut builder = EnvelopeBuilder::keeping(500);
    for n in 0..10_000_u32 {
        builder.add(&Geometry::point(f64::from(n % 100), f64::from(n.div_euclid(100)), 0).unwrap());
    }
    assert_eq!(builder.sample.len(), 500);
    let half = builder.finish().unwrap().meeting(window([0.0_f64, 0.0_f64], [49.5_f64, 99.0_f64]));
    assert!((half - 0.5_f64).abs() < 0.08_f64, "{half}");
    assert!(EnvelopeBuilder::keeping(10).finish().is_none());
    let mut empty = EnvelopeBuilder::keeping(10);
    empty.add(&Geometry::from_wkt("LINESTRING EMPTY", 0).unwrap());
    assert!(empty.finish().unwrap().meeting(window([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64])).abs() < 1.0e-12_f64);
    let mut wide = EnvelopeBuilder::keeping(10);
    wide.add(&Geometry::point(-f64::MAX, 0.0_f64, 0).unwrap());
    wide.add(&Geometry::point(f64::MAX, 0.0_f64, 0).unwrap());
    assert!(wide.finish().unwrap().meeting(window([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64])).abs() < 1.0e-12_f64);
}

/// Verifies: [DESIGN] RULES.md D137: a cell's share along an axis is the
/// part of it a window covers, and of a cell of no width 1 only if the
/// window holds its one coordinate. MC/DC: `from <= low` and `low <= to`
/// each alone false.
#[test]
fn spans_cover_cells_in_part() {
    assert!((along((0.0_f64, 10.0_f64), (5.0_f64, 20.0_f64)) - 0.5_f64).abs() < 1.0e-12_f64);
    assert!(along((0.0_f64, 10.0_f64), (20.0_f64, 30.0_f64)).abs() < 1.0e-12_f64);
    assert!((along((3.0_f64, 3.0_f64), (0.0_f64, 5.0_f64)) - 1.0_f64).abs() < 1.0e-12_f64);
    assert!(along((3.0_f64, 3.0_f64), (4.0_f64, 5.0_f64)).abs() < 1.0e-12_f64);
    assert!(along((3.0_f64, 3.0_f64), (0.0_f64, 2.0_f64)).abs() < 1.0e-12_f64);
}
