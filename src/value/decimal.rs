//! Exact numeric values [X3.135-1986 §4.2.2]: integer significant digits times
//! 10^-scale, and their arithmetic [X3.135-1986 §5.9]. Split from value.rs
//! (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::fmt;

use super::MAX_EXACT_PRECISION;
use crate::error::{Code, Result, SqlState, fail_with};

/// An exact number: `units` × 10^-`scale`.
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    /// The significant digits, as an integer.
    pub units: i128,
    /// How many of them are after the decimal point.
    pub scale: u32,
}

/// [X3.135-1986 §4.2.2] Exact numbers are decimal: digits times a power of ten.
pub const RADIX: u32 = 10;
/// `RADIX` as the type of `Decimal::units`.
const RADIX_I128: i128 = 10;

/// 10^`n`, if it fits an `i128`.
pub fn pow10(n: u32) -> Option<i128> {
    RADIX_I128.checked_pow(n)
}

fn too_big<T>() -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
        format!("[X3.135-1986 §5.9 GR6] the exact numeric result needs more than the implementor-defined {MAX_EXACT_PRECISION} digits"),
    )
}

impl Decimal {
    /// `units` × 10^-`scale`.
    pub fn new(units: i128, scale: u32) -> Decimal {
        Decimal { units, scale }
    }

    /// Number of significant decimal digits in the value.
    pub fn digits(&self) -> u32 {
        // An i128 has at most 39 digits.
        u32::try_from(self.units.unsigned_abs().to_string().len()).unwrap_or(u32::MAX)
    }

    pub(super) fn fits(&self, precision: u32, scale: u32) -> bool {
        self.scale <= scale && self.units.unsigned_abs() < u128::from(RADIX).pow(precision)
    }

    /// [X3.135-1986 §5.9 GR6a] a result "shall be exactly representable" in our precision.
    fn checked(self) -> Result<Decimal> {
        if self.fits(MAX_EXACT_PRECISION, MAX_EXACT_PRECISION) { Ok(self) } else { too_big() }
    }

    /// The units of this value written at a larger scale.
    fn at_scale(&self, scale: u32) -> Option<i128> {
        self.units.checked_mul(pow10(scale.checked_sub(self.scale)?)?)
    }

    /// [X3.135-1986 §5.9 SR4b] scale of a sum or difference is max(s1, s2).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if the sum needs more than `MAX_EXACT_PRECISION` digits.
    pub fn checked_add(self, o: Decimal) -> Result<Decimal> {
        let s = self.scale.max(o.scale);
        match (self.at_scale(s), o.at_scale(s)) {
            (Some(a), Some(b)) => a.checked_add(b).map_or_else(too_big, |sum| Decimal::new(sum, s).checked()),
            _ => too_big(),
        }
    }

    /// The difference, with the scale of `checked_add`.
    ///
    /// # Errors
    ///
    /// As for `checked_add`.
    pub fn checked_sub(self, o: Decimal) -> Result<Decimal> {
        self.checked_add(o.negated())
    }

    /// [X3.135-1986 §5.9 SR4c] scale of a product is s1 + s2.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if the product needs more than `MAX_EXACT_PRECISION` digits.
    pub fn checked_mul(self, o: Decimal) -> Result<Decimal> {
        match (self.units.checked_mul(o.units), self.scale.checked_add(o.scale)) {
            (Some(u), Some(scale)) => Decimal::new(u, scale).checked(),
            _ => too_big(),
        }
    }

