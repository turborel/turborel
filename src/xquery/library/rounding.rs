//! The functions on numbers [XQueryFO §6.4]: `fn:abs`, `fn:ceiling`,
//! `fn:floor`, `fn:round` and `fn:round-half-to-even`, each of a value of
//! the numeric type of its argument, or of the numeric type its type is
//! derived from.

use std::num::FpCategory;

use super::args::{arg, integer, optional, optional_atomic};
use super::decimal::{Decimal, Rounding};
use super::numeric::Number;
use super::{Builtin, Focus};
use crate::error::Result;
use crate::value::round_to_single;
use crate::xml::Sequence;

/// A half, the fraction a rounding to the nearest is between two at.
const HALF: f64 = 0.5;

/// The functions of [XQueryFO §6.4].
pub(super) const FUNCTIONS: [Builtin; 5] = [
    Builtin::function("abs", 1, 1, abs),
    Builtin::function("ceiling", 1, 1, ceiling),
    Builtin::function("floor", 1, 1, floor),
    Builtin::function("round", 1, 1, round),
    Builtin::function("round-half-to-even", 1, 2, round_half_to_even),
];

/// The number of `args`'s first, an untyped value cast to a double; none
/// of the empty sequence [XQueryFO §6.4].
fn operand(args: &[Sequence]) -> Result<Option<Number>> {
    optional_atomic(arg(args, 0))?.map(|a| Number::of(&a)).transpose()
}

/// `n` mapped by `exact` if an integer or decimal, by `real` if a float or
/// double, its type kept.
fn mapped(n: Number, exact: impl Fn(&Decimal) -> Decimal, real: impl Fn(f64) -> f64) -> Number {
    match n {
        Number::Integer(d) => Number::Integer(exact(&d)),
        Number::Decimal(d) => Number::Decimal(exact(&d)),
        Number::Float(x) => Number::Float(round_to_single(real(x))),
        Number::Double(x) => Number::Double(real(x)),
    }
}

/// `args`'s first, mapped as `mapped` maps it; the empty sequence of the
/// empty sequence.
fn apply(args: &[Sequence], exact: impl Fn(&Decimal) -> Decimal, real: impl Fn(f64) -> f64) -> Result<Sequence> {
    let Some(n) = operand(args)? else { return Ok(Sequence::empty()) };
    Ok(optional(Some(mapped(n, exact, real).atomic()?)))
}

/// [XQueryFO §6.4.1] `fn:abs($arg as numeric?) as numeric?`: its absolute
/// value, positive zero and infinity of a float or double.
fn abs(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    apply(args, Decimal::abs, f64::abs)
}

/// [XQueryFO §6.4.2] `fn:ceiling($arg as numeric?) as numeric?`: the least
/// number with no fraction not less than it, negative zero of a float or
/// double between -1 and zero.
fn ceiling(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    apply(args, |d| d.round(0, Rounding::Ceiling), f64::ceil)
}

/// [XQueryFO §6.4.3] `fn:floor($arg as numeric?) as numeric?`: the
/// greatest number with no fraction not greater than it.
fn floor(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    apply(args, |d| d.round(0, Rounding::Floor), f64::floor)
}

/// [XQueryFO §6.4.4] `x` rounded to the nearest number with no fraction,
/// of two the one toward positive infinity; negative zero of a negative
/// number from -0.5, which `floor(x + 0.5)` would not give.
pub(super) fn round_real(x: f64) -> f64 {
    if !x.is_finite() || x.classify() == FpCategory::Zero {
        return x;
    }
    let below = x.floor();
    // x - floor(x) is exact: both are within one binade's spacing.
    let rounded = if x - below >= HALF { below + 1.0_f64 } else { below };
    if rounded.classify() == FpCategory::Zero && x.is_sign_negative() { -0.0_f64 } else { rounded }
}

/// [XQueryFO §6.4.4] `fn:round($arg as numeric?) as numeric?`.
fn round(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    apply(args, |d| d.round(0, Rounding::HalfUp), round_real)
}

/// [XQueryFO §6.4.5] `x`, a float if `single` or double, rounded half to
/// even at `precision` digits after the period, on its decimal value, as
/// the mantissa with exponent zero is rounded; negative zero of a negative
/// number it rounds to zero.
fn round_real_half_even(x: f64, precision: i64, single: bool) -> f64 {
    if !x.is_finite() || x.classify() == FpCategory::Zero {
        return x;
    }
    // Display writes the shortest decimal that reads back as the number,
    // never with an exponent.
    let written = if single { format!("{x:e}").parse::<f32>().map(|f| f.to_string()).unwrap_or_default() } else { x.to_string() };
    let Some(rounded) = Decimal::parse(&written).map(|d| d.round(precision, Rounding::HalfEven)) else { return x };
    let back = rounded.text().parse::<f64>().unwrap_or(x);
    if back.classify() == FpCategory::Zero && x.is_sign_negative() { -0.0_f64 } else { back }
}

/// [XQueryFO §6.4.5] `fn:round-half-to-even($arg as numeric?, $precision
/// as xs:integer) as numeric?`: the nearest multiple of ten to the minus
/// `$precision`, of two the one whose last digit is even; `$precision` 0
/// if not given.
fn round_half_to_even(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let precision = match args.get(1) {
        Some(given) => {
            let p = integer(given)?;
            p.to_i128().map_or(if p.is_negative() { i64::MIN } else { i64::MAX }, saturated)
        }
        None => 0,
    };
    let Some(n) = operand(args)? else { return Ok(Sequence::empty()) };
    let single = matches!(n, Number::Float(_));
    let rounded = mapped(n, |d| d.round(precision, Rounding::HalfEven), |x| round_real_half_even(x, precision, single));
    Ok(optional(Some(rounded.atomic()?)))
}

/// `n` within an `i64`, saturated.
fn saturated(n: i128) -> i64 {
    i64::try_from(n).unwrap_or(if n < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests;
