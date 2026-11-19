use super::*;

fn points(kind: ElementKind, parts: &[&[(f64, f64, f64)]], has_z: bool) -> Constraint {
    Constraint { kind, parts: parts.iter().map(|p| p.iter().map(|&(x, y, z)| Coord::xyz(x, y, z)).collect()).collect(), has_z }
}

fn area(triangles: &[[Coord; 3]]) -> f64 {
    triangles.iter().map(|[a, b, c]| 0.5_f64 * ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x))).sum()
}

fn grid() -> Constraint {
    let mut spots = Vec::new();
    for i in 0_i32..=4_i32 {
        for j in 0_i32..=4_i32 {
            spots.push((f64::from(i), f64::from(j), f64::from(i) + f64::from(j)));
        }
    }
    points(ElementKind::RandomPoints, &[&spots], true)
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D4c, §17.1]
/// Random points triangulate their hull, counterclockwise; a planar
/// surface keeps its z; too few points raise 2FF71.
#[test]
fn points_triangulate_their_hull() {
    let tin = triangulate(&[grid()], None).unwrap();
    assert!((area(&tin) - 16.0).abs() < 1e-9_f64);
    assert_eq!(tin.len(), 32);
    for t in &tin {
        let [a, b, c] = t;
        assert!((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) > 0.0_f64);
        assert!(t.iter().all(|p| (p.z - p.x - p.y).abs() < 1e-12_f64));
    }
    let two = points(ElementKind::RandomPoints, &[&[(0.0_f64, 0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64, 0.0_f64)]], true);
    assert_eq!(triangulate(&[two], None).unwrap_err().sqlstate().as_str(), "2FF71");
    let line =
        points(ElementKind::GroupSpot, &[&[(0.0_f64, 0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64, 0.0_f64), (2.0_f64, 2.0_f64, 0.0_f64)]], true);
    assert_eq!(triangulate(&[line], None).unwrap_err().sqlstate().as_str(), "2FF71");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D5, §17.1]
/// A breakline's edges appear in the triangulation; a hole cuts out its
/// area and takes z from the surface; a 2D boundary clips to its area.
#[test]
fn breaklines_holes_and_boundaries() {
    let breakline = points(ElementKind::Breakline, &[&[(0.5_f64, 0.2_f64, 9.0_f64), (3.5_f64, 3.7_f64, 9.0_f64)]], true);
    let tin = triangulate(&[grid(), breakline], None).unwrap();
    let has = |x: f64, y: f64| {
        tin.iter().flatten().any(|p| (p.x - x).abs() < 1e-12_f64 && (p.y - y).abs() < 1e-12_f64 && (p.z - 9.0).abs() < 1e-12_f64)
    };
    assert!(has(0.5_f64, 0.2_f64) && has(3.5_f64, 3.7_f64));
    let hole = points(
        ElementKind::Hole,
        &[&[
            (1.0_f64, 1.0_f64, 0.0_f64),
            (3.0_f64, 1.0_f64, 0.0_f64),
            (3.0_f64, 3.0_f64, 0.0_f64),
            (1.0_f64, 3.0_f64, 0.0_f64),
            (1.0_f64, 1.0_f64, 0.0_f64),
        ]],
        false,
    );
    let holed = triangulate(&[grid(), hole], None).unwrap();
    assert!((area(&holed) - 12.0).abs() < 1e-9_f64);
    assert!(holed.iter().flatten().all(|p| (p.z - p.x - p.y).abs() < 1e-9_f64));
    let boundary = points(
        ElementKind::Boundary,
        &[&[(0.0_f64, 0.0_f64, 0.0_f64), (4.0_f64, 0.0_f64, 0.0_f64), (0.0_f64, 4.0_f64, 0.0_f64), (0.0_f64, 0.0_f64, 0.0_f64)]],
        false,
    );
    let clipped = triangulate(&[grid(), boundary], None).unwrap();
    assert!((area(&clipped) - 8.0).abs() < 1e-9_f64, "{}", area(&clipped));
    let open = points(
        ElementKind::Void,
        &[&[(1.0_f64, 1.0_f64, 0.0_f64), (2.0_f64, 1.0_f64, 0.0_f64), (2.0_f64, 2.0_f64, 0.0_f64), (1.0_f64, 2.0_f64, 0.0_f64)]],
        true,
    );
    assert_eq!(triangulate(&[grid(), open], None).unwrap_err().sqlstate().as_str(), "2FF74");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.1 D5, D6,
/// §8.6.4] The maximum side length peels long outside triangles and
/// must be positive (2FF02); a stop line removes the triangles it
/// crosses.
#[test]
fn max_side_and_stop_lines() {
    let sparse = points(
        ElementKind::RandomPoints,
        &[&[(0.0_f64, 0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64, 0.0_f64), (5.0_f64, 1.0_f64, 0.0_f64), (5.0_f64, 10.0_f64, 0.0_f64)]],
        true,
    );
    let all = triangulate(std::slice::from_ref(&sparse), None).unwrap();
    let peeled = triangulate(std::slice::from_ref(&sparse), Some(6.0_f64)).unwrap();
    assert!(peeled.len() < all.len());
    assert_eq!(triangulate(std::slice::from_ref(&sparse), Some(0.0_f64)).unwrap_err().sqlstate().as_str(), "2FF02");
    let stop = points(ElementKind::StopLine, &[&[(1.5_f64, 1.5_f64, 0.0_f64), (1.6_f64, 1.6_f64, 0.0_f64)]], true);
    let stopped = triangulate(&[grid(), stop], None).unwrap();
    assert_eq!(stopped.len(), 31);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1 D4,
/// §5.1.67] Element type names and keywords map both ways.
#[test]
fn element_names() {
    for kind in KINDS {
        assert_eq!(ElementKind::from_name(kind.name()), Some(kind));
        assert_eq!(ElementKind::from_keyword(&kind.keyword().to_lowercase()), Some(kind));
    }
    assert_eq!(ElementKind::from_name(" Soft Break "), Some(ElementKind::SoftBreak));
    assert_eq!(ElementKind::from_name("cliff"), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.3 D5, §17.1] A
/// hole takes z from the surface; a vertex of it outside the surface
/// takes the z of the surface's nearest vertex, here (4 3) of z 7 for
/// (6 3) and (3 4) of z 7 for (3 6).
#[test]
fn a_hole_beyond_the_surface_takes_the_nearest_z() {
    let hole = points(
        ElementKind::Hole,
        &[&[
            (3.0_f64, 3.0_f64, 0.0_f64),
            (6.0_f64, 3.0_f64, 0.0_f64),
            (6.0_f64, 6.0_f64, 0.0_f64),
            (3.0_f64, 6.0_f64, 0.0_f64),
            (3.0_f64, 3.0_f64, 0.0_f64),
        ]],
        false,
    );
    let tin = triangulate(&[grid(), hole], None).unwrap();
    let at = |x: f64, y: f64| tin.iter().flatten().filter(move |p| p.x.to_bits() == x.to_bits() && p.y.to_bits() == y.to_bits());
    let outside: Vec<&Coord> = at(6.0_f64, 3.0_f64).chain(at(3.0_f64, 6.0_f64)).collect();
    let Some(_) = outside.first() else { panic!("no vertex beyond the surface") };
    assert!(outside.iter().all(|p| (p.z - 7.0_f64).abs() < 1e-12_f64), "{outside:?}");
    assert!(at(3.0_f64, 3.0_f64).all(|p| (p.z - 6.0_f64).abs() < 1e-12_f64));
}
