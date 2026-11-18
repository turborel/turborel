//! The SQL numeric functions the item methods `ceiling()`, `floor()` and
//! `abs()` and the operator `%` compute with [ISO/IEC 9075-2:2023 §9.46
//! GR11g ii 6 C IV to VI, GR11i iii 2 C]: CEILING, FLOOR, ABS and MOD, by
//! the General Rules of `<numeric value function>` [§6.31 GR8, GR9, GR15,
//! GR16]. \[DESIGN\] turborel's SQL has none of these functions yet, so they
//! are defined here, of the numbers an SQL/JSON item may be.

use super::Number;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::{DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Value, pow10};

/// Which way a number is taken to a whole number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toward {
    /// The greatest not above it, FLOOR's.
    Floor,
    /// The least not below it, CEILING's.
    Ceiling,
}

/// [§6.31 GR16] CEILING (`n`): the least whole number not below it, of
/// scale 0 if exact \[SR20\].
///
/// # Errors
///
/// SQLSTATE 22003 if no decimal floating-point number holds it.
pub(super) fn ceiling(n: Number) -> Result<Value> {
    whole(n, Toward::Ceiling)
}

/// [§6.31 GR15] FLOOR (`n`): the greatest whole number not above it, of
/// scale 0 if exact \[SR20\].
///
/// # Errors
///
/// SQLSTATE 22003 if no decimal floating-point number holds it.
pub(super) fn floor(n: Number) -> Result<Value> {
    whole(n, Toward::Floor)
}

/// CEILING or FLOOR of `n`, as `toward` says.
fn whole(n: Number, toward: Toward) -> Result<Value> {
    match n {
        Number::Exact(d) => Ok(Value::Exact(Decimal::new(rounded(d.units, i64::from(d.scale).saturating_neg(), toward), 0))),
        // Plus zero, so that one between -1 and 0 is 0, not -0: every
        // number of the result type is one, and 0 is the one a negative
        // number above -1 rounds up to [§6.31 GR16b i].
        Number::Approx(x) => Ok(Value::Approx(
            match toward {
                Toward::Floor => x.floor(),
                Toward::Ceiling => x.ceil(),
            } + 0.0_f64,
        )),
        Number::DecFloat(d) => {
            // One of no negative exponent is whole.
            if d.exponent() >= 0 {
                return Ok(Value::DecFloat(d));
            }
            let q = rounded(d.coefficient(), i64::from(d.exponent()), toward);
            DecFloat::new(q, 0, MAX_DECFLOAT_PRECISION).map(Value::DecFloat).ok_or_else(out_of_range)
        }
    }
}

/// `c` × 10^`e`, of `e` not above 0, taken to a whole number `toward`.
fn rounded(c: i128, e: i64, toward: Toward) -> i128 {
    // Below 1 in magnitude if no i128 holds the power of ten.
    let (q, r) = u32::try_from(e.saturating_neg())
        .ok()
        .and_then(pow10)
        .map_or((0, c), |p| (c.checked_div(p).unwrap_or(0), c.checked_rem(p).unwrap_or(0)));
    match toward {
        Toward::Floor => q.saturating_sub(i128::from(r < 0)),
        Toward::Ceiling => q.saturating_add(i128::from(r > 0)),
    }
}

/// [§6.31 GR8] ABS (`n`): `n` if not negative, else −1 × `n`, which an
/// exact number of at most 38 digits always holds.
pub(super) fn abs(n: Number) -> Value {
    match n {
        Number::Exact(d) => Value::Exact(Decimal::new(d.units.saturating_abs(), d.scale)),
        Number::Approx(x) => Value::Approx(x.abs()),
        Number::DecFloat(d) => Value::DecFloat(if d.coefficient() < 0 { d.negated() } else { d }),
    }
}

/// [§6.31 GR9] MOD (`n`, `m`): the whole number of the sign of `n`, of
/// magnitude below `m`'s, that `n` is above a whole multiple of `m`;
/// decimal floating-point if either is, else exact of scale 0 \[SR12\].
/// \[SR12\] Each is to be exact of scale 0 or decimal floating-point;
/// \[DESIGN\] of an SQL/JSON item, whose type is its value's, an exact or
/// decimal floating-point number that is whole is taken, of any scale, so
/// `4.0 % 3` is 1. An approximate number, `5e0` too, is of no type SR12
/// allows, and neither is a number that is not whole. A Syntax Rule
/// broken in the General Rules of a path is no Syntax Rule of the
/// statement's, and the calculation of §9.46 GR11i iii 2 C cannot be
/// made, so it is an exception condition, as the path's others are: the
/// standard names no subclass for it, so it is data exception with no
/// subclass, SQLSTATE 22000.
///
/// # Errors
///
/// SQLSTATE 22000 if either is approximate, not whole, or past every
/// exact number of scale 0 \[SR12\]; 22012 if `m` is zero \[GR9b\].
pub(super) fn modulo(n: Number, m: Number) -> Result<Value> {
    let (Some(dividend), Some(divisor)) = (integer(n), integer(m)) else {
        return Err(SqlError::new(
            Code::Data,
            "[ISO/IEC 9075-2:2023 §6.31 SR12] an operand of MOD is not a whole exact or decimal floating-point number of at most 38 digits",
        )
        .with_state(SqlState::DATA_EXCEPTION));
    };
    if divisor == 0 {
        return Err(
            SqlError::new(Code::Data, "[ISO/IEC 9075-2:2023 §6.31 GR9b] the divisor of MOD is zero").with_state(SqlState::DIVISION_BY_ZERO)
        );
    }
    // Of the sign of the dividend [GR9c i], as Rust's remainder is; `MIN % -1`,
    // the one remainder that overflows, is 0.
    let remainder = dividend.checked_rem(divisor).unwrap_or(0);
    if matches!(n, Number::DecFloat(_)) || matches!(m, Number::DecFloat(_)) {
        return DecFloat::new(remainder, 0, MAX_DECFLOAT_PRECISION).map(Value::DecFloat).ok_or_else(out_of_range);
    }
    Ok(Value::Exact(Decimal::new(remainder, 0)))
}

/// The whole number `n` is, if it is exact or decimal floating-point,
/// whole, and an `i128` holds it.
fn integer(n: Number) -> Option<i128> {
    let (c, e) = match n {
        Number::Exact(d) => (d.units, i64::from(d.scale).saturating_neg()),
        Number::DecFloat(d) => (d.coefficient(), i64::from(d.exponent())),
        Number::Approx(_) => return None,
    };
    if let Ok(up) = u32::try_from(e) {
        return c.checked_mul(pow10(up)?);
    }
    let p = pow10(u32::try_from(e.saturating_neg()).ok()?)?;
    (c.checked_rem(p)? == 0).then(|| c.checked_div(p)).flatten()
}

/// The error of a result no decimal floating-point number holds [§6.31
/// GR15b ii, GR16b ii].
fn out_of_range() -> SqlError {
    SqlError::new(Code::Data, "[ISO/IEC 9075-2:2023 §6.31 GR16b ii] the result is not representable by its declared type")
        .with_state(SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
}

#[cfg(test)]
mod tests;
