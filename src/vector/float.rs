//! \[EXTENSION\] Conversions between the binary floating-point widths a
//! VECTOR stores (IEEE 754 binary16, binary32, binary64) and integers, by
//! integer bit manipulation. Rust has no `f64` to `f32` conversion but
//! `as`, which RULES.md R-NUM-2 forbids, and no stable `f16`, so both
//! narrowings are done here, rounding to nearest, ties to even, as IEEE
//! 754 §4.3.1 asks of a conversion.

// ---------------------------------------------------------------------------
// IEEE 754 binary64 layout
// ---------------------------------------------------------------------------

/// Fraction bits of a binary64.
const F64_FRACTION: u32 = 52;
/// Exponent bias of a binary64.
const F64_BIAS: i32 = 1023;
/// The biased exponent of a binary64 infinity or NaN.
const F64_SPECIAL: i32 = 0x7ff;
/// The mask of a binary64's fraction.
const F64_FRACTION_MASK: u64 = (1 << F64_FRACTION) - 1;
/// A binary64's sign bit position.
const F64_SIGN: u32 = 63;

// ---------------------------------------------------------------------------
// IEEE 754 binary32 and binary16 layouts
// ---------------------------------------------------------------------------

/// The layout of a narrower binary format.
#[derive(Clone, Copy)]
struct Narrow {
    /// Its exponent bits.
    exponent: u32,
    /// Its fraction bits.
    fraction: u32,
}

/// IEEE 754 binary32.
const BINARY32: Narrow = Narrow { exponent: 8, fraction: 23 };
/// IEEE 754 binary16.
const BINARY16: Narrow = Narrow { exponent: 5, fraction: 10 };
/// The quiet bit of a narrowed NaN, as a shift below the fraction's top.
const QUIET_SHIFT: u32 = 1;
/// The bits a binary16 has below binary32's fraction.
const HALF_TO_SINGLE_SHIFT: u32 = 13;
/// binary16's exponent bias.
const HALF_BIAS: u32 = 15;
/// binary32's exponent bias.
const SINGLE_BIAS: u32 = 127;
/// binary16's exponent field of infinity and NaN.
const HALF_SPECIAL: u32 = 31;
/// binary16's fraction mask.
const HALF_FRACTION_MASK: u16 = 0x3ff;
/// binary16's sign bit.
const HALF_SIGN: u16 = 0x8000;
/// The value of a binary16 subnormal's least fraction bit, 2^-24.
const HALF_SUBNORMAL_UNIT: f32 = 5.960_464_5e-8;
/// The largest power of two whose shift keeps any bit of a significand.
const WIDEST_SHIFT: u32 = 63;

impl Narrow {
    fn bias(self) -> i32 {
        i32::try_from((1_u32 << (self.exponent.saturating_sub(1))).saturating_sub(1)).unwrap_or(i32::MAX)
    }

    fn special(self) -> u64 {
        (1_u64 << self.exponent).saturating_sub(1)
    }

    fn sign_shift(self) -> u32 {
        self.exponent.saturating_add(self.fraction)
    }

    /// `x` rounded to this format, as its bits.
    fn of(self, x: f64) -> u64 {
        let bits = x.to_bits();
        let sign = (bits >> F64_SIGN) << self.sign_shift();
        let biased = i32::try_from((bits >> F64_FRACTION) & 0x7ff).unwrap_or(F64_SPECIAL);
        let fraction = bits & F64_FRACTION_MASK;
        if biased == F64_SPECIAL {
            let nan = if fraction == 0 { 0 } else { 1_u64 << self.fraction.saturating_sub(QUIET_SHIFT) };
            return sign | (self.special() << self.fraction) | nan;
        }
        if biased == 0_i32 {
            // A binary64 subnormal is below half of any narrower subnormal.
            return sign;
        }
        let significand = fraction | (1 << F64_FRACTION);
        let target = biased.saturating_sub(F64_BIAS).saturating_add(self.bias());
        let drop = F64_FRACTION.saturating_sub(self.fraction);
        if target >= 1_i32 {
            return sign | self.normal(significand, drop, target);
        }
        let below = u32::try_from(1_i32.saturating_sub(target)).unwrap_or(u32::MAX);
        sign | round_shift(significand, drop.saturating_add(below))
    }

