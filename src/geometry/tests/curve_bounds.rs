//! The boxes that hold geodesic strings, NURBS curves and the collections
//! and solids of the model, as the spatial index keys them.

use crate::spatial::formats::wkt::read_geom;

fn bounds(text: &str) -> ([f64; 2], [f64; 2]) {
    let b = read_geom(text, 4326).unwrap().bounds().unwrap().unwrap();
    (b.lower(), b.upper())
}

/// Verifies: [DESIGN] RULES.md D85 a geodesic string's box holds the
/// great circle between its points: past the antimeridian it spans every
/// longitude; north or south of the equator it reaches the arc's vertex;
/// of two points the same, whose great circle is none, it reaches the
/// pole on their side.
#[test]
fn geodesic_boxes_hold_their_arcs() {
    let (west, east) = bounds("GEODESICSTRING (170 10, -170 10)");
    assert!(west[0] <= -180.0_f64 && east[0] >= 180.0_f64, "{west:?} {east:?}");
    let (south, north) = bounds("GEODESICSTRING (0 -40, 90 -40)");
    assert!(south[1] < -40.0_f64 && north[1] >= -40.0_f64, "{south:?} {north:?}");
    assert_eq!(bounds("GEODESICSTRING (10 20, 10 20)"), ([10.0_f64, 20.0_f64], [10.0_f64, 90.0_f64]));
}

/// Verifies: [DESIGN] RULES.md D85 a rational NURBS curve's box holds its
/// control points and their weighted places; a multisurface's and a
/// solid's hold each of their patches.
#[test]
fn nurbs_collection_and_solid_boxes() {
    let nurbs = "NURBSCURVE (DEGREE 2, CONTROLPOINTS (NURBSPOINT (WEIGHTEDPOINT (0 0), WEIGHT 1), NURBSPOINT (WEIGHTEDPOINT (2 2), WEIGHT 0.5), \
                 NURBSPOINT (WEIGHTEDPOINT (2 0), WEIGHT 1)), KNOTS (KNOT (VALUE 0, MULTIPLICITY 3), KNOT (VALUE 1, MULTIPLICITY 3)))";
    assert_eq!(bounds(nurbs), ([0.0_f64, 0.0_f64], [4.0_f64, 4.0_f64]));
    let surfaces = "MULTISURFACE (((0 0, 1 0, 0 1, 0 0)), ((5 5, 6 5, 5 7, 5 5)))";
    assert_eq!(bounds(surfaces), ([0.0_f64, 0.0_f64], [6.0_f64, 7.0_f64]));
    let solid = "BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES (POLYGON Z ((0 0 0, 0 1 0, 1 0 0, 0 0 0)), POLYGON Z ((0 0 0, 1 0 0, 0 0 1, 0 0 0)), \
                 POLYGON Z ((0 0 0, 0 0 1, 0 1 0, 0 0 0)), POLYGON Z ((1 0 0, 0 1 0, 0 0 1, 1 0 0)))))";
    assert_eq!(bounds(solid), ([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64]));
}
