//! \[DESIGN\] The numeric value functions of a DECFLOAT operand
//! [ISO/IEC 9075-2:2023 §6.31 SR16a, SR17a, SR18a, SR20c]: a value
//! `c` × 10^`e` taken through its decimal exponent `e`, so one past the
//! range of a double is still found, as a double holds its digits `c`
//! and `e` alone. SQRT is found by Newton's steps in DECFLOAT arithmetic,
//! to its 34 digits; FLOOR and CEILING exactly.

use super::{Result, Value, double};
use crate::value::{DecFloat, Decimal, MAX_DECFLOAT_PRECISION, pow10};

/// The digits a double holds, to which LN, EXP and POWER of a DECFLOAT
/// are kept: more would be digits of its binary form, not of the result.
const DOUBLE_DIGITS: u32 = 16;
/// The greatest base-10 logarithm of a DECFLOAT value's magnitude, past
/// its greatest adjusted exponent, 6144 (IL054): a power whose logarithm
/// is above it is out of range.
const MOST_LOG10: f64 = 6145.0;
/// A base-10 logarithm below the least adjusted exponent, -6143, by
/// more than 34 digits: of a value that rounds to 0.
const LEAST_LOG10: f64 = -6178.0;
/// Newton's steps for a square root, from a first guess of a double's
/// 16 digits: each doubles the digits right, so 2 reach 34, and the rest
/// are spare; it stops once a step changes nothing.
const NEWTON_STEPS: usize = 6;
/// The greatest magnitude of a whole exponent `whole_power` raises a
/// base to by products: at most 2 × 17 of them, each rounded once.
const MOST_WHOLE_EXPONENT: i128 = 100_000;
/// The most digits an `i128` power of ten holds, 10^38.
const I128_DIGITS: u32 = 38;

/// Its coefficient, as a double.
fn coefficient(d: DecFloat) -> f64 {
    Decimal::new(d.coefficient(), 0).to_f64()
}

/// A double `x` as DECFLOAT, kept to a double's 16 digits.
pub(super) fn of_double(x: f64) -> Option<Value> {
    DecFloat::of_f64(x, DOUBLE_DIGITS).map(Value::DecFloat)
}

/// The natural logarithm of `d`, above 0: ln `c` + `e` ln 10.
pub(super) fn ln(d: DecFloat) -> f64 {
    coefficient(d).ln() + f64::from(d.exponent()) * std::f64::consts::LN_10
}

/// The base-10 logarithm of the magnitude of the number `v`, not 0.
pub(super) fn log10(v: &Value) -> Result<f64> {
    if let Value::DecFloat(d) = v { Ok(coefficient(*d).abs().log10() + f64::from(d.exponent())) } else { Ok(double(v)?.abs().log10()) }
}

/// 10 raised to `log10`, as DECFLOAT of a double's 16 digits: 10^`f` ×
/// 10^`k`, `k` the whole part, `f` the rest; 0 if it rounds to 0; none if
/// it is out of range.
pub(super) fn exp10(log10: f64) -> Option<Value> {
    if log10 < LEAST_LOG10 {
        return Some(Value::DecFloat(DecFloat::ZERO));
    }
    if log10.is_nan() || log10 > MOST_LOG10 {
        return None;
    }
    let k = log10.floor();
    let digits = DecFloat::of_f64(10_f64.powf(log10 - k), DOUBLE_DIGITS)?;
    let k = i64::try_from(DecFloat::of_f64(k, MAX_DECFLOAT_PRECISION)?.to_decimal(0)?.units).ok()?;
    let exponent = i64::from(digits.exponent()).checked_add(k)?;
    // Below the least exponent, it rounds to 0.
    DecFloat::new(digits.coefficient(), exponent, DOUBLE_DIGITS).or_else(|| (k < 0).then_some(DecFloat::ZERO)).map(Value::DecFloat)
}

