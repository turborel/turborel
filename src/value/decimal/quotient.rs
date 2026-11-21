//! Quotients of exact numbers [X3.135-1986 §5.9 SR4d, GR5, GR6b;
//! ISO/IEC 9075-2:2023 §6.30 SR1c iv, GR5b] (RULES.md D697): each
//! computed once, at the scale its digits allow, by long division of the
//! units' magnitudes, so no product on the way overflows 128 bits.

use super::{Decimal, too_big};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::MAX_EXACT_PRECISION;

/// `RADIX` as the type of a magnitude.
const RADIX_U128: u128 = 10;

/// The fewest trailing zeros a quotient's units are stripped of at once,
/// doubled down to one: so 38 of them take a few divisions, not 38.
const STRIP_STEPS: [u32; 5] = [16, 8, 4, 2, 1];

/// 10^`n`, if it fits a `u128`.
fn pow10(n: u32) -> Option<u128> {
    RADIX_U128.checked_pow(n)
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

impl Decimal {
    /// [X3.135-1986 §5.9 SR4d] precision and scale of a quotient are
    /// implementor-defined; [X3.135-1986 §5.9 GR6b] it "shall not lose any
    /// leading significant digits". \[DESIGN\] We give it the largest scale,
    /// up to `MAX_EXACT_PRECISION`, at which it fits in `MAX_EXACT_PRECISION`
    /// digits, truncate toward zero, and drop trailing fractional zeros down
    /// to max(s1, s2): 1 / 4 is 0.25, 6 / 2 is 3, and 1 / 3 is 0.3 and 37
    /// more threes. The scale so depends on the value; the static type's
    /// scale, max(s1, s2), is the least (`binder::exact_result_type`).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for a zero divisor, a quotient whose integer part needs
    /// more than `MAX_EXACT_PRECISION` digits, or a nonzero quotient smaller
    /// than 10^-`MAX_EXACT_PRECISION`, either of which would lose leading
    /// digits.
    pub fn checked_div(self, o: Decimal) -> Result<Decimal> {
        if o.units == 0 {
            return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[X3.135-1986 §5.9 GR5] a divisor shall not be 0");
        }
        let floor = self.scale.max(o.scale);
        if self.units == 0 {
            return Ok(Decimal::new(0, floor));
        }
        let Some(q) = self.widest_over(o) else { return too_big() };
        if q.units == 0 {
            return fail_with(
                Code::Data,
                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                format!("[X3.135-1986 §5.9 GR6b] the quotient is below 10^-{MAX_EXACT_PRECISION}, so it would lose its leading digits"),
            );
        }
        Ok(q.stripped(floor))
    }

    /// This value, not 0, divided by `o` at the largest scale at most
    /// `MAX_EXACT_PRECISION` at which it fits that many digits: none if it
    /// fits at none. Of `n` digits over `m`, at scale `s` the quotient's
    /// magnitude is below 10^(n - m + 1 + e), e = s + s2 - s1, and at least
    /// 10^(n - m - 1 + e), so it fits at e = MAX - n + m or one less.
    fn widest_over(self, o: Decimal) -> Option<Decimal> {
        let (dividend, divisor) = (self.units.unsigned_abs(), o.units.unsigned_abs());
        let max = i64::from(MAX_EXACT_PRECISION);
        let top = max
            .checked_sub(i64::from(digits_of(dividend)))?
            .checked_add(i64::from(digits_of(divisor)))?
            .checked_add(i64::from(self.scale))?
            .checked_sub(i64::from(o.scale))?;
        let limit = pow10(MAX_EXACT_PRECISION)?;
        let fitting = |at: i64| {
            let shift = at.checked_add(i64::from(o.scale))?.checked_sub(i64::from(self.scale))?;
            shifted_over(dividend, shift, divisor).filter(|units| *units < limit)
        };
        let at = top.min(max);
        // At the estimated scale it may need a digit more than fit, or than
        // 128 bits hold: then one scale less.
        let (units, at) = match fitting(at) {
            Some(units) => (units, at),
            None => (fitting(at.checked_sub(1)?)?, at.checked_sub(1)?),
        };
        let scale = u32::try_from(at).ok()?;
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