    /// [X3.135-1986 §5.9 SR4d] precision and scale of a quotient are
    /// implementor-defined; [X3.135-1986 §5.9 GR6b] it "shall not lose any
    /// leading significant digits". \[DESIGN\] We give it the largest scale,
    /// up to `MAX_EXACT_PRECISION`, at which it fits in `MAX_EXACT_PRECISION`
    /// digits, truncate toward zero, and drop trailing fractional zeros down
    /// to max(s1, s2): 1 / 3 is 0.333333333333333333, 6 / 2 is 3, 1 / 4 is
    /// 0.25. The scale so depends on the value; the static type's scale,
    /// max(s1, s2), is the least (`binder::exact_result_type`).
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
        let found = (0..=MAX_EXACT_PRECISION)
            .rev()
            .find_map(|s| self.units_over(o, s).map(|q| Decimal::new(q, s)).filter(|d| d.fits(MAX_EXACT_PRECISION, MAX_EXACT_PRECISION)));
        let Some(mut q) = found else { return too_big() };
        if q.units == 0 && self.units != 0 {
            return fail_with(
                Code::Data,
                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                format!("[X3.135-1986 §5.9 GR6b] the quotient is below 10^-{MAX_EXACT_PRECISION}, so it would lose its leading digits"),
            );
        }
        while q.scale > floor && q.units % RADIX_I128 == 0 {
            q = Decimal::new(q.units.checked_div(RADIX_I128).unwrap_or_default(), q.scale.saturating_sub(1));
        }
        Ok(q)
    }

    /// The units of this value divided by `o` at scale `s`, truncated toward
    /// zero, if they fit an `i128`: (a / 10^s1) / (b / 10^s2) × 10^s, that is
    /// a × 10^(s + s2 - s1) / b.
    fn units_over(self, o: Decimal, s: u32) -> Option<i128> {
        let up = s.checked_add(o.scale)?;
        if up >= self.scale {
            self.units.checked_mul(pow10(up.checked_sub(self.scale)?)?)?.checked_div(o.units)
        } else {
            self.units.checked_div(o.units.checked_mul(pow10(self.scale.checked_sub(up)?)?)?)
        }
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
        match self.units_over(o, scale) {
            Some(q) => Decimal::new(q, scale).checked(),
            None => too_big(),
        }
    }

    /// Compare by algebraic value [X3.135-1986 §5.11 GR4].
    ///
    /// Values within `MAX_EXACT_PRECISION` digits and scale always rescale
    /// exactly. A value built outside those limits (the fields are public) is
    /// compared as `f64` rather than panicking.
    pub fn cmp_value(&self, o: &Decimal) -> Ordering {
        let s = self.scale.max(o.scale);
        match (self.at_scale(s), o.at_scale(s)) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => self.to_f64().total_cmp(&o.to_f64()),
        }
    }

    /// The nearest `f64`, near enough for approximate arithmetic; comparison
    /// with an approximate number goes through `value/comparison.rs`, which
    /// is exact.
    pub fn to_f64(&self) -> f64 {
        nearest_f64(self.units) / f64::from(RADIX).powi(i32::try_from(self.scale).unwrap_or(i32::MAX))
    }

    /// The nearest `f64`, correctly rounded (Rust's casts and parsing
    /// round to nearest), so two equal decimals have the same one and the
    /// conversion is monotonic; NaN only if parsing failed, which it cannot.
    pub(super) fn nearest(&self) -> f64 {
        if self.scale == 0 {
            return nearest_f64(self.units);
        }
        format!("{}e-{}", self.units, self.scale).parse().unwrap_or(f64::NAN)
    }

    /// Strip trailing fractional zeros so that 1.50 and 1.5 look the same: equal
    /// numbers compare by algebraic value [X3.135-1986 §5.11 GR4].
    pub(crate) fn normalized(&self) -> (i128, u32) {
        let (mut u, mut s) = (self.units, self.scale);
        while s > 0 && u % RADIX_I128 == 0 {
            u /= RADIX_I128;
            s = s.saturating_sub(1);
        }
        (u, s)
    }
}

impl Decimal {
    /// The value with its sign reversed [X3.135-1986 §5.9 GR4]. Units stay
    /// within `MAX_EXACT_PRECISION` digits, so this never saturates.
    #[must_use]
    pub fn negated(self) -> Decimal {
        Decimal::new(0_i128.saturating_sub(self.units), self.scale)
    }
}

/// The `f64` nearest to `n`.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "RULES.md R-DEV-3: the nearest f64 is the intent, and std has no i128-to-f64 conversion function"
)]
fn nearest_f64(n: i128) -> f64 {
    n as f64
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.units < 0 { "-" } else { "" };
        let digits = self.units.unsigned_abs().to_string();
        if self.scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        // [DESIGN] No data type has a larger scale, so only a decimal built
        // directly (the fields are public) gets here. Padding it with zeros
        // could need gigabytes, so the power of ten is written out instead.
        if self.scale > MAX_EXACT_PRECISION {
            return write!(f, "{sign}{digits}E-{}", self.scale);
        }
        let s = usize::try_from(self.scale).unwrap_or(usize::MAX);
        let padded = format!("{:0>width$}", digits, width = s.saturating_add(1));
        let (int, frac) = padded.split_at(padded.len().saturating_sub(s));
        write!(f, "{sign}{int}.{frac}")
    }
}