/// The square root of `a`, not negative, to 34 digits: Newton's steps
/// x ← (x + a / x) / 2 from a double's root of its digits, `a` taken as
/// `m` × 10^(2`h`), `m` from 1 to 100, so the guess is `m`'s root ×
/// 10^`h`. None if `a` is negative.
pub fn square_root(a: DecFloat) -> Option<DecFloat> {
    if a.is_zero() {
        return Some(DecFloat::ZERO);
    }
    if a.coefficient() < 0 {
        return None;
    }
    let digits = a.coefficient().unsigned_abs().checked_ilog10()?;
    let adjusted = i64::from(a.exponent()).checked_add(i64::from(digits))?;
    let h = adjusted.div_euclid(2);
    let m = DecFloat::new(a.coefficient(), i64::from(a.exponent()).checked_sub(h.checked_mul(2)?)?, MAX_DECFLOAT_PRECISION)?;
    let guess = DecFloat::of_f64(m.to_f64().sqrt(), DOUBLE_DIGITS)?;
    let mut x = DecFloat::new(guess.coefficient(), i64::from(guess.exponent()).checked_add(h)?, MAX_DECFLOAT_PRECISION)?;
    let half = DecFloat::new(5, -1, MAX_DECFLOAT_PRECISION)?;
    for _ in 0..NEWTON_STEPS {
        let next = x.plus(a.over(x)?)?.times(half)?;
        if next == x {
            break;
        }
        x = next;
    }
    Some(x)
}

/// [§6.31 GR15b ii, GR16b ii] FLOOR of `d`, or CEILING if `up`: whole
/// numbers kept, others to the whole number below, or above.
pub(super) fn rounded(d: DecFloat, up: bool) -> Option<DecFloat> {
    let Ok(places) = u32::try_from(0_i64.checked_sub(i64::from(d.exponent()))?) else { return Some(d) };
    if places == 0 {
        return Some(d);
    }
    let c = d.coefficient();
    let whole = if places > I128_DIGITS {
        // Its 34 digits all lie below the point: it is between -1 and 1.
        match (c < 0, up) {
            (true, false) => -1,
            (false, true) => 1,
            (true, true) | (false, false) => 0,
        }
    } else {
        let unit = pow10(places)?;
        if up { c.checked_neg()?.checked_div_euclid(unit)?.checked_neg()? } else { c.checked_div_euclid(unit)? }
    };
    DecFloat::new(whole, 0, MAX_DECFLOAT_PRECISION)
}

/// The number `v` as DECFLOAT, if it is one it holds.
fn of_number(v: &Value) -> Option<DecFloat> {
    if let Value::DecFloat(d) = v {
        return Some(*d);
    }
    if let Value::Exact(d) = v {
        return DecFloat::of_decimal(d);
    }
    DecFloat::of_f64(v.as_f64()?, MAX_DECFLOAT_PRECISION)
}

/// `base` raised to `exponent`, if the exponent is a whole number of
/// magnitude at most `MOST_WHOLE_EXPONENT`: by squaring, each product of
/// 34 digits, the reciprocal of a negative exponent's; 22003 if out of
/// range. None if the exponent is not such.
pub(super) fn whole_power(base: &Value, exponent: &Value) -> Option<Result<DecFloat>> {
    let e = of_number(exponent)?;
    if e.exponent() < 0_i32 {
        return None;
    }
    let k = e.to_decimal(0_u32)?.units;
    if k.unsigned_abs() > MOST_WHOLE_EXPONENT.unsigned_abs() {
        return None;
    }
    let one = DecFloat::new(1, 0, MAX_DECFLOAT_PRECISION)?;
    let raised = || -> Option<DecFloat> {
        let (mut result, mut square, mut left) = (one, of_number(base)?, k.unsigned_abs());
        while left > 0_u128 {
            if left % 2_u128 == 1_u128 {
                result = result.times(square)?;
            }
            left /= 2_u128;
            if left > 0_u128 {
                square = square.times(square)?;
            }
        }
        if k < 0 { one.over(result) } else { Some(result) }
    };
    Some(raised().map_or_else(|| super::out_of_range("§6.31 GR14h"), Ok))
}
