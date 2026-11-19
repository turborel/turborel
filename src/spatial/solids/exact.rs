//! \[DESIGN\] Exact signed integers of any size, the fallback of the 3D
//! predicates and the arithmetic of plane-based geometry. A binary64
//! number is an odd integer times a power of two, so a set of them shares
//! a least power `2^e` and each is an integer multiple of it ([`Scale`]).
//! Planes, their meets and the signs of points against planes are then
//! integer polynomials of the inputs, computed with no rounding at all.

use std::cmp::Ordering;

/// IEEE 754 binary64: 52 fraction bits and 11 exponent bits.
const FRACTION_BITS: u32 = 52;
/// The fraction field's mask.
const FRACTION_MASK: u64 = 0x000f_ffff_ffff_ffff;
/// The exponent field's mask, after shifting the fraction out.
const EXPONENT_MASK: u64 = 0x7ff;
/// The exponent bias plus the fraction width: a normal number is its
/// significand times `2^(field - UNBIAS)`.
const UNBIAS: i64 = 1075;
/// The bits of a limb.
const LIMB_BITS: u32 = 64;
/// 2^64 as a float, to weigh a limb by the one below it.
const LIMB_SCALE: f64 = 4_294_967_296.0 * 4_294_967_296.0;

/// A signed integer of any size: its magnitude's limbs, least first, with
/// no high zero limb, and its sign. Zero has no limbs and is not negative.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Big {
    negative: bool,
    limbs: Vec<u64>,
}

impl Big {
    /// Zero.
    pub fn zero() -> Big {
        Big::default()
    }

    /// The integer `v`.
    pub fn of(v: i64) -> Big {
        Big::from_parts(v < 0, vec![v.unsigned_abs()])
    }

    fn from_parts(negative: bool, mut limbs: Vec<u64>) -> Big {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Big { negative: negative && !limbs.is_empty(), limbs }
    }

    /// `magnitude * 2^shift`, negated if `negative`.
    pub fn shifted(negative: bool, magnitude: u64, shift: u32) -> Big {
        let whole = usize::try_from(shift.checked_div(LIMB_BITS).unwrap_or_default()).unwrap_or_default();
        let part = shift.checked_rem(LIMB_BITS).unwrap_or_default();
        let mut limbs = vec![0_u64; whole];
        limbs.push(magnitude.checked_shl(part).unwrap_or_default());
        if part > 0 {
            limbs.push(magnitude.checked_shr(LIMB_BITS.saturating_sub(part)).unwrap_or_default());
        }
        Big::from_parts(negative, limbs)
    }

    /// Its sign.
    pub fn sign(&self) -> Ordering {
        match (self.limbs.is_empty(), self.negative) {
            (true, _) => Ordering::Equal,
            (false, true) => Ordering::Less,
            (false, false) => Ordering::Greater,
        }
    }

    /// Whether it is zero.
    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    /// Its negation.
    #[must_use]
    pub fn neg(&self) -> Big {
        Big::from_parts(!self.negative, self.limbs.clone())
    }

    /// `self + other`.
    #[must_use]
    pub fn add(&self, other: &Big) -> Big {
        if self.negative == other.negative {
            return Big::from_parts(self.negative, add_magnitudes(&self.limbs, &other.limbs));
        }
        match compare_magnitudes(&self.limbs, &other.limbs) {
            Ordering::Equal => Big::zero(),
            Ordering::Greater => Big::from_parts(self.negative, subtract_magnitudes(&self.limbs, &other.limbs)),
            Ordering::Less => Big::from_parts(other.negative, subtract_magnitudes(&other.limbs, &self.limbs)),
        }
    }

    /// `self - other`.
    #[must_use]
    pub fn sub(&self, other: &Big) -> Big {
        self.add(&other.neg())
    }

