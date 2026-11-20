//! The `<modulus expression>` MOD of Feature T441 [ISO/IEC 9075-2:2023
//! §6.31 SR12, GR9]: the remainder R of N divided by M, of N's sign and
//! less than M in magnitude, N = M × K + R for a whole K.
//!
//! Of exact operands, of scale 0 \[SR12\], R is found exactly. Of a
//! DECFLOAT operand it is found exactly too, of the operands' digits at
//! the lesser of their exponents. A dividend's exponent past the
//! divisor's is taken by modular arithmetic, so 1E100 is divided as
//! itself. Of DECFLOAT operands that are not whole numbers, which GR9c
//! gives no R of scale 0, R is the same remainder, of the smaller
//! exponent (RULES.md D332; CONFORMANCE.md, Readings of ambiguous text).

use std::cmp::Ordering;

use super::{out_of_range, sign};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::value::{DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Value, pow10};

/// [§6.31] MOD, no key word of SQL-92.
pub const MOD: &str = "MOD";

/// The radix a DECFLOAT's coefficient is scaled by.
const RADIX: u128 = 10;

/// [§6.31 GR9b] SQLSTATE 22012.
fn by_zero<T>() -> Result<T> {
    fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[ISO/IEC 9075-2:2023 §6.31 GR9b] the divisor of MOD shall not be 0")
}

/// [§6.31 GR9] MOD ( `n`, `m` ), neither null: 22012 if `m` is 0
/// \[GR9b\]; else the remainder \[GR9c\], DECFLOAT if either is.
pub(super) fn remainder(n: &Value, m: &Value) -> Result<Value> {
    if sign(m)? == Ordering::Equal {
        return by_zero();
    }
    if let (Value::Exact(a), Value::Exact(b)) = (n, m) {
        return exact(*a, *b);
    }
    let (Some(a), Some(b)) = (decfloat(n), decfloat(m)) else {
        return fail(Code::Api, "[DESIGN] MOD is of exact numbers of scale 0 or decimal floating-point numbers");
    };
    decimal(a, b).map(Value::DecFloat).map_or_else(|| out_of_range("§6.31 GR9c"), Ok)
}

/// The number `v` as DECFLOAT: an exact number or a DECFLOAT.
fn decfloat(v: &Value) -> Option<DecFloat> {
    if let Value::DecFloat(d) = v {
        return Some(*d);
    }
    if let Value::Exact(d) = v { DecFloat::of_decimal(d) } else { None }
}

/// [§6.31 GR9c] The remainder of exact numbers of scale 0 \[SR12\]: of
/// the dividend's sign, as a truncated quotient leaves it. Of a divisor
/// of magnitude 1 it is 0, though the quotient of the least `i128` by −1
/// would not fit.
fn exact(n: Decimal, m: Decimal) -> Result<Value> {
    if n.scale != 0 || m.scale != 0 {
        return fail(Code::Api, "[DESIGN] the operands of MOD are of scale 0");
    }
    let r = if m.units.unsigned_abs() == 1 { 0 } else { n.units.checked_rem(m.units).unwrap_or(0) };
    Ok(Value::Exact(Decimal::new(r, 0)))
}

/// [§6.31 GR9c] The remainder of `n` by `m`, not 0, as DECFLOAT values
/// `c` × 10^`e`, of their digits at the lesser exponent, of `n`'s sign.
fn decimal(n: DecFloat, m: DecFloat) -> Option<DecFloat> {
    let (en, em) = (i64::from(n.exponent()), i64::from(m.exponent()));
    let divisor = m.coefficient().unsigned_abs();
    let magnitude = if en >= em {
        // N's digits × 10^(en − em) mod M's digits, at M's exponent.
        let shift = u32::try_from(en.checked_sub(em)?).ok()?;
        times_mod(n.coefficient().unsigned_abs().checked_rem(divisor)?, power_mod(shift, divisor), divisor)
    } else {
        // M's digits at N's exponent. Past 128 bits M is greater than N,
        // which is then the remainder.
        let shift = u32::try_from(em.checked_sub(en)?).ok()?;
        let scaled = pow10(shift).and_then(|p| u128::try_from(p).ok()).and_then(|p| divisor.checked_mul(p));
        let digits = n.coefficient().unsigned_abs();
        scaled.map_or(Some(digits), |s| digits.checked_rem(s))?
    };
    let r = i128::try_from(magnitude).ok()?;
    DecFloat::new(if n.coefficient() < 0 { r.checked_neg()? } else { r }, en.min(em), MAX_DECFLOAT_PRECISION)
}

/// `a` × `b` mod `m`, each of `a` and `b` less than `m`, which is below
/// 2^127, without forming the product, doubled and added a bit of `b` at
/// a time, each sum less than 2`m`.
fn times_mod(a: u128, b: u128, m: u128) -> u128 {
    let mut product = 0_u128;
    for bit in (0..u128::BITS).rev() {
        product = product.wrapping_shl(1).checked_rem(m).unwrap_or(0);
        if b.wrapping_shr(bit) & 1 == 1 {
            product = product.wrapping_add(a).checked_rem(m).unwrap_or(0);
        }
    }
    product
}

/// 10^`k` mod `m`, by squaring.
fn power_mod(k: u32, m: u128) -> u128 {
    let (mut result, mut square, mut left) = (1_u128.checked_rem(m).unwrap_or(0), RADIX.checked_rem(m).unwrap_or(0), k);
    while left > 0 {
        if left & 1 == 1 {
            result = times_mod(result, square, m);
        }
        square = times_mod(square, square, m);
        left = left.wrapping_shr(1);
    }
    result
}
