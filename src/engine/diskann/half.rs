//! \[DESIGN\] IEEE 754 binary16 node vectors, by bits: a node of FLOAT16
//! coordinates is half the bytes, so twice as many share a page and a hop
//! of a wider vector stays one read [DiskANN, NeurIPS 2019 §3.2]. The
//! conversion rounds to nearest, ties to even, without `as` (R-NUM-2).

/// The bits of a binary32 sign.
const SIGN_32: u32 = 0x8000_0000;
/// The exponent bias of binary32.
const BIAS_32: i32 = 127;
/// The exponent bias of binary16.
const BIAS_16: i32 = 15;
/// The mantissa bits of binary32.
const MANTISSA_32: u32 = 23;
/// The mantissa bits of binary16.
const MANTISSA_16: u32 = 10;
/// The mantissa bits binary16 drops of binary32's.
const DROPPED: u32 = MANTISSA_32 - MANTISSA_16;
/// A binary32 exponent field of all ones: infinity or NaN.
const EXPONENT_32: u32 = 0xFF;
/// A binary16 exponent field of all ones.
const EXPONENT_16: u32 = 0x1F;
/// The binary32 mantissa mask.
const MANTISSA_MASK_32: u32 = (1 << MANTISSA_32) - 1;
/// The binary16 mantissa mask.
const MANTISSA_MASK_16: u16 = (1 << MANTISSA_16) - 1;
/// A quiet NaN's leading mantissa bit in binary16.
const QUIET_16: u32 = 1 << (MANTISSA_16 - 1);
/// The shift that moves a binary16 sign to binary32's place.
const SIGN_SHIFT: u32 = 16;
/// The least normal binary16 exponent, unbiased.
const MIN_NORMAL_16: i32 = 1 - BIAS_16;

/// `value` rounded to binary16, its bits.
pub(super) fn to_half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = (bits & SIGN_32) >> SIGN_SHIFT;
    let field = (bits >> MANTISSA_32) & EXPONENT_32;
    let mantissa = bits & MANTISSA_MASK_32;
    let half = if field == EXPONENT_32 {
        let nan = if mantissa == 0 { 0 } else { QUIET_16 };
        (EXPONENT_16 << MANTISSA_16) | nan
    } else {
        finite(field, mantissa)
    };
    u16::try_from(sign | half).unwrap_or(u16::MAX)
}

/// The magnitude bits of a finite binary32 of exponent `field` rounded to
/// binary16: overflow to infinity, underflow through subnormals to zero.
fn finite(field: u32, mantissa: u32) -> u32 {
    let exponent = i32::try_from(field).unwrap_or(0_i32).saturating_sub(BIAS_32);
    if field == 0 || exponent < MIN_NORMAL_16.saturating_sub(i32::try_from(MANTISSA_16).unwrap_or(0_i32)).saturating_sub(1) {
        return 0;
    }
    let (shift, significand, biased) = if exponent < MIN_NORMAL_16 {
        let below = u32::try_from(MIN_NORMAL_16.saturating_sub(exponent)).unwrap_or(0);
        (DROPPED.saturating_add(below), mantissa | (1 << MANTISSA_32), 0_u32)
    } else {
        (DROPPED, mantissa, u32::try_from(exponent.saturating_add(BIAS_16)).unwrap_or(0))
    };
    let kept = significand >> shift;
    let rest = significand & ((1_u32 << shift).saturating_sub(1));
    let halfway = 1_u32 << shift.saturating_sub(1);
    let up = rest > halfway || (rest == halfway && kept & 1 == 1);
    // A carry out of the mantissa raises the exponent, as the format's
    // encoding makes adjacent, so rounding the largest finite gives
    // infinity.
    let rounded = ((biased << MANTISSA_16) | kept).saturating_add(u32::from(up));
    rounded.min(EXPONENT_16 << MANTISSA_16)
}

/// The binary32 value of binary16 `bits`, exactly.
pub(super) fn from_half(bits: u16) -> f32 {
    let sign = (u32::from(bits) << SIGN_SHIFT) & SIGN_32;
    let field = u32::from(bits >> MANTISSA_16) & EXPONENT_16;
    let mantissa = u32::from(bits & MANTISSA_MASK_16);
    let magnitude = if field == EXPONENT_16 {
        (EXPONENT_32 << MANTISSA_32) | (mantissa << DROPPED)
    } else if field == 0 {
        return signed(sign, subnormal(mantissa));
    } else {
        let biased = field.saturating_add(u32::try_from(BIAS_32.saturating_sub(BIAS_16)).unwrap_or(0));
        (biased << MANTISSA_32) | (mantissa << DROPPED)
    };
    f32::from_bits(sign | magnitude)
}

/// A binary16 subnormal's value, its mantissa times 2^-24, exact in f32.
fn subnormal(mantissa: u32) -> f32 {
    /// 2^-24, the binary16 subnormal unit.
    const UNIT: f32 = 5.960_464_5e-8_f32;
    f32::from(u16::try_from(mantissa).unwrap_or(0)) * UNIT
}

fn signed(sign: u32, magnitude: f32) -> f32 {
    if sign == 0 { magnitude } else { -magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] Every binary16 value converts to binary32 and
    /// back to the same bits, NaNs as NaNs; values between round to the
    /// nearest, ties to even; overflow is infinity and underflow zero.
    #[test]
    fn half_precision_round_trips_and_rounds_to_nearest_even() {
        for bits in 0..=u16::MAX {
            let value = from_half(bits);
            if value.is_nan() {
                assert!(from_half(to_half(value)).is_nan());
            } else {
                assert_eq!(to_half(value), bits, "{bits:#x}");
            }
        }
        assert_eq!(to_half(1.0_f32 + 1.0_f32 / 2048.0_f32), to_half(1.0_f32));
        assert_eq!(to_half(1.0_f32 + 3.0_f32 / 2048.0_f32), to_half(1.0_f32 + 1.0_f32 / 512.0_f32));
        assert_eq!(from_half(to_half(1.0e6_f32)), f32::INFINITY);
        assert_eq!(from_half(to_half(-1.0e-9_f32)), -0.0_f32);
        assert_eq!(from_half(to_half(0.333_333_34_f32)), 0.333_251_95_f32);
    }
}
