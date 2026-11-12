//! \[DESIGN\] An unsigned integer of 256 bits: enough for the exact product
//! of two DECFLOAT coefficients of 34 digits, or for two operands of a sum
//! aligned to one exponent, before either is rounded (`DecFloat`).

use std::cmp::Ordering;

/// Limbs of a `Wide`.
const LIMBS: usize = 4;
/// Bits of a limb.
const LIMB_BITS: u32 = u64::BITS;
/// The most decimal digits a `u64` multiplier or divisor of 10^n holds.
const LIMB_DIGITS: u32 = 19;
/// The decimal radix.
const RADIX: u64 = 10;

/// An unsigned integer of `LIMBS` 64-bit limbs, the least significant
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wide([u64; LIMBS]);

/// The low 64 bits of `n`.
fn low(n: u128) -> u64 {
    u64::try_from(n & u128::from(u64::MAX)).unwrap_or(u64::MAX)
}

/// The high 64 bits of `n`.
fn high(n: u128) -> u64 {
    u64::try_from(n >> LIMB_BITS).unwrap_or(u64::MAX)
}

/// The decimal digits of `n`, none for 0.
pub(super) fn digits(n: u128) -> u32 {
    n.checked_ilog10().map_or(0, |l| l.saturating_add(1))
}

impl Wide {
    /// `n`.
    pub(super) fn of(n: u128) -> Wide {
        Wide([low(n), high(n), 0, 0])
    }

    /// `a` × `b`, exactly.
    pub(super) fn product(left: u128, right: u128) -> Wide {
        let (lefts, rights) = ([low(left), high(left)], [low(right), high(right)]);
        let mut out = [0_u64; LIMBS];
        for (i, &l) in lefts.iter().enumerate() {
            let mut carry = 0_u128;
            for (j, &r) in rights.iter().enumerate() {
                let at = i.saturating_add(j);
                let limb = out.get(at).copied().unwrap_or_default();
                // (2^64 - 1)^2 + 2 (2^64 - 1) is 2^128 - 1: no overflow.
                let term = u128::from(l).wrapping_mul(u128::from(r)).wrapping_add(u128::from(limb)).wrapping_add(carry);
                if let Some(slot) = out.get_mut(at) {
                    *slot = low(term);
                }
                carry = u128::from(high(term));
            }
            if let Some(slot) = out.get_mut(i.saturating_add(rights.len())) {
                *slot = low(carry);
            }
        }
        Wide(out)
    }

    /// It × `m`, none if 256 bits cannot hold it.
    fn times(self, m: u64) -> Option<Wide> {
        let mut out = [0_u64; LIMBS];
        let mut carry = 0_u128;
        for (slot, &limb) in out.iter_mut().zip(&self.0) {
            let t = u128::from(limb).checked_mul(u128::from(m))?.checked_add(carry)?;
            *slot = low(t);
            carry = u128::from(high(t));
        }
        (carry == 0).then_some(Wide(out))
    }

    /// It × 10^`k`, none if 256 bits cannot hold it.
    pub(super) fn shifted(self, k: u32) -> Option<Wide> {
        let mut out = self;
        let mut left = k;
        while left > 0 {
            let step = left.min(LIMB_DIGITS);
            out = out.times(RADIX.checked_pow(step)?)?;
            left = left.saturating_sub(step);
        }
        Some(out)
    }

    /// It + `o`, none if 256 bits cannot hold it.
    pub(super) fn plus(self, o: Wide) -> Option<Wide> {
        let mut out = [0_u64; LIMBS];
        let mut carry = false;
        for ((slot, &a), &b) in out.iter_mut().zip(&self.0).zip(&o.0) {
            let (s, c1) = a.overflowing_add(b);
            let (s, c2) = s.overflowing_add(u64::from(carry));
            *slot = s;
            carry = c1 || c2;
        }
        (!carry).then_some(Wide(out))
    }

