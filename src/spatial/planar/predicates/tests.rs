//! The planar predicates against exact integer evaluation.

use std::cmp::Ordering;

use super::super::tests::Seeded;
use super::*;

/// The determinant of a square integer matrix by cofactor expansion,
/// wrapping arithmetic being exact for the small entries used.
pub(in super::super) fn determinant(rows: &[Vec<i128>]) -> i128 {
    let Some((first, rest)) = rows.split_first() else { return 1 };
    let mut sum = 0_i128;
    for (column, entry) in first.iter().enumerate() {
        let minor: Vec<Vec<i128>> = rest
            .iter()
            .map(|row| row.iter().enumerate().filter(|(index, _)| *index != column).map(|(_, value)| *value).collect())
            .collect();
        let term = entry.wrapping_mul(determinant(&minor));
        sum = if column % 2 == 0 { sum.wrapping_add(term) } else { sum.wrapping_sub(term) };
    }
    sum
}

/// A small signed integer, as a coordinate and exactly.
pub(in super::super) fn integer(random: &mut Seeded, side: u64) -> (f64, i128) {
    let value = i32::try_from(random.below(side.wrapping_mul(2).wrapping_add(1))).unwrap().wrapping_sub(i32::try_from(side).unwrap());
    (f64::from(value), i128::from(value))
}

fn orient_exact(points: [[i128; 2]; 3]) -> Ordering {
    let [first, second, [apex_x, apex_y]] = points;
    let rows: Vec<Vec<i128>> = [first, second].iter().map(|[x, y]| vec![x.wrapping_sub(apex_x), y.wrapping_sub(apex_y)]).collect();
    determinant(&rows).cmp(&0)
}

fn incircle_exact(points: [[i128; 2]; 4]) -> Ordering {
    let [first, second, third, [apex_x, apex_y]] = points;
    let rows: Vec<Vec<i128>> = [first, second, third]
        .iter()
        .map(|[x, y]| {
            let (across, up) = (x.wrapping_sub(apex_x), y.wrapping_sub(apex_y));
            vec![across, up, across.wrapping_mul(across).wrapping_add(up.wrapping_mul(up))]
        })
        .collect();
    determinant(&rows).cmp(&0)
}

fn random_points<const N: usize>(random: &mut Seeded, side: u64) -> ([Point; N], [[i128; 2]; N]) {
    let mut floats = [[0.0_f64; 2]; N];
    let mut exact = [[0_i128; 2]; N];
    for (float, integer_point) in floats.iter_mut().zip(exact.iter_mut()) {
        for (coordinate, value) in float.iter_mut().zip(integer_point.iter_mut()) {
            (*coordinate, *value) = integer(random, side);
        }
    }
    (floats, exact)
}

/// Verifies: [DESIGN] orient2d and incircle give the exact sign on
/// random small integer points, a large share of them degenerate.
#[test]
fn random_integer_points() {
    let mut random = Seeded(7);
    for _ in 0_i32..20_000_i32 {
        let (triangle, exact_triangle) = random_points::<3>(&mut random, 3);
        let [a, b, c] = triangle;
        assert_eq!(orient2d(a, b, c), orient_exact(exact_triangle), "{triangle:?}");
        let (quad, exact_quad) = random_points::<4>(&mut random, 3);
        assert_eq!(incircle(quad[0], quad[1], quad[2], quad[3]), incircle_exact(exact_quad), "{quad:?}");
    }
}

/// Verifies: [DESIGN] the sign is exact one unit in the last place away
/// from collinearity, where the floating filter cannot decide. A point
/// near 0.5 on the diagonal steps by 2^-53 over the 256 by 256 grid of
/// Kettner et al. (2008), against 12 and 24.
#[test]
fn near_collinear_grid() {
    let unit = 0.5_f64.next_up() - 0.5_f64;
    let base = 1_i128 << 52_i32;
    let scale = 1_i128 << 53_i32;
    for i in 0_i32..256_i32 {
        for j in 0_i32..256_i32 {
            let a = [0.5_f64 + f64::from(i) * unit, 0.5_f64 + f64::from(j) * unit];
            let exact_a = [base.wrapping_add(i128::from(i)), base.wrapping_add(i128::from(j))];
            let expected = orient_exact([exact_a, [scale.wrapping_mul(12); 2], [scale.wrapping_mul(24); 2]]);
            assert_eq!(orient2d(a, [12.0_f64, 12.0_f64], [24.0_f64, 24.0_f64]), expected, "{i} {j}");
            assert_eq!(orient2d([12.0_f64, 12.0_f64], [24.0_f64, 24.0_f64], a), expected, "{i} {j}");
        }
    }
}

/// `2^exponent`, by exact halving or doubling.
pub(in super::super) fn power(exponent: i32) -> f64 {
    let step = if exponent < 0_i32 { 0.5_f64 } else { 2.0_f64 };
    (0..exponent.unsigned_abs()).fold(1.0, |value, _| value * step)
}

/// Verifies: [DESIGN] signs are kept at every scale of binary64,
/// subnormal and huge, where the filter's analysis does not apply.
#[test]
fn every_exponent() {
    let mut random = Seeded(11);
    let configurations: Vec<_> =
        (0_i32..40_i32).map(|_| (random_points::<3>(&mut random, 1 << 10), random_points::<4>(&mut random, 1 << 8))).collect();
    for exponent in (-1_060_i32..1_000_i32).step_by(10) {
        let factor = power(exponent);
        for ((orient_points, orient_exact_points), (circle_points, circle_exact_points)) in &configurations {
            let [a, b, c] = orient_points.map(|[x, y]| [x * factor, y * factor]);
            assert_eq!(orient2d(a, b, c), orient_exact(*orient_exact_points), "{exponent}");
            let circle = circle_points.map(|[x, y]| [x * factor, y * factor]);
            assert_eq!(incircle(circle[0], circle[1], circle[2], circle[3]), incircle_exact(*circle_exact_points), "{exponent}");
        }
    }
}

/// Verifies: [DESIGN] the documented orientation conventions: a
/// counterclockwise turn is greater, and a point inside the circle of a
/// counterclockwise triangle is greater.
#[test]
fn conventions() {
    assert_eq!(orient2d([0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64]), Ordering::Greater);
    assert_eq!(orient2d([0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64], [1.0_f64, 0.0_f64]), Ordering::Less);
    assert_eq!(orient2d([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64], [2.0_f64, 2.0_f64]), Ordering::Equal);
    let (a, b, c) = ([1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64], [-1.0_f64, 0.0_f64]);
    assert_eq!(incircle(a, b, c, [0.0_f64, 0.0_f64]), Ordering::Greater);
    assert_eq!(incircle(a, b, c, [0.0_f64, -1.0_f64]), Ordering::Equal);
    assert_eq!(incircle(a, b, c, [2.0_f64, 0.0_f64]), Ordering::Less);
    assert_eq!(incircle(c, b, a, [0.0_f64, 0.0_f64]), Ordering::Less);
    assert!(tame(&[0.0_f64, 1.0_f64, -1e60_f64]));
    assert!(!tame(&[1e-61_f64]));
    assert_eq!(proven(1.0, 0.5), Some(Ordering::Greater));
    assert_eq!(proven(-1.0, 0.5), Some(Ordering::Less));
    assert_eq!(proven(0.25, 0.5), None);
}
