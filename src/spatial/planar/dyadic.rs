//! \[DESIGN\] Exact dyadic rationals, the last stage of every predicate.
//! A value is a sign, a magnitude in 64-bit limbs and a base-two
//! exponent. Every finite binary64 value is one, and sums, differences
//! and products of them are again exact; no division is ever needed, so
//! a polynomial's sign is decided without rounding at any exponent.

use std::cmp::Ordering;

/// IEEE 754 binary64 layout, with 52 fraction bits, 11 exponent bits and
/// a sign.
const FRACTION_BITS: u32 = 52;
const FRACTION_MASK: u64 = 0x000f_ffff_ffff_ffff;
const EXPONENT_MASK: u64 = 0x7ff;
const SIGN_SHIFT: u32 = 63;
/// The unbiased exponent of the least subnormal's unit, 2^-1074.
const LEAST_EXPONENT: i32 = -1074;
const LIMB_BITS: u32 = 64;
const LIMB_PLACES: i32 = 64;
const HALF_BITS: u32 = 32;
const HALF_MASK: u64 = 0xffff_ffff;
/// 2^32, exact in binary64.
const HALF_SCALE: f64 = 4_294_967_296.0;
const LIMB_MASK: u128 = 0xffff_ffff_ffff_ffff;
/// 2^64 and 2^-64, exact in binary64, for the approximate value.
const LIMB_SCALE: f64 = 1.844_674_407_370_955_2e19;
const LIMB_FRACTION: f64 = 1.0 / LIMB_SCALE;
/// The binary64 exponent range a power of two is built within.
const MIN_NORMAL_EXPONENT: i32 = -1022;
const MAX_NORMAL_EXPONENT: i32 = 1023;
const EXPONENT_BIAS: i32 = 1023;
/// The remaining scale is applied in two normal power-of-two steps.
const TWO_STEPS: i32 = 2;

/// `(-1)^negative × limbs × 2^exponent`, limbs little-endian with no
/// trailing zero limb; zero has no limbs.
#[derive(Clone, Debug)]
pub(crate) struct Dyadic {
    negative: bool,
    limbs: Vec<u64>,
    exponent: i32,
}

impl Dyadic {
    /// The exact value of a finite binary64 number.
    pub(crate) fn of(value: f64) -> Self {
        let bits = value.to_bits();
        let biased = i32::try_from((bits >> FRACTION_BITS) & EXPONENT_MASK).unwrap_or_default();
        let fraction = bits & FRACTION_MASK;
        let (mantissa, exponent) = if biased == 0_i32 {
            (fraction, LEAST_EXPONENT)
        } else {
            (fraction | (1_u64 << FRACTION_BITS), LEAST_EXPONENT.saturating_add(biased).saturating_sub(1))
        };
        Self::normal(bits >> SIGN_SHIFT != 0, vec![mantissa], exponent)
    }

    fn normal(negative: bool, mut limbs: Vec<u64>, exponent: i32) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { negative: negative && !limbs.is_empty(), limbs, exponent }
    }

    /// The sign of the value.
    pub(crate) fn sign(&self) -> Ordering {
        match (self.limbs.is_empty(), self.negative) {
            (true, _) => Ordering::Equal,
            (false, true) => Ordering::Less,
            (false, false) => Ordering::Greater,
        }
    }

    /// Half the value, exactly.
    pub(crate) fn halved(&self) -> Self {
        Self { negative: self.negative, limbs: self.limbs.clone(), exponent: self.exponent.saturating_sub(1) }
    }

    /// The exact product.
    pub(crate) fn times(&self, other: &Self) -> Self {
        let mut limbs = vec![0_u64; self.limbs.len().saturating_add(other.limbs.len())];
        for (position, left) in self.limbs.iter().copied().enumerate() {
            let mut carry = 0_u128;
            let factors = other.limbs.iter().copied().chain(std::iter::once(0));
            for (out, right) in limbs.iter_mut().skip(position).zip(factors) {
                // A limb product plus a limb and a carry fits exactly in u128.
                let value = u128::from(left).wrapping_mul(u128::from(right)).wrapping_add(u128::from(*out)).wrapping_add(carry);
                *out = u64::try_from(value & LIMB_MASK).unwrap_or_default();
                carry = value >> LIMB_BITS;
            }
        }
        Self::normal(self.negative != other.negative, limbs, self.exponent.saturating_add(other.exponent))
    }

    /// The exact sum.
    pub(crate) fn plus(&self, other: &Self) -> Self {
        if self.limbs.is_empty() {
            return other.clone();
        }
        if other.limbs.is_empty() {
            return self.clone();
        }
        let exponent = self.exponent.min(other.exponent);
        let left = shifted(&self.limbs, self.exponent.saturating_sub(exponent));
        let right = shifted(&other.limbs, other.exponent.saturating_sub(exponent));
        if self.negative == other.negative {
            return Self::normal(self.negative, add(&left, &right), exponent);
        }
        match compare(&left, &right) {
            Ordering::Less => Self::normal(other.negative, subtract(&right, &left), exponent),
            Ordering::Equal => Self::normal(false, Vec::new(), exponent),
            Ordering::Greater => Self::normal(self.negative, subtract(&left, &right), exponent),
        }
    }

    /// The exact difference.
    pub(crate) fn minus(&self, other: &Self) -> Self {
        let negated = Self { negative: !other.negative && !other.limbs.is_empty(), limbs: other.limbs.clone(), exponent: other.exponent };
        self.plus(&negated)
    }
}