    /// It − `o`, none if `o` is greater.
    pub(super) fn minus(self, o: Wide) -> Option<Wide> {
        let mut out = [0_u64; LIMBS];
        let mut borrow = false;
        for ((slot, &a), &b) in out.iter_mut().zip(&self.0).zip(&o.0) {
            let (d, b1) = a.overflowing_sub(b);
            let (d, b2) = d.overflowing_sub(u64::from(borrow));
            *slot = d;
            borrow = b1 || b2;
        }
        (!borrow).then_some(Wide(out))
    }

    /// It ÷ `d`, and the remainder; `d` is not 0.
    pub(super) fn over(self, d: u64) -> (Wide, u64) {
        let mut out = [0_u64; LIMBS];
        let mut rem = 0_u128;
        for (slot, &limb) in out.iter_mut().zip(&self.0).rev() {
            let cur = (rem << LIMB_BITS) | u128::from(limb);
            // The remainder is below `d`, so the quotient fits a limb.
            *slot = low(cur.checked_div(u128::from(d)).unwrap_or_default());
            rem = cur.checked_rem(u128::from(d)).unwrap_or_default();
        }
        (Wide(out), low(rem))
    }

    /// It, if a `u128` holds it.
    pub(super) fn narrow(self) -> Option<u128> {
        let [a, b, c, d] = self.0;
        (c == 0 && d == 0).then(|| (u128::from(b) << LIMB_BITS) | u128::from(a))
    }

    /// Its decimal digits, none for 0.
    pub(super) fn digits(self) -> u32 {
        let mut w = self;
        let mut count = 0_u32;
        loop {
            if let Some(n) = w.narrow() {
                return count.saturating_add(digits(n));
            }
            // Above u128::MAX, so at least 39 digits: 19 go.
            w = w.over(RADIX.pow(LIMB_DIGITS)).0;
            count = count.saturating_add(LIMB_DIGITS);
        }
    }
}

impl PartialOrd for Wide {
    fn partial_cmp(&self, o: &Wide) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Wide {
    /// By value: the most significant limb first.
    fn cmp(&self, o: &Wide) -> Ordering {
        self.0.iter().rev().cmp(o.0.iter().rev())
    }
}

// Unit tests: the arithmetic of 256 bits (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] a product of two `u128`s is exact, past 128
    /// bits; a sum and a difference carry and borrow across limbs, none
    /// past 256 bits or below 0; ×10^k and ÷d invert each other; digits
    /// count past 38; values order by magnitude, not by their first limb.
    #[test]
    fn wide_integers_are_exact() {
        let big = u128::MAX;
        let p = Wide::product(big, big);
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1.
        assert_eq!(p.0, [1, 0, u64::MAX - 1, u64::MAX]);
        assert_eq!(Wide::product(6, 7).narrow(), Some(42));
        assert_eq!(Wide::of(big).plus(Wide::of(1)).map(|w| w.0), Some([0, 0, 1, 0]));
        assert!(Wide([u64::MAX; LIMBS]).plus(Wide::of(1)).is_none());
        assert_eq!(Wide([0, 0, 1, 0]).minus(Wide::of(1)).and_then(Wide::narrow), Some(big));
        assert!(Wide::of(1).minus(Wide::of(2)).is_none());
        let ten_74 = Wide::of(1).shifted(74).unwrap();
        assert_eq!(ten_74.digits(), 75);
        assert!(Wide::of(1).shifted(78).is_none(), "10^78 is past 2^256");
        let (q, r) = ten_74.plus(Wide::of(7)).unwrap().over(10);
        assert_eq!((q, r), (Wide::of(1).shifted(73).unwrap(), 7));
        assert_eq!((Wide::of(0).digits(), Wide::of(9).digits(), Wide::of(big).digits()), (0, 1, 39));
        assert!(Wide([0, 0, 1, 0]) > Wide([u64::MAX, u64::MAX, 0, 0]));
    }
}
