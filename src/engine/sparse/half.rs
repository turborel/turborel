//! \[DESIGN\] IEEE 754 binary16 ("half") conversions for the sparse
//! inverted index's FLOAT16 weights, done on the bits so that no `as`
//! conversion is needed (RULES.md R-NUM-2). A single is rounded to the
//! nearest half, ties to even, as IEEE 754-2008 §4.3.1 asks of a
//! conversion; a half widens to a single exactly.

/// The bits of a single's sign.
const SINGLE_SIGN: u32 = 0x8000_0000;
/// A single's biased exponent field, after shifting its mantissa out.
const SINGLE_EXPONENT: u32 = 0xff;
/// The bits of a single's mantissa.
const SINGLE_MANTISSA: u32 = 0x007f_ffff;
/// A single's implicit leading mantissa bit.
const SINGLE_IMPLICIT: u32 = 0x0080_0000;
/// The bits of a single's mantissa a half drops.
const DROPPED_BITS: u32 = 13;
/// A single's mantissa width.
const SINGLE_MANTISSA_BITS: u32 = 23;
/// The difference of the two exponent biases, 127 - 15.
const REBIAS: u32 = 112;
/// The least single exponent of a normal half, 1 + `REBIAS`.
const LEAST_NORMAL: u32 = 113;
/// The greatest single exponent of a finite half, 30 + `REBIAS`.
const MOST_FINITE: u32 = 142;
/// The least single exponent that does not round to a zero half: 2^-25.
const LEAST_SUBNORMAL: u32 = 102;
/// The exponent at which a single's full mantissa counts 2^-24 units.
const SUBNORMAL_SHIFT: u32 = 126;
/// A half's infinity, without its sign.
const HALF_INFINITY: u16 = 0x7c00;
/// A half's quiet NaN, without its sign.
const HALF_NAN: u16 = 0x7e00;
/// The bits of a half's sign.
const HALF_SIGN: u16 = 0x8000;
/// A half's exponent field, after shifting its mantissa out.
const HALF_EXPONENT: u16 = 0x1f;
/// The bits of a half's mantissa.
const HALF_MANTISSA: u16 = 0x03ff;
/// A half's mantissa width.
const HALF_MANTISSA_BITS: u16 = 10;
/// The value of a half's least subnormal step, 2^-24.
const HALF_UNIT: f32 = 5.960_464_5e-8_f32;

/// The half nearest `value`, ties to even; an infinity past the greatest
/// finite half, 65504.
pub fn to_half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = if bits & SINGLE_SIGN == 0 { 0 } else { HALF_SIGN };
    let exponent = (bits >> SINGLE_MANTISSA_BITS) & SINGLE_EXPONENT;
    let mantissa = bits & SINGLE_MANTISSA;
    if exponent == SINGLE_EXPONENT {
        return sign | if mantissa == 0 { HALF_INFINITY } else { HALF_NAN };
    }
    if exponent > MOST_FINITE {
        return sign | HALF_INFINITY;
    }
    if exponent >= LEAST_NORMAL {
        let rebased = (exponent.wrapping_sub(REBIAS) << HALF_MANTISSA_BITS) | (mantissa >> DROPPED_BITS);
        // A carry out of the mantissa raises the exponent, as rounding up
        // to the next binade must; past 30 it makes the infinity.
        let rounded = round_even(rebased, mantissa, DROPPED_BITS);
        return sign | u16::try_from(rounded).unwrap_or(HALF_INFINITY);
    }
    if exponent < LEAST_SUBNORMAL {
        return sign;
    }
    let full = mantissa | SINGLE_IMPLICIT;
    let shift = SUBNORMAL_SHIFT.wrapping_sub(exponent);
    let rounded = round_even(full.checked_shr(shift).unwrap_or(0), full, shift);
    sign | u16::try_from(rounded).unwrap_or(HALF_INFINITY)
}

/// `kept`, the bits of `full` above its low `dropped`, rounded to nearest
/// by those dropped, ties to even.
fn round_even(kept: u32, full: u32, dropped: u32) -> u32 {
    let mask = 1_u32.checked_shl(dropped).map_or(u32::MAX, |b| b.wrapping_sub(1));
    let rest = full & mask;
    let half = 1_u32.checked_shl(dropped.saturating_sub(1)).unwrap_or(0);
    if rest > half || rest == half && kept & 1 == 1 { kept.wrapping_add(1) } else { kept }
}

/// The single `half` is, exactly.
pub fn from_half(half: u16) -> f32 {
    let negative = half & HALF_SIGN != 0;
    let exponent = (half >> HALF_MANTISSA_BITS) & HALF_EXPONENT;
    let mantissa = half & HALF_MANTISSA;
    let magnitude = if exponent == 0 {
        f32::from(mantissa) * HALF_UNIT
    } else if exponent == HALF_EXPONENT {
        if mantissa == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        let biased = u32::from(exponent).wrapping_add(REBIAS);
        f32::from_bits((biased << SINGLE_MANTISSA_BITS) | (u32::from(mantissa) << DROPPED_BITS))
    };
    if negative { -magnitude } else { magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] Every finite half widens to a single and narrows
    /// back to the same bits, so stored FLOAT16 weights read back exactly.
    #[test]
    fn every_finite_half_round_trips() {
        for bits in 0..=u16::MAX {
            let value = from_half(bits);
            if value.is_finite() {
                assert_eq!(to_half(value), bits, "{bits:#06x}");
            }
        }
    }

    /// Verifies: [DESIGN] Narrowing rounds to the nearest half, ties to
    /// even, overflows to infinity past 65504 and underflows to zero below
    /// half the least subnormal.
    #[test]
    fn narrowing_rounds_to_nearest_even() {
        let narrowed = |x: f32| from_half(to_half(x)).to_bits();
        assert_eq!(narrowed(1.0_f32 + 1.0_f32 / 2048.0_f32), 1.0_f32.to_bits());
        assert_eq!(narrowed(1.0_f32 + 3.0_f32 / 2048.0_f32), (1.0_f32 + 2.0_f32 / 1024.0_f32).to_bits());
        assert_eq!(narrowed(65_504.0_f32), 65_504.0_f32.to_bits());
        assert!(from_half(to_half(70_000.0_f32)).is_infinite());
        assert_eq!(narrowed(-1.0e-9_f32), (-0.0_f32).to_bits());
        assert_eq!(narrowed(HALF_UNIT), HALF_UNIT.to_bits());
        assert_eq!(narrowed(-0.333_333_34_f32), (-0.333_251_95_f32).to_bits());
    }
}