    /// `self * other`.
    #[must_use]
    pub fn mul(&self, other: &Big) -> Big {
        let mut limbs = vec![0_u64; self.limbs.len().saturating_add(other.limbs.len())];
        for (at, left) in self.limbs.iter().enumerate() {
            let mut carry = 0_u128;
            let rights = other.limbs.iter().copied().chain(std::iter::once(0));
            for (out, right) in limbs.iter_mut().skip(at).zip(rights) {
                // A limb product plus a limb and a carry fits in 128 bits.
                let value = u128::from(*left).wrapping_mul(u128::from(right)).wrapping_add(u128::from(*out)).wrapping_add(carry);
                *out = low(value);
                carry = value.checked_shr(LIMB_BITS).unwrap_or_default();
            }
        }
        Big::from_parts(self.negative != other.negative, limbs)
    }

    /// `self * self`.
    #[must_use]
    pub fn square(&self) -> Big {
        self.mul(self)
    }

    /// Compares two integers.
    pub fn compare(&self, other: &Big) -> Ordering {
        self.sub(other).sign()
    }

    /// Its value as `mantissa * 2^exponent`, the mantissa a float within
    /// one rounding of the top two limbs, so huge integers do not
    /// overflow.
    pub fn approximate(&self) -> (f64, i64) {
        let Some((top, rest)) = self.limbs.split_last() else { return (0.0_f64, 0) };
        let below = rest.last().copied().unwrap_or_default();
        let mantissa = limb(*top).mul_add(LIMB_SCALE, limb(below)) / LIMB_SCALE;
        let skipped = i64::try_from(rest.len()).unwrap_or(i64::MAX).saturating_mul(i64::from(LIMB_BITS));
        let signed = if self.negative { -mantissa } else { mantissa };
        (signed, skipped.saturating_add(i64::from(LIMB_BITS)))
    }
}

/// The low 64 bits of `value`.
fn low(value: u128) -> u64 {
    u64::try_from(value & u128::from(u64::MAX)).unwrap_or_default()
}

/// A limb as a float, exact to rounding.
fn limb(word: u64) -> f64 {
    let high = u32::try_from(word >> 32_u32).unwrap_or_default();
    let low_half = u32::try_from(word & u64::from(u32::MAX)).unwrap_or_default();
    f64::from(high).mul_add(4_294_967_296.0_f64, f64::from(low_half))
}

fn compare_magnitudes(left: &[u64], right: &[u64]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.iter().rev().cmp(right.iter().rev()))
}

fn add_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64> {
    let width = left.len().max(right.len());
    let mut out = Vec::with_capacity(width.saturating_add(1));
    let mut carry = false;
    for at in 0..width {
        let (sum, first) = left.get(at).copied().unwrap_or_default().overflowing_add(right.get(at).copied().unwrap_or_default());
        let (sum, second) = sum.overflowing_add(u64::from(carry));
        carry = first || second;
        out.push(sum);
    }
    out.push(u64::from(carry));
    out
}

/// `left - right` for `left >= right`, so the last borrow is zero.
fn subtract_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64> {
    let mut out = Vec::with_capacity(left.len());
    let mut borrow = false;
    for (at, word) in left.iter().enumerate() {
        let (difference, first) = word.overflowing_sub(right.get(at).copied().unwrap_or_default());
        let (difference, second) = difference.overflowing_sub(u64::from(borrow));
        borrow = first || second;
        out.push(difference);
    }
    out
}

/// A finite binary64 number as `(negative, odd integer, power of two)`,
/// or None for zero.
pub fn decompose(value: f64) -> Option<(bool, u64, i64)> {
    let bits = value.to_bits();
    let field = (bits >> FRACTION_BITS) & EXPONENT_MASK;
    let fraction = bits & FRACTION_MASK;
    let (significand, exponent) = if field == 0 {
        (fraction, 1_i64.saturating_sub(UNBIAS))
    } else {
        (fraction | (1_u64 << FRACTION_BITS), i64::try_from(field).unwrap_or_default().saturating_sub(UNBIAS))
    };
    if significand == 0 {
        return None;
    }
    let zeros = significand.trailing_zeros();
    Some((value.is_sign_negative(), significand >> zeros, exponent.saturating_add(i64::from(zeros))))
}

/// The common power of two of a set of binary64 numbers: each is an
/// integer multiple of `2^exponent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// The least power of two of the numbers' lowest set bits.
    pub exponent: i64,
}

