//! Quotients of exact numbers [X3.135-1986 §5.9 SR4d, GR5, GR6b;
//! ISO/IEC 9075-2:2023 §6.30 SR1c iv, GR5b] (RULES.md D697): each at a
//! scale its operands give, `MIN_QUOTIENT_SCALE` at least, so a sum of
//! quotients keeps room for its integer digits; computed once by long
//! division of the units' magnitudes, so no product on the way overflows
//! 128 bits.

use super::{Decimal, too_big};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::MAX_EXACT_PRECISION;

/// `RADIX` as the type of a magnitude.
const RADIX_U128: u128 = 10;

/// \[DESIGN\] (RULES.md D697) The fewest fractional digits a quotient is
/// computed to, as 1 / 3 is 0.333333, unless its integer digits leave
/// fewer of `MAX_EXACT_PRECISION`.
pub const MIN_QUOTIENT_SCALE: u32 = 6;

/// The fewest trailing zeros a quotient's units are stripped of at once,
/// doubled down to one: so 38 of them take a few divisions, not 38.
const STRIP_STEPS: [u32; 5] = [16, 8, 4, 2, 1];

/// 10^`n`, if it fits a `u128`.
fn pow10(n: u32) -> Option<u128> {
    RADIX_U128.checked_pow(n)
}

/// The power of ten the dividend's units are scaled by for a quotient
/// of `a` by `b` at `scale`: scale + s2 - s1.
fn scale_shift(a: Decimal, b: Decimal, scale: u32) -> Option<i64> {
    i64::from(scale).checked_add(i64::from(b.scale))?.checked_sub(i64::from(a.scale))
}

/// The decimal digits of `n`, 0 of 0.
fn digits_of(n: u128) -> u32 {
    n.checked_ilog10().map_or(0, |l| l.saturating_add(1))
}

/// ⌊`dividend` × 10^`shift` / `divisor`⌋ for a divisor not 0, if a
/// `u128` holds it. Each step appends as many digits as a remainder below
/// the divisor times a power of ten can take without overflow; of a
/// divisor past a tenth of the range, one digit, by ten additions each
/// less than twice the divisor.
fn shifted_over(dividend: u128, shift: i64, divisor: u128) -> Option<u128> {
    if shift < 0 {
        let by = u32::try_from(shift.unsigned_abs()).ok().and_then(pow10).and_then(|p| divisor.checked_mul(p));
        // A divisor past 128 bits is above every dividend.
        return Some(by.map_or(0, |by| dividend.checked_div(by).unwrap_or(0)));
    }
    let mut rest = u32::try_from(shift).ok()?;
    let (mut quotient, mut remainder) = (dividend.checked_div(divisor)?, dividend.checked_rem(divisor)?);
    while rest > 0 {
        let room = u128::MAX.checked_div(divisor)?.checked_ilog10().unwrap_or(0).min(rest);
        let (digits, (more, left)) =
            if room == 0 { (1, ten_over(remainder, divisor)?) } else { (room, appended(remainder, room, divisor)?) };
        quotient = quotient.checked_mul(pow10(digits)?)?.checked_add(more)?;
        remainder = left;
        rest = rest.saturating_sub(digits);
    }
    Some(quotient)
}

/// The quotient and remainder of `r` × 10^`k` by `b`, which a `u128`
/// holds as `k` is within `b`'s room.
fn appended(r: u128, k: u32, b: u128) -> Option<(u128, u128)> {
    let t = r.checked_mul(pow10(k)?)?;
    Some((t.checked_div(b)?, t.checked_rem(b)?))
}

/// The quotient and remainder of 10 × `r` by `b`, `r` below `b`: the
/// remainder taken back below `b` after each of ten additions.
fn ten_over(r: u128, b: u128) -> Option<(u128, u128)> {
    let (mut digit, mut left) = (0_u128, 0_u128);
    for _ in 0..RADIX_U128 {
        left = left.checked_add(r)?;
        if left >= b {
            left = left.saturating_sub(b);
            digit = digit.saturating_add(1);
        }
    }
    Some((digit, left))
}

/// \[DESIGN\] (RULES.md D697) The precision and scale of a quotient of
/// exact values within `(p1, s1)` by ones within `(p2, s2)`: its integer
/// digits I = p1 - s1 + s2, the most a divisor of at least 10^-s2 gives;
/// its scale max(`MIN_QUOTIENT_SCALE`, s1 + p2 + 1), at most the
/// `MAX_EXACT_PRECISION` - I left; its precision I and the scale, at most
/// `MAX_EXACT_PRECISION`.
pub fn quotient_bound((p1, s1): (u32, u32), (p2, s2): (u32, u32)) -> (u32, u32) {
    let whole = p1.saturating_sub(s1).saturating_add(s2);
    let scale = MIN_QUOTIENT_SCALE.max(s1.saturating_add(p2).saturating_add(1)).min(MAX_EXACT_PRECISION.saturating_sub(whole));
    (whole.saturating_add(scale).clamp(1, MAX_EXACT_PRECISION), scale)
}

