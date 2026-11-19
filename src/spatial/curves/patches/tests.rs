use super::*;

fn ring(points: &[[f64; 3]]) -> Vec<Coord> {
    points.iter().map(|p| Coord::at(*p, 0.0)).collect()
}

/// The unit cube's six faces, outward.
fn cube() -> Vec<Patch> {
    let faces = [
        [[0.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64]],
        [[0.0_f64, 0.0_f64, 1.0_f64], [1.0_f64, 0.0_f64, 1.0_f64], [1.0_f64, 1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 1.0_f64]],
        [[0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64, 1.0_f64]],
        [[0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 1.0_f64], [1.0_f64, 1.0_f64, 1.0_f64], [1.0_f64, 1.0_f64, 0.0_f64]],
        [[0.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 0.0_f64]],
        [[1.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64, 1.0_f64], [1.0_f64, 0.0_f64, 1.0_f64]],
    ];
    faces.iter().map(|f| vec![ring(&[f[0], f[1], f[2], f[3], f[0]])]).collect()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.3, §8.1.5,
/// §8.1.7, §8.5] A closed unit cube has 3D area 6, no boundary, its
/// centroid at its center; its plane area counts the projections of the
/// top and bottom.
#[test]
fn cube_measures() {
    let cube = cube();
    assert!((area(&cube, Metric::Space).unwrap() - 6.0).abs() < 1e-15_f64);
    assert!((area(&cube, Metric::Plane).unwrap() - 2.0).abs() < 1e-15_f64);
    assert_eq!(perimeter(&cube, Metric::Space), Some(0.0_f64));
    let c = centroid(&cube, Metric::Space).unwrap();
    assert!((c.x - 0.5).abs() < 1e-15_f64 && (c.y - 0.5).abs() < 1e-15_f64 && (c.z - 0.5).abs() < 1e-15_f64);
    assert!(is_contiguous(&cube));
    let p = point_on_surface(&cube, Metric::Space).unwrap().unwrap();
    assert!(p.x >= 0.0_f64 && p.x <= 1.0_f64 && p.y >= 0.0_f64 && p.y <= 1.0_f64 && p.z >= -1e-12_f64 && p.z <= 1.0_f64 + 1e-12_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.4, §8.6] Two
/// triangles of a square TIN: area 1, perimeter 4 with the shared
/// diagonal not counted; a separate patch makes the surface not
/// contiguous.
#[test]
fn tin_boundary_and_contiguity() {
    let tin = vec![
        vec![ring(&[[0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64, 1.0_f64], [0.0_f64, 0.0_f64, 0.0_f64]])],
        vec![ring(&[[0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 0.0_f64]])],
    ];
    assert!((area(&tin, Metric::Plane).unwrap() - 1.0).abs() < 1e-15_f64);
    assert!((perimeter(&tin, Metric::Plane).unwrap() - 4.0).abs() < 1e-15_f64);
    assert!(is_contiguous(&tin));
    let mut apart = tin.clone();
    apart.push(vec![ring(&[
        [5.0_f64, 5.0_f64, 0.0_f64],
        [6.0_f64, 5.0_f64, 0.0_f64],
        [5.0_f64, 6.0_f64, 0.0_f64],
        [5.0_f64, 5.0_f64, 0.0_f64],
    ])]);
    assert!(!is_contiguous(&apart));
    let c = centroid(&tin, Metric::Plane).unwrap();
    assert!((c.x - 0.5).abs() < 1e-15_f64 && (c.y - 0.5).abs() < 1e-15_f64);
    assert_eq!(area(&[], Metric::Plane), None);
    assert_eq!(perimeter(&[], Metric::Plane), None);
    assert_eq!(centroid(&[], Metric::Plane), None);
    assert_eq!(point_on_surface(&[], Metric::Plane).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.4.1] Triangle
/// rings are one closed ring of four points: holes raise 2FF66, other
/// counts 2FF75, an open ring 2FF74. A holed patch subtracts its hole;
/// a flat patch has its boundary's centroid.
#[test]
fn triangles_and_holes() {
    let t = ring(&[[0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 0.0_f64]]);
    assert!(check_triangle(std::slice::from_ref(&t)).is_ok());
    assert_eq!(check_triangle(&[t.clone(), t.clone()]).unwrap_err().sqlstate().as_str(), "2FF66");
    assert_eq!(check_triangle(&[t[..3].to_vec()]).unwrap_err().sqlstate().as_str(), "2FF75");
    let open = ring(&[[0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 2.0_f64, 0.0_f64]]);
    assert_eq!(check_triangle(&[open]).unwrap_err().sqlstate().as_str(), "2FF74");
    let square = ring(&[
        [0.0_f64, 0.0_f64, 0.0_f64],
        [4.0_f64, 0.0_f64, 0.0_f64],
        [4.0_f64, 4.0_f64, 0.0_f64],
        [0.0_f64, 4.0_f64, 0.0_f64],
        [0.0_f64, 0.0_f64, 0.0_f64],
    ]);
    let hole = ring(&[
        [1.0_f64, 1.0_f64, 0.0_f64],
        [2.0_f64, 1.0_f64, 0.0_f64],
        [2.0_f64, 2.0_f64, 0.0_f64],
        [1.0_f64, 2.0_f64, 0.0_f64],
        [1.0_f64, 1.0_f64, 0.0_f64],
    ]);
    let (a, _) = patch_moments(&vec![square, hole], Metric::Plane);
    assert!((a - 15.0).abs() < 1e-15_f64);
    let flat = vec![vec![ring(&[
        [0.0_f64, 0.0_f64, 0.0_f64],
        [2.0_f64, 0.0_f64, 0.0_f64],
        [1.0_f64, 0.0_f64, 0.0_f64],
        [0.0_f64, 0.0_f64, 0.0_f64],
    ])]];
    assert_eq!(centroid(&flat, Metric::Plane).map(|c| c.x), Some(1.0_f64));
}