// Unit tests: scale, digits and overflow of exact arithmetic, with the MC/DC
// cases written out (RULES.md R-TEST-1, R-COV-3).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.9 GR6a] `fits` admits a value only if both its scale and its digits fit.
    ///
    /// Decision `scale_ok && digits_ok`. MC/DC pairs: (TT→T, FT→F) shows `scale_ok`;
    /// (TT→T, TF→F) shows `digits_ok`.
    #[test]
    fn fits_needs_scale_and_digits() {
        let d = Decimal::new(999, 2); // 9.99: three digits, scale 2
        assert!(d.fits(3, 2)); // T T
        assert!(!d.fits(3, 1)); // F T: scale 2 > 1
        assert!(!d.fits(2, 2)); // T F: 999 needs 3 digits
    }

    /// Verifies: [DESIGN] `normalized` strips fractional trailing zeros and nothing else.
    ///
    /// Loop decision `s > 0 && u % 10 == 0`. MC/DC pairs: (TT, FT) shows `s > 0`;
    /// (TT, TF) shows `u % 10 == 0`.
    #[test]
    fn normalized_strips_only_fractional_zeros() {
        assert_eq!(Decimal::new(150, 2).normalized(), (15, 1)); // T T, then T F
        assert_eq!(Decimal::new(10, 0).normalized(), (10, 0)); // F T: integer zeros stay
        assert_eq!(Decimal::new(15, 1).normalized(), (15, 1)); // T F
    }

    /// Verifies: [X3.135-1986 §5.9 SR4b, GR6a] sums take scale max(s1, s2) and must fit
    /// `MAX_EXACT_PRECISION` digits (normal range and robustness).
    #[test]
    fn checked_add_scale_and_overflow() {
        let sum = Decimal::new(105, 1).checked_add(Decimal::new(1, 2)).map(|d| (d.units, d.scale));
        assert_eq!(sum.ok(), Some((1051, 2)));
        let max = Decimal::new(pow10(MAX_EXACT_PRECISION).unwrap() - 1, 0);
        assert_eq!(max.checked_add(Decimal::new(1, 0)).unwrap_err().code, Code::Data);
    }

    /// Verifies: [DESIGN] arithmetic on decimals built outside the limits (the
    /// fields are public) is a data exception, never an overflow panic.
    #[test]
    fn out_of_range_decimals_are_reported() {
        let huge = Decimal::new(i128::MAX, 0);
        let one = Decimal::new(1, 0);
        assert_eq!(huge.checked_add(one).unwrap_err().code, Code::Data);
        assert_eq!(huge.checked_add(Decimal::new(1, 1)).unwrap_err().code, Code::Data); // cannot rescale
        assert_eq!(Decimal::new(1, u32::MAX).checked_mul(Decimal::new(1, 1)).unwrap_err().code, Code::Data);
        assert_eq!(one.checked_div(Decimal::new(1, u32::MAX)).unwrap_err().code, Code::Data);
        assert_eq!(Decimal::new(i128::MIN, 0).checked_div(Decimal::new(-1, 0)).unwrap_err().code, Code::Data);
        assert_eq!(Decimal::new(-15, 40).to_string(), "-15E-40");
    }

    /// Verifies: [X3.135-1986 §5.11 GR4] comparison is by algebraic value, and
    /// [DESIGN] a decimal too large to rescale is still ordered, not a panic.
    #[test]
    fn cmp_value_rescales_or_falls_back() {
        assert_eq!(Decimal::new(150, 2).cmp_value(&Decimal::new(15, 1)), Ordering::Equal);
        assert_eq!(Decimal::new(i128::MAX, 0).cmp_value(&Decimal::new(1, 30)), Ordering::Greater);
    }

    /// Verifies: [X3.135-1986 §5.9 GR5] a zero divisor is a data exception, whatever its scale.
    #[test]
    fn checked_div_by_zero() {
        for zero in [Decimal::new(0, 0), Decimal::new(0, 3)] {
            assert_eq!(Decimal::new(1, 0).checked_div(zero).unwrap_err().code, Code::Data);
        }
    }

    /// Verifies: [X3.135-1986 §5.9 SR4d, GR6b] a quotient loses no leading
    /// significant digit: it has the most fractional digits that fit 18
    /// digits, truncated toward zero, trailing zeros dropped down to the
    /// operands' larger scale; an integer part past 18 digits, or a nonzero
    /// quotient below 10^-18, is refused with SQLCODE -301 rather than given
    /// wrong. At a fixed scale, as for AVG, it is truncated there.
    #[test]
    fn quotients_keep_their_leading_digits() {
        let q = |a: i128, sa: u32, b: i128, sb: u32| Decimal::new(a, sa).checked_div(Decimal::new(b, sb)).map(|d| d.to_string());
        assert_eq!(q(1, 0, 3, 0).unwrap(), "0.333333333333333333");
        assert_eq!(q(2, 0, 3, 0).unwrap(), "0.666666666666666666", "truncated, not rounded");
        assert_eq!(q(6, 0, 2, 0).unwrap(), "3");
        assert_eq!(q(1, 0, 4, 0).unwrap(), "0.25");
        assert_eq!(q(-7, 0, 2, 0).unwrap(), "-3.5");
        assert_eq!(q(1050, 2, 2, 0).unwrap(), "5.25");
        assert_eq!(q(600, 2, 2, 0).unwrap(), "3.00", "not below the operands' scale");
        assert_eq!(q(10, 0, 3, 0).unwrap(), "3.33333333333333333", "18 digits in all");
        assert_eq!(q(1, 18, 1_000, 0).unwrap_err().code, Code::Data, "below 10^-18");
        assert_eq!(q(999_999_999_999_999_999, 0, 1, 2).unwrap_err().code, Code::Data, "an integer part of 20 digits");
        assert_eq!(q(999_999_999_999_999_999, 0, 1, 0).unwrap(), "999999999999999999");
        let at = |a: i128, b: i128, scale: u32| Decimal::new(a, 0).quotient_at(Decimal::new(b, 0), scale).map(|d| d.to_string());
        assert_eq!(at(5, 3, 2).unwrap(), "1.66");
        assert_eq!(at(1, 0, 2).unwrap_err().code, Code::Data);
        assert_eq!(at(999_999_999_999_999_999, 1, 2).unwrap_err().code, Code::Data, "past 18 digits at that scale");
    }
}