    fn normal(self, significand: u64, drop: u32, target: i32) -> u64 {
        let mut rounded = round_shift(significand, drop);
        let mut exponent = u64::try_from(target).unwrap_or(0);
        if rounded >> self.fraction.saturating_add(1) != 0 {
            rounded >>= 1_u32;
            exponent = exponent.saturating_add(1);
        }
        if exponent >= self.special() {
            return self.special() << self.fraction;
        }
        (exponent << self.fraction) | (rounded & low_mask(self.fraction))
    }
}

/// The low `bits` bits set.
fn low_mask(bits: u32) -> u64 {
    1_u64.checked_shl(bits).map_or(u64::MAX, |p| p.wrapping_sub(1))
}

/// `v` shifted right by `by`, rounded to nearest, ties to even.
fn round_shift(v: u64, by: u32) -> u64 {
    if by == 0 {
        return v;
    }
    if by > WIDEST_SHIFT {
        return 0;
    }
    let kept = v >> by;
    let rest = v & low_mask(by);
    let half = 1 << by.saturating_sub(1);
    let up = rest > half || (rest == half && kept & 1 == 1);
    kept.saturating_add(u64::from(up))
}

/// `x` rounded to the nearest `f32`, ties to even; infinite past
/// `f32::MAX`'s rounding range.
pub fn single(x: f64) -> f32 {
    f32::from_bits(u32::try_from(BINARY32.of(x)).unwrap_or(u32::MAX))
}

/// `x` rounded to the nearest IEEE binary16, as its bits.
pub fn half_bits(x: f64) -> u16 {
    u16::try_from(BINARY16.of(x)).unwrap_or(u16::MAX)
}

/// The binary16 `bits` as an `f32`, exactly.
pub fn half(bits: u16) -> f32 {
    let sign = if bits & HALF_SIGN == 0 { 1.0_f32 } else { -1.0_f32 };
    let exponent = u32::from((bits >> BINARY16.fraction) & 0x1f);
    let fraction = u32::from(bits & HALF_FRACTION_MASK);
    if exponent == 0 {
        return sign * f32::from(u16::try_from(fraction).unwrap_or(0)) * HALF_SUBNORMAL_UNIT;
    }
    let sign_bit = u32::from(bits & HALF_SIGN) << 16_u32;
    if exponent == HALF_SPECIAL {
        return f32::from_bits(sign_bit | (0xff << BINARY32.fraction) | (fraction << HALF_TO_SINGLE_SHIFT));
    }
    let widened = exponent.saturating_add(SINGLE_BIAS).saturating_sub(HALF_BIAS);
    f32::from_bits(sign_bit | (widened << BINARY32.fraction) | (fraction << HALF_TO_SINGLE_SHIFT))
}

/// `x` as an integer, if it is one that an `i64` holds.
pub fn integral(x: f64) -> Option<i64> {
    if x == 0.0_f64 {
        return Some(0);
    }
    let bits = x.to_bits();
    let biased = i32::try_from((bits >> F64_FRACTION) & 0x7ff).ok()?;
    let exponent = biased.checked_sub(F64_BIAS)?;
    if exponent < 0_i32 || biased == F64_SPECIAL || exponent > 62_i32 {
        return None;
    }
    let significand = (bits & F64_FRACTION_MASK) | (1 << F64_FRACTION);
    let shift = u32::try_from(exponent).ok()?;
    let magnitude = if shift >= F64_FRACTION {
        significand.checked_shl(shift.saturating_sub(F64_FRACTION))?
    } else {
        let drop = F64_FRACTION.saturating_sub(shift);
        if significand & low_mask(drop) != 0 {
            return None;
        }
        significand >> drop
    };
    let value = i64::try_from(magnitude).ok()?;
    Some(if bits >> F64_SIGN == 0 { value } else { value.checked_neg()? })
}

/// The bits of a u32's range.
const HALF_WORD: u32 = 32;
/// 2^32, a high half's weight.
const HIGH_WEIGHT: f64 = 4_294_967_296.0;

/// `x` as an `f64`: exactly while its magnitude is below 2^53, as every
/// integer sum of a kernel's is.
pub fn wide(x: i64) -> f64 {
    let high = i32::try_from(x >> HALF_WORD).unwrap_or_default();
    let low = u32::try_from(x & i64::from(u32::MAX)).unwrap_or_default();
    f64::from(high) * HIGH_WEIGHT + f64::from(low)
}

