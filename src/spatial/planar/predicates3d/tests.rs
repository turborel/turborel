//! The spatial predicates against exact integer evaluation.

use std::cmp::Ordering;

use super::super::predicates::tests::{determinant, integer, power};
use super::super::tests::Seeded;
use super::*;

fn random_points<const N: usize>(random: &mut Seeded, side: u64) -> ([Point3; N], [[i128; 3]; N]) {
    let mut floats = [[0.0_f64; 3]; N];
    let mut exact = [[0_i128; 3]; N];
    for (float, integer_point) in floats.iter_mut().zip(exact.iter_mut()) {
        for (coordinate, value) in float.iter_mut().zip(integer_point.iter_mut()) {
            (*coordinate, *value) = integer(random, side);
        }
    }
    (floats, exact)
}

/// det(a - d, b - d, c - d).
fn orient_exact(points: [[i128; 3]; 4]) -> Ordering {
    let [first, second, third, apex] = points;
    let rows: Vec<Vec<i128>> =
        [first, second, third].iter().map(|row| row.iter().zip(apex).map(|(value, corner)| value.wrapping_sub(corner)).collect()).collect();
    determinant(&rows).cmp(&0)
}

/// The determinant of the rows (p - e, |p - e|²), whose sign is
/// Shewchuk's insphere sign.
fn insphere_exact(points: [[i128; 3]; 5]) -> Ordering {
    let [first, second, third, fourth, apex] = points;
    let rows: Vec<Vec<i128>> = [first, second, third, fourth]
        .iter()
        .map(|row| {
            let mut out: Vec<i128> = row.iter().zip(apex).map(|(value, apex)| value.wrapping_sub(apex)).collect();
            let lift = out.iter().fold(0_i128, |sum, value| sum.wrapping_add(value.wrapping_mul(*value)));
            out.push(lift);
            out
        })
        .collect();
    determinant(&rows).cmp(&0)
}

/// Verifies: [DESIGN] orient3d and insphere give the exact sign on
/// random small integer points, many of them degenerate.
#[test]
fn random_integer_points() {
    let mut random = Seeded(5);
    for _ in 0_i32..10_000_i32 {
        let (tetrahedron, exact_tetrahedron) = random_points::<4>(&mut random, 2);
        let [a, b, c, d] = tetrahedron;
        assert_eq!(orient3d(a, b, c, d), orient_exact(exact_tetrahedron), "{tetrahedron:?}");
        let (five, exact_five) = random_points::<5>(&mut random, 2);
        assert_eq!(insphere(five[0], five[1], five[2], five[3], five[4]), insphere_exact(exact_five), "{five:?}");
    }
}

/// Verifies: [DESIGN] signs are kept at every scale, subnormal and huge.
#[test]
fn every_exponent() {
    let mut random = Seeded(13);
    let configurations: Vec<_> =
        (0_i32..30_i32).map(|_| (random_points::<4>(&mut random, 1 << 8), random_points::<5>(&mut random, 1 << 6))).collect();
    for exponent in (-1_060_i32..1_000_i32).step_by(10) {
        let factor = power(exponent);
        for ((orient_points, orient_points_exact), (sphere_points, sphere_points_exact)) in &configurations {
            let [a, b, c, d] = orient_points.map(|corner| corner.map(|value| value * factor));
            assert_eq!(orient3d(a, b, c, d), orient_exact(*orient_points_exact), "{exponent}");
            let sphere = sphere_points.map(|corner| corner.map(|value| value * factor));
            assert_eq!(insphere(sphere[0], sphere[1], sphere[2], sphere[3], sphere[4]), insphere_exact(*sphere_points_exact), "{exponent}");
        }
    }
}

/// Verifies: [DESIGN] the documented conventions: a point below a
/// counterclockwise triangle is greater, and a point inside the sphere
/// of a positively oriented tetrahedron is greater.
#[test]
fn conventions() {
    let (a, b, c) = ([0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64]);
    assert_eq!(orient3d(a, b, c, [0.0_f64, 0.0_f64, -1.0_f64]), Ordering::Greater);
    assert_eq!(orient3d(a, b, c, [0.0_f64, 0.0_f64, 1.0_f64]), Ordering::Less);
    assert_eq!(orient3d(a, b, c, [5.0_f64, 5.0_f64, 0.0_f64]), Ordering::Equal);
    let d = [0.0_f64, 0.0_f64, -1.0_f64];
    assert_eq!(insphere(a, b, c, d, [0.25_f64, 0.25_f64, -0.25_f64]), Ordering::Greater);
    assert_eq!(insphere(a, b, c, d, [1.0_f64, 1.0_f64, -1.0_f64]), Ordering::Equal);
    assert_eq!(insphere(a, b, c, d, [9.0_f64, 9.0_f64, 9.0_f64]), Ordering::Less);
}
