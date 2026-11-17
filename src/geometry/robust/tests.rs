//! Orientation fixtures independent of floating determinant evaluation.

use super::*;

/// Verifies: [DESIGN] D82: signs match an integer oracle in all octants,
/// including degeneracy, negative coordinates and both turn directions.
#[test]
fn orientations_agree_with_integer_determinants() {
    let mut seed = 1_u32;
    for _ in 0_u32..10_000 {
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            i32::try_from(seed >> 16_u32).unwrap() - 32_768_i32
        };
        let [ax, ay, bx, by, cx, cy] = [next(), next(), next(), next(), next(), next()];
        let expected = (i64::from(ax - cx) * i64::from(by - cy) - i64::from(ay - cy) * i64::from(bx - cx)).cmp(&0);
        let a = [f64::from(ax), f64::from(ay)];
        let b = [f64::from(bx), f64::from(by)];
        let c = [f64::from(cx), f64::from(cy)];
        assert_eq!(orient(a, b, c), expected);
        assert_eq!(exact(a, b, c), expected);
    }
}

/// Verifies: [DESIGN] D82: the sign of one-ULP triangles survives every
/// binary64 exponent, including products that underflow or overflow.
/// Reversing the points reverses the sign; repeating one makes it zero.
#[test]
fn one_ulp_triangles_have_exact_sign_at_every_exponent() {
    for exponent in 0_u64..2047 {
        let x = f64::from_bits(exponent << 52);
        let next = x.next_up();
        let a = [x, x];
        let b = [next, x];
        let c = [x, next];
        assert_eq!(orient(a, b, c), Ordering::Greater, "exponent {exponent}");
        assert_eq!(orient(c, b, a), Ordering::Less, "exponent {exponent}");
        assert_eq!(orient(a, a, c), Ordering::Equal, "exponent {exponent}");
    }
}

/// Verifies: [DESIGN] D82: exact cancellation and differences that
/// overflow binary64 cannot change collinearity or swallow a tiny offset.
#[test]
fn cancellation_and_extreme_offsets_are_not_tolerances() {
    let n = 4_503_599_627_370_496.0_f64;
    assert_eq!(orient([0.0_f64, 0.0_f64], [n, n - 1.0_f64], [n + 1.0_f64, n]), Ordering::Greater);
    let a = [-f64::MAX, -f64::MAX];
    let b = [f64::MAX, f64::MAX];
    assert_eq!(orient(a, b, [0.0_f64, 0.0_f64]), Ordering::Equal);
    assert_eq!(orient(a, b, [0.0_f64, f64::from_bits(1)]), Ordering::Greater);
    assert_eq!(orient(a, b, [f64::from_bits(1), 0.0_f64]), Ordering::Less);
    assert_eq!(orient([0.0_f64, 0.0_f64], [f64::MIN_POSITIVE, 0.0_f64], [0.0_f64, f64::from_bits(1)]), Ordering::Greater);
}

/// Verifies: [DESIGN] D82: the fast orientation proof accepts ordinary
/// turns of either sign but declines cancellation, zero products, overflow
/// and subnormal products or error bounds. Declined cases retain the exact
/// sign; a rounded product of zero is never a proof of collinearity.
#[test]
fn orientation_filter_proves_only_normal_separated_determinants() {
    let a = [1.0_f64, 1.0_f64];
    let b = [3.0_f64, 2.0_f64];
    let c = [2.0_f64, 4.0_f64];
    assert_eq!(filtered(a, b, c), Some(Ordering::Greater));
    assert_eq!(filtered(b, a, c), Some(Ordering::Less));
    let n = 4_503_599_627_370_496.0_f64;
    for (first, second, third) in [
        ([0.0_f64, 0.0_f64], [n, n - 1.0_f64], [n + 1.0_f64, n]),
        ([0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64]),
        (a, a, c),
        ([-f64::MAX, -f64::MAX], [f64::MAX, f64::MAX], [0.0_f64, f64::from_bits(1)]),
        (a.map(|v| v * 1e-160_f64), b.map(|v| v * 1e-160_f64), c.map(|v| v * 1e-160_f64)),
        (a.map(|v| v * 1e-150_f64), b.map(|v| v * 1e-150_f64), c.map(|v| v * 1e-150_f64)),
    ] {
        assert_eq!(filtered(first, second, third), None);
        assert_eq!(orient(first, second, third), exact(first, second, third));
    }
}

/// Verifies: [DESIGN] D82: the filtered and complete orientation agree
/// with dyadic integer arithmetic for reproducible binary64 bit patterns
/// across the exponent range, including unequal scales and signed zeros.
#[test]
fn filtered_orientations_match_exact_binary64_determinants() {
    let mut seed = 7_u64;
    for _ in 0_u32..10_000 {
        let mut point = || {
            std::array::from_fn(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let bits = if seed & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000 { seed & !0x0010_0000_0000_0000 } else { seed };
                f64::from_bits(bits)
            })
        };
        let (a, b, c) = (point(), point(), point());
        let expected = exact(a, b, c);
        if let Some(sign) = filtered(a, b, c) {
            assert_eq!(sign, expected, "{a:?} {b:?} {c:?}");
        }
        assert_eq!(orient(a, b, c), expected);
    }
}
