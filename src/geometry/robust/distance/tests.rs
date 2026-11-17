//! Range and reconstruction requirements for approximate distance measures.

use super::*;

/// Verifies: [DESIGN] D85: exact binary scaling preserves every normal
/// and subnormal power of two, underflows only below the least subnormal,
/// and overflows beyond the greatest power. Splitting the scale keeps
/// finite reconstructed values even with exponents outside the normal range.
#[test]
fn binary_scales_preserve_normal_and_subnormal_results() {
    assert_eq!(power(-1074).to_bits(), 1);
    assert_eq!(power(0).to_bits(), 1.0_f64.to_bits());
    for exponent in -1074_i32..1023_i32 {
        assert_eq!(power(exponent + 1).to_bits(), (power(exponent) * 2.0_f64).to_bits(), "{exponent}");
    }
    assert_eq!(power(-1075).to_bits(), 0);
    assert_eq!(power(-2200).to_bits(), 0);
    assert!(power(1024).is_infinite());
    assert!(power(2200).is_infinite());
    let cases: [(f64, i32, u64); 5] = [
        (1.0_f64, -1074, 1),
        (0.5_f64, -1074, 0),
        (1.5_f64, -1074, 2),
        (f64::from_bits(0x3bf0_0000_0000_0000), 1087, 0x7fe0_0000_0000_0000),
        (f64::from_bits(0x43f0_0000_0000_0000), -1087, 0x0008_0000_0000_0000),
    ];
    for (value, exponent, expected) in cases {
        assert_eq!(scaled(value, exponent).to_bits(), expected, "{value:e}, {exponent}");
    }
}

/// Verifies: [DESIGN] D85: a norm is between its largest component and
/// its outward-rounded L1 bound, across every exponent and both signs.
/// Zero and infinite differences are defined; no squared intermediate
/// turns a finite norm into zero or infinity.
#[test]
fn norms_stay_within_axis_and_l1_bounds() {
    assert_eq!(norm(0.0_f64, -0.0_f64).to_bits(), 0);
    for exponent in 0_u64..2047 {
        let x = f64::from_bits((exponent << 52) | 1);
        let y = x * 0.75_f64;
        for (a, b) in [(x, y), (-x, y), (y, -x)] {
            let result = norm(a, b);
            assert!(result >= a.abs().max(b.abs()));
            assert!(result <= (a.abs() + b.abs()).next_up());
            assert!(result.is_finite(), "{a:e}, {b:e}");
        }
    }
    assert!(norm(f64::INFINITY, 0.0_f64).is_infinite());
    assert!(norm(f64::NEG_INFINITY, f64::INFINITY).is_infinite());
}