impl Scale {
    /// The scale of `values`, each finite.
    pub fn of(values: impl IntoIterator<Item = f64>) -> Scale {
        let least = values.into_iter().filter_map(decompose).map(|(_, _, e)| e).min();
        Scale { exponent: least.unwrap_or_default() }
    }

    /// `value / 2^exponent`, an integer since the scale covers `value`.
    pub fn integer(self, value: f64) -> Big {
        let Some((negative, odd, power)) = decompose(value) else { return Big::zero() };
        let shift = u32::try_from(power.saturating_sub(self.exponent)).unwrap_or_default();
        Big::shifted(negative, odd, shift)
    }

    /// The float nearest `numerator / denominator * 2^exponent`,
    /// correctly rounded, for a nonzero denominator; zero for a zero one.
    pub fn ratio(self, numerator: &Big, denominator: &Big) -> f64 {
        if denominator.is_zero() || numerator.is_zero() {
            return 0.0_f64;
        }
        let negative = numerator.negative != denominator.negative;
        let (top, bottom) = (numerator.magnitude(), denominator.magnitude());
        // Scale so the quotient has QUOTIENT_BITS bits, give or take one.
        let shift = QUOTIENT_BITS.saturating_sub(top.log2().saturating_sub(bottom.log2()));
        let (top, bottom) = if shift >= 0 { (top.times_two_to(shift), bottom) } else { (top, bottom.times_two_to(shift.saturating_neg())) };
        let quotient = top.quotient(&bottom);
        // A remainder joins the lowest bit, below the rounding position.
        let exact = Big::shifted(false, quotient, 0).mul(&bottom).compare(&top) == Ordering::Equal;
        let sticky = quotient | u64::from(!exact);
        let high = u32::try_from(sticky >> 32_u32).unwrap_or_default();
        let low_half = u32::try_from(sticky & u64::from(u32::MAX)).unwrap_or_default();
        let value = f64::from(high).mul_add(4_294_967_296.0_f64, f64::from(low_half));
        let scaled = times_power_of_two(value, self.exponent.saturating_sub(shift));
        if negative { -scaled } else { scaled }
    }
}

/// The bits a quotient is scaled to before rounding to 53.
const QUOTIENT_BITS: i64 = 58;

impl Big {
    /// Its absolute value.
    fn magnitude(&self) -> Big {
        Big { negative: false, limbs: self.limbs.clone() }
    }

    /// The position of its highest set bit, for a nonzero value.
    fn log2(&self) -> i64 {
        let Some(top) = self.limbs.last() else { return 0 };
        let below = i64::try_from(self.limbs.len().saturating_sub(1)).unwrap_or(i64::MAX).saturating_mul(i64::from(LIMB_BITS));
        below.saturating_add(i64::from(LIMB_BITS.saturating_sub(1).saturating_sub(top.leading_zeros())))
    }

    /// It times `2^power`, `power` not negative.
    fn times_two_to(&self, power: i64) -> Big {
        self.mul(&Big::shifted(false, 1, u32::try_from(power).unwrap_or(u32::MAX)))
    }

    /// `self / divisor` rounded down, both positive and the quotient
    /// below `2^63`, bit by bit.
    fn quotient(&self, divisor: &Big) -> u64 {
        let mut out = 0_u64;
        for bit in (0..63_u32).rev() {
            let candidate = out | (1_u64 << bit);
            if Big::shifted(false, candidate, 0).mul(divisor).compare(self) != Ordering::Greater {
                out = candidate;
            }
        }
        out
    }
}

/// The largest power of two applied in one step, so no factor overflows.
const STEP: i64 = 512;

/// `value * 2^power`, overflowing to infinity or underflowing to zero only
/// if the result does.
pub fn times_power_of_two(value: f64, power: i64) -> f64 {
    let mut out = value;
    let mut left = power;
    while left != 0 && out != 0.0_f64 && out.is_finite() {
        let step = left.clamp(-STEP, STEP);
        out *= 2.0_f64.powi(i32::try_from(step).unwrap_or_default());
        left = left.saturating_sub(step);
    }
    out
}
