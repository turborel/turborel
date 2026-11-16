//! Exact dyadic arithmetic for geometric predicate and distance fallback. Scaling finite
//! binary64 values by 2^1074 makes them integers. Differences need at
//! most 2099 bits and determinant products at most 4198 bits; the vectors
//! are therefore bounded by the input format, not by geometry size.

use std::cmp::Ordering;

/// IEEE 754 binary64: 52 fraction bits, 11 exponent bits, one sign bit.
const FRACTION_BITS: u32 = 52;
const FRACTION_MASK: u64 = 0x000f_ffff_ffff_ffff;
const EXPONENT_MASK: u64 = 0x7ff;
const SIGN_SHIFT: u32 = 63;
const LIMB_BITS: usize = 64;
const LOW_LIMB: u128 = 0xffff_ffff_ffff_ffff;
const HALF_BITS: u32 = 32;
const HALF_MASK: u64 = 0xffff_ffff;
const HALF_SCALE: f64 = 4_294_967_296.0;
const PREVIOUS_LIMB_SCALE: f64 = 1.0 / (HALF_SCALE * HALF_SCALE);
const LIMB_EXPONENT: i32 = 64;

#[derive(Clone)]
pub(super) struct Integer {
    negative: bool,
    limbs: Vec<u64>,
}

impl Integer {
    pub(super) fn coordinate(value: f64) -> Self {
        let bits = value.to_bits();
        let exponent = usize::try_from((bits >> FRACTION_BITS) & EXPONENT_MASK).unwrap_or_default();
        let fraction = bits & FRACTION_MASK;
        let mantissa = if exponent == 0 { fraction } else { fraction | (1_u64 << FRACTION_BITS) };
        let shift = exponent.saturating_sub(1);
        let mut limbs = vec![0; shift.div_euclid(LIMB_BITS)];
        let offset = shift.rem_euclid(LIMB_BITS);
        limbs.push(mantissa << offset);
        if offset > 0 {
            limbs.push(mantissa >> LIMB_BITS.saturating_sub(offset));
        }
        Self::of(bits >> SIGN_SHIFT != 0, limbs)
    }

    fn of(negative: bool, mut limbs: Vec<u64>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { negative: negative && !limbs.is_empty(), limbs }
    }

    pub(super) fn sign(&self) -> Ordering {
        if self.limbs.is_empty() {
            Ordering::Equal
        } else if self.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }

    pub(super) fn subtract(&self, other: &Self) -> Self {
        if self.negative != other.negative {
            return Self::of(self.negative, add(&self.limbs, &other.limbs));
        }
        let order = self.limbs.len().cmp(&other.limbs.len()).then_with(|| self.limbs.iter().rev().cmp(other.limbs.iter().rev()));
        match order {
            Ordering::Less => Self::of(!self.negative, subtract(&other.limbs, &self.limbs)),
            Ordering::Equal => Self::of(false, Vec::new()),
            Ordering::Greater => Self::of(self.negative, subtract(&self.limbs, &other.limbs)),
        }
    }

    pub(super) fn multiply(&self, other: &Self) -> Self {
        let mut limbs = vec![0; self.limbs.len().saturating_add(other.limbs.len())];
        for (position, left) in self.limbs.iter().copied().enumerate() {
            let mut carry = 0_u128;
            let factors = other.limbs.iter().copied().chain(std::iter::once(0));
            for (out, right) in limbs.iter_mut().skip(position).zip(factors) {
                // A limb product plus one limb and carry fits exactly in u128.
                let value = u128::from(left).wrapping_mul(u128::from(right)).wrapping_add(u128::from(*out)).wrapping_add(carry);
                *out = u64::try_from(value & LOW_LIMB).unwrap_or_default();
                carry = value >> LIMB_BITS;
            }
        }
        Self::of(self.negative != other.negative, limbs)
    }

    pub(super) fn add(&self, other: &Self) -> Self {
        let opposite = Self { negative: !other.negative, limbs: other.limbs.clone() };
        self.subtract(&opposite)
    }

    /// The leading two limbs give a floating magnitude without overflowing;
    /// the separate base-two exponent retains its original scale.
    pub(super) fn magnitude(&self) -> (f64, i32) {
        let Some((last, rest)) = self.limbs.split_last() else { return (0.0_f64, 0) };
        let mantissa = real(*last) + real(rest.last().copied().unwrap_or_default()) * PREVIOUS_LIMB_SCALE;
        let exponent = i32::try_from(rest.len()).unwrap_or_default().saturating_mul(LIMB_EXPONENT);
        (mantissa, exponent)
    }
}

fn real(word: u64) -> f64 {
    let high = u32::try_from(word >> HALF_BITS).unwrap_or_default();
    let low = u32::try_from(word & HALF_MASK).unwrap_or_default();
    f64::from(high) * HALF_SCALE + f64::from(low)
}

fn add(left: &[u64], right: &[u64]) -> Vec<u64> {
    let mut limbs = Vec::with_capacity(left.len().max(right.len()).saturating_add(1));
    let mut carry = false;
    for position in 0..left.len().max(right.len()) {
        let (sum, first) =
            left.get(position).copied().unwrap_or_default().overflowing_add(right.get(position).copied().unwrap_or_default());
        let (sum, second) = sum.overflowing_add(u64::from(carry));
        carry = first || second;
        limbs.push(sum);
    }
    limbs.push(u64::from(carry));
    limbs
}

/// Requires left >= right; subtraction's final borrow is therefore zero.
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