impl Decimal {
    /// [X3.135-1986 §5.9 SR4d] precision and scale of a quotient are
    /// implementor-defined; [X3.135-1986 §5.9 GR6b] it "shall not lose any
    /// leading significant digits". \[DESIGN\] (RULES.md D697) As
    /// `quotient_bound` of the values: at scale max(`MIN_QUOTIENT_SCALE`,
    /// s1 + d2 + 1, s2), d2 the divisor's digits, or less as its integer
    /// digits need, truncated toward zero, then its trailing fractional
    /// zeros dropped down to max(s1, s2): 1 / 4 is 0.25, 6 / 2 is 3, and
    /// 1 / 3 is 0.333333. Its leading digit is never lost: the scale is
    /// past the divisor's digits and the dividend's scale.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for a zero divisor, or a quotient whose integer part
    /// needs more than `MAX_EXACT_PRECISION` digits.
    pub fn checked_div(self, o: Decimal) -> Result<Decimal> {
        if o.units == 0 {
            return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[X3.135-1986 §5.9 GR5] a divisor shall not be 0");
        }
        let floor = self.scale.max(o.scale);
        let (dividend, divisor) = (self.units.unsigned_abs(), o.units.unsigned_abs());
        let wanted =
            MIN_QUOTIENT_SCALE.max(self.scale.saturating_add(digits_of(divisor)).saturating_add(1)).max(o.scale).min(MAX_EXACT_PRECISION);
        let Some(q) = self.over_at(o, wanted).or_else(|| {
            // Too many integer digits at that scale: as many fewer
            // fractional ones as it has past the limit.
            let at = shifted_over(dividend, scale_shift(self, o, 0)?, divisor)?;
            let over = digits_of(at).saturating_add(wanted).checked_sub(MAX_EXACT_PRECISION)?;
            self.over_at(o, wanted.checked_sub(over)?)
        }) else {
            return too_big();
        };
        Ok(q.stripped(floor))
    }

    /// This value divided by `o`, not 0, at scale `scale`, truncated, if it
    /// fits `MAX_EXACT_PRECISION` digits there.
    fn over_at(self, o: Decimal, scale: u32) -> Option<Decimal> {
        let (dividend, divisor) = (self.units.unsigned_abs(), o.units.unsigned_abs());
        let units = shifted_over(dividend, scale_shift(self, o, scale)?, divisor).filter(|u| digits_of(*u) <= MAX_EXACT_PRECISION)?;
        let units = i128::try_from(units).ok()?;
        let negative = (self.units < 0) != (o.units < 0);
        Some(Decimal::new(if negative { units.checked_neg()? } else { units }, scale))
    }

    /// This value with its trailing fractional zeros dropped, down to
    /// scale `floor`.
    fn stripped(self, floor: u32) -> Decimal {
        let mut d = self;
        for step in STRIP_STEPS {
            let Some(p) = super::pow10(step) else { continue };
            while d.scale >= floor.saturating_add(step) && d.units.checked_rem(p) == Some(0) {
                d = Decimal::new(d.units.checked_div(p).unwrap_or_default(), d.scale.saturating_sub(step));
            }
        }
        d
    }

    /// This value divided by `o` at scale `scale`, truncated toward zero:
    /// for a quotient whose scale is fixed, as AVG's
    /// (`functions::AVG_EXTRA_SCALE`).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for a zero divisor, or a quotient that does not fit
    /// `MAX_EXACT_PRECISION` digits at that scale.
    pub fn quotient_at(self, o: Decimal, scale: u32) -> Result<Decimal> {
        if o.units == 0 {
            return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[X3.135-1986 §5.9 GR5] a divisor shall not be 0");
        }
        let shift = i64::from(scale).saturating_add(i64::from(o.scale)).saturating_sub(i64::from(self.scale));
        let q = shifted_over(self.units.unsigned_abs(), shift, o.units.unsigned_abs()).and_then(|q| i128::try_from(q).ok());
        let negative = (self.units < 0) != (o.units < 0);
        match q {
            Some(q) => Decimal::new(if negative { q.saturating_neg() } else { q }, scale).checked(),
            None => too_big(),
        }
    }
}

// Unit tests: the quotient's scale, truncation and bounds at 38 digits,
// and the long division's steps (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