/// The quotient of two exact values as binary64, within a few units in
/// the last place. Both are scaled to their leading limbs first, so the
/// division neither overflows nor loses an intermediate exponent.
pub(crate) fn quotient(numerator: &Dyadic, denominator: &Dyadic) -> f64 {
    let (Some((top, top_rest)), Some((bottom, bottom_rest))) = (numerator.limbs.split_last(), denominator.limbs.split_last()) else {
        return 0.0_f64;
    };
    let lead = |last: u64, rest: &[u64]| real(last) + real(rest.last().copied().unwrap_or_default()) * LIMB_FRACTION;
    let places =
        |rest: &[u64], exponent: i32| i32::try_from(rest.len()).unwrap_or(i32::MAX).saturating_mul(LIMB_PLACES).saturating_add(exponent);
    let ratio = lead(*top, top_rest) / lead(*bottom, bottom_rest);
    let exponent = places(top_rest, numerator.exponent).saturating_sub(places(bottom_rest, denominator.exponent));
    let value = scale(ratio, exponent);
    if numerator.negative == denominator.negative { value } else { -value }
}

fn real(word: u64) -> f64 {
    let high = u32::try_from(word >> HALF_BITS).unwrap_or_default();
    let low = u32::try_from(word & HALF_MASK).unwrap_or_default();
    f64::from(high) * HALF_SCALE + f64::from(low)
}

/// `value × 2^exponent` in two exact power-of-two steps, so neither
/// factor overflows before the product is formed.
fn scale(value: f64, exponent: i32) -> f64 {
    let first = exponent.clamp(MIN_NORMAL_EXPONENT, MAX_NORMAL_EXPONENT);
    let second =
        exponent.saturating_sub(first).clamp(MIN_NORMAL_EXPONENT.saturating_mul(TWO_STEPS), MAX_NORMAL_EXPONENT.saturating_mul(TWO_STEPS));
    let half = second.checked_div(TWO_STEPS).unwrap_or_default();
    value * power(first) * power(half) * power(second.saturating_sub(half))
}

/// 2^exponent for a normal exponent, built from its bits so no platform
/// `powi` rounding enters.
fn power(exponent: i32) -> f64 {
    let clamped = exponent.clamp(MIN_NORMAL_EXPONENT, MAX_NORMAL_EXPONENT);
    let biased = u64::try_from(clamped.saturating_add(EXPONENT_BIAS)).unwrap_or_default();
    f64::from_bits(biased << FRACTION_BITS)
}

/// Shifts a magnitude left by `bits`, which is never negative here.
fn shifted(limbs: &[u64], bits: i32) -> Vec<u64> {
    let bits = u32::try_from(bits).unwrap_or_default();
    let whole = usize::try_from(bits.checked_div(LIMB_BITS).unwrap_or_default()).unwrap_or_default();
    let part = bits % LIMB_BITS;
    let mut out = vec![0_u64; whole];
    out.reserve(limbs.len().saturating_add(1));
    let mut carry = 0_u64;
    for limb in limbs {
        if part == 0 {
            out.push(*limb);
        } else {
            out.push((limb << part) | carry);
            carry = limb >> LIMB_BITS.saturating_sub(part);
        }
    }
    out.push(carry);
    out
}

fn compare(left: &[u64], right: &[u64]) -> Ordering {
    let length = left.len().max(right.len());
    for position in (0..length).rev() {
        let order = left.get(position).copied().unwrap_or_default().cmp(&right.get(position).copied().unwrap_or_default());
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn add(left: &[u64], right: &[u64]) -> Vec<u64> {
    let length = left.len().max(right.len());
    let mut limbs = Vec::with_capacity(length.saturating_add(1));
    let mut carry = false;
    for position in 0..length {
        let (sum, first) =
            left.get(position).copied().unwrap_or_default().overflowing_add(right.get(position).copied().unwrap_or_default());
        let (sum, second) = sum.overflowing_add(u64::from(carry));
        carry = first || second;
        limbs.push(sum);
    }
    limbs.push(u64::from(carry));
    limbs
}

/// Requires `left >= right`, so the final borrow is zero.
fn subtract(left: &[u64], right: &[u64]) -> Vec<u64> {
    let mut limbs = Vec::with_capacity(left.len());
    let mut borrow = false;
    for (position, word) in left.iter().copied().enumerate() {
        let (difference, first) = word.overflowing_sub(right.get(position).copied().unwrap_or_default());
        let (difference, second) = difference.overflowing_sub(u64::from(borrow));
        borrow = first || second;
        limbs.push(difference);
    }
    limbs
}