/// `n` as an `f64`, exactly for every count a vector has.
pub fn count(n: usize) -> f64 {
    u32::try_from(n).map_or(f64::from(u32::MAX), f64::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::round_to_single;

    /// Verifies: [EXTENSION] [DESIGN] narrowing to binary32 by bits agrees
    /// with the hardware's rounding (the one `as` deviation, R-DEV-3) on
    /// normals, ties, subnormals, overflow and specials.
    #[test]
    fn single_matches_hardware_rounding() {
        let mut cases = vec![
            0.0_f64,
            -0.0_f64,
            1.0_f64,
            0.1_f64,
            -2.5_f64,
            1e-40_f64,
            1e-46_f64,
            7e-46_f64,
            3.4e38_f64,
            3.5e38_f64,
            1e300_f64,
            -1e-300_f64,
        ];
        cases.push(f64::from(f32::MAX) + 1e22_f64);
        cases.push(1.0_f64 + f64::EPSILON * 2_f64.powi(28));
        cases.push(1.0_f64 + f64::EPSILON * 2_f64.powi(29));
        cases.push(1.0_f64 + 3.0_f64 * f64::EPSILON * 2_f64.powi(28));
        let mut state = 0x1234_5678_u64;
        for _ in 0_i32..10_000_i32 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            cases.push(f64::from_bits(state));
        }
        for x in cases.into_iter().filter(|x| x.is_finite()) {
            let ours = f64::from(single(x));
            let theirs = round_to_single(x);
            assert_eq!(ours.to_bits(), theirs.to_bits(), "{x:e}");
        }
        assert!(single(f64::NAN).is_nan());
        assert_eq!(single(f64::NEG_INFINITY), f32::NEG_INFINITY);
    }

    /// Verifies: [EXTENSION] binary16 round trip: every finite half
    /// widens to an f32 and narrows back to its own bits; halfway cases
    /// round to even; 65520 overflows to infinity, 65504 is the largest.
    #[test]
    fn half_round_trips_and_rounds_to_even() {
        for bits in 0..=u16::MAX {
            let x = half(bits);
            if x.is_nan() {
                assert!(half(half_bits(f64::from(x))).is_nan());
                continue;
            }
            assert_eq!(half_bits(f64::from(x)), bits, "{bits:#x}");
        }
        assert_eq!(half(half_bits(65504.0)), 65504.0);
        assert!(half(half_bits(65520.0)).is_infinite());
        assert_eq!(half(half_bits(65519.0)), 65504.0);
        // 1 + 2^-11 is halfway between 1 and 1 + 2^-10: ties to 1 (even).
        assert_eq!(half(half_bits(1.0 + 2_f64.powi(-11))), 1.0);
        // 1 + 3 * 2^-11 ties up to 1 + 2^-9 (even).
        assert_eq!(f64::from(half(half_bits(1.0 + 3.0 * 2_f64.powi(-11)))), 1.0_f64 + 2_f64.powi(-9));
        assert_eq!(f64::from(half(half_bits(0.1))), 0.099_975_585_937_5_f64);
        assert_eq!(half(half_bits(2_f64.powi(-24))), HALF_SUBNORMAL_UNIT);
        assert_eq!(half(half_bits(2_f64.powi(-26))), 0.0);
    }

    /// Verifies: [EXTENSION] integral gives an integer only for integral
    /// values an i64 holds.
    #[test]
    fn integral_takes_integers_only() {
        assert_eq!(integral(0.0), Some(0));
        assert_eq!(integral(-128.0), Some(-128));
        assert_eq!(integral(127.0), Some(127));
        assert_eq!(integral(1e15), Some(1_000_000_000_000_000));
        assert_eq!(integral(2_f64.powi(60)), Some(1 << 60_i32));
        assert_eq!(integral(0.5), None);
        assert_eq!(integral(-1.25), None);
        assert_eq!(integral(1e30), None);
        assert_eq!(integral(f64::NAN), None);
        assert_eq!(integral(f64::INFINITY), None);
        assert_eq!(count(3), 3.0_f64);
    }
}
