//! Exact numeric values [X3.135-1986 §4.2.2]: integer significant digits times
//! 10^-scale, and their arithmetic [X3.135-1986 §5.9].

use std::cmp::Ordering;
use std::fmt;

use super::{ArithOp, MAX_EXACT_PRECISION};
use crate::error::{Code, Result, SqlState, fail_with};

mod quotient;

pub use quotient::{MIN_QUOTIENT_SCALE, quotient_bound};

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

/// [X3.135-1986 §5.9 SR4b, SR4c] The precision and scale every value of
/// `op` of exact values within `(p1, s1)` and `(p2, s2)` is within, its
/// precision not capped: of a sum or difference, a digit before the point
/// more than the wider operand has, at the larger scale; of a product, the
/// digits of both, at the sum of the scales. None of a quotient, whose
/// scale is each value's own (`Decimal::checked_div`).
pub fn bound_of(op: ArithOp, (p1, s1): (u32, u32), (p2, s2): (u32, u32)) -> Option<(u32, u32)> {
    let (w1, w2) = (p1.saturating_sub(s1), p2.saturating_sub(s2));
    let (whole, scale) = match op {
        ArithOp::Add | ArithOp::Sub => (w1.max(w2).saturating_add(1), s1.max(s2)),
        ArithOp::Mul => (w1.saturating_add(w2), s1.saturating_add(s2)),
        ArithOp::Div => return None,
    };
    Some((whole.saturating_add(scale), scale))
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
        self.units.unsigned_abs().checked_ilog10().map_or(1, |l| l.saturating_add(1))
    }

    pub(super) fn fits(&self, precision: u32, scale: u32) -> bool {
        self.scale <= scale && self.units.unsigned_abs() < u128::from(RADIX).pow(precision)
    }

    /// [X3.135-1986 §5.9 GR6a] a result "shall be exactly representable" in our precision.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it needs more than `MAX_EXACT_PRECISION` digits.
    pub fn checked(self) -> Result<Decimal> {
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

    /// \[DESIGN\] The sum at the scale of `checked_add`, not judged
    /// against `MAX_EXACT_PRECISION` but held in the 128 bits of `units`:
    /// a set function's total so far, of which [ISO/IEC 9075:1992 §6.5
    /// GR2b iv] judges only the last (`checked`). None if 128 bits cannot
    /// hold it.
    pub fn wide_add(self, o: Decimal) -> Option<Decimal> {
        let s = self.scale.max(o.scale);
        Some(Decimal::new(self.at_scale(s)?.checked_add(o.at_scale(s)?)?, s))
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

    /// Compare by algebraic value [X3.135-1986 §5.11 GR4].
    ///
    /// Values within `MAX_EXACT_PRECISION` digits and scale always rescale
    /// exactly. A value built outside those limits (the fields are public) is
    /// compared as `f64` rather than panicking.
    pub fn cmp_value(&self, o: &Decimal) -> Ordering {
        // Of one scale, as two values of a column are, the units compare.
        if self.scale == o.scale {
            return self.units.cmp(&o.units);
        }
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
}
