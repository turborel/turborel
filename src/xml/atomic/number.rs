//! Numbers and truth values [Schema2 §3.2.2 to §3.2.5, §3.3.13]: their
//! lexical forms, the strings casting them to `xs:string` gives [XQueryFO
//! §17.1.2], and casts among them [XQueryFO §17.1.3, §17.1.6].
//!
//! \[DESIGN\] RULES.md D86: an `xs:decimal` or `xs:integer` is kept as its
//! digits, so it has no limit; an `xs:float` as the `f64` its single
//! precision value widens to.

use std::num::FpCategory;

use super::types::Primitive;
use super::{Atomic, xquery_error};
use crate::error::Result;

/// [XQueryFO §17.1.2] The least magnitude a float or double is written
/// as a decimal of, one millionth.
const DECIMAL_LEAST: f64 = 0.000_001;
/// [XQueryFO §17.1.2] The magnitude from which it is written with an
/// exponent, one million.
const DECIMAL_BOUND: f64 = 1_000_000.0;
/// [Schema2 §3.2.2.1] The literals of `xs:boolean` true.
const TRUE: [&str; 2] = ["true", "1"];
/// [Schema2 §3.2.2.1] The literals of `xs:boolean` false.
const FALSE: [&str; 2] = ["false", "0"];
/// [Schema2 §3.2.4.1] The literals of the special values of a float or
/// double: positive and negative infinity, and not-a-number.
const SPECIAL: [(&str, f64); 3] = [("INF", f64::INFINITY), ("-INF", f64::NEG_INFINITY), ("NaN", f64::NAN)];

/// Whether `s` is digits alone.
fn digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// The sign and the digits before and after the period of `text`, if it
/// is a lexical form of `xs:decimal`: a sign perhaps, then digits with a
/// period perhaps among them, one digit at least [Schema2 §3.2.3.1].
pub(super) fn parts(text: &str) -> Option<(bool, &str, &str)> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let valid = digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    valid.then_some((negative, whole, fraction))
}

/// The string of the decimal of the sign `negative` and the digits
/// `whole` and `fraction` [XQueryFO §17.1.2]: of an integer its digits,
/// else its canonical form [Schema2 §3.2.3.2], no zero leading or
/// trailing; never negative zero.
fn decimal_text(negative: bool, whole: &str, fraction: &str) -> String {
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = fraction.trim_end_matches('0');
    let sign = if negative && (whole != "0" || !fraction.is_empty()) { "-" } else { "" };
    if fraction.is_empty() { format!("{sign}{whole}") } else { format!("{sign}{whole}.{fraction}") }
}

/// The string of the `xs:decimal` of `text`, if it is a lexical form of
/// one [Schema2 §3.2.3.1] [XQueryFO §17.1.2].
pub fn decimal(text: &str) -> Option<String> {
    parts(text).map(|(negative, whole, fraction)| decimal_text(negative, whole, fraction))
}

/// The string of the `xs:integer` of `text`, if it is a lexical form of
/// one: a sign perhaps, then digits [Schema2 §3.3.13.1, §3.3.13.2].
pub fn integer(text: &str) -> Option<String> {
    let (negative, whole, _) = parts(text).filter(|_| !text.contains('.'))?;
    Some(decimal_text(negative, whole, ""))
}

/// The `xs:double` of `text`, if it is a lexical form of one: a decimal's,
/// then E or e and an integer's perhaps, or a special value's [Schema2
/// §3.2.5.1].
pub fn double(text: &str) -> Option<f64> {
    if let Some((_, x)) = SPECIAL.iter().find(|(literal, _)| *literal == text) {
        return Some(*x);
    }
    let (mantissa, exponent) = text.split_once(['E', 'e']).unwrap_or((text, "0"));
    parts(mantissa)?;
    integer(exponent)?;
    text.parse::<f64>().ok()
}

/// The `xs:float` of `text`, if it is a lexical form of one [Schema2
/// §3.2.4.1], rounded to single precision.
pub fn float(text: &str) -> Option<f64> {
    double(text)?;
    single(text)
}

/// The single precision value nearest the number `text` writes, widened.
fn single(text: &str) -> Option<f64> {
    if let Some((_, x)) = SPECIAL.iter().find(|(literal, _)| *literal == text) {
        return Some(*x);
    }
    text.parse::<f32>().ok().map(f64::from)
}

/// The string of the float, if `single`, or double `x` [XQueryFO
/// §17.1.2]: NaN, INF, -INF, 0 or -0; a decimal's from a millionth to
/// below a million; else the canonical form [Schema2 §3.2.4.2, §3.2.5.2],
/// its mantissa the fewest digits that read back as `x`.
pub fn real_text(x: f64, single: bool) -> String {
    match x.classify() {
        FpCategory::Nan => return "NaN".to_owned(),
        FpCategory::Infinite => return if x.is_sign_negative() { "-INF" } else { "INF" }.to_owned(),
        FpCategory::Zero => return if x.is_sign_negative() { "-0" } else { "0" }.to_owned(),
        FpCategory::Subnormal | FpCategory::Normal => {}
    }
    let magnitude = x.abs();
    if (DECIMAL_LEAST..DECIMAL_BOUND).contains(&magnitude) {
        let narrow = format!("{x:e}").parse::<f32>().ok();
        let shortest = if single { narrow.map(|f| f.to_string()) } else { None }.unwrap_or_else(|| x.to_string());
        return decimal(&shortest).unwrap_or(shortest);
    }
    exponent_form(x, single)
}

/// The float, if `single`, or double `x`, finite, written as a mantissa
/// of one digit before its period, E and an exponent [Schema2 §3.2.4.2,
/// §3.2.5.2], the mantissa the fewest digits that read back as `x`.
fn exponent_form(x: f64, single: bool) -> String {
    let narrow = || format!("{x:e}").parse::<f32>().ok();
    let shortest = if single { narrow().map(|f| format!("{f:e}")) } else { None }.unwrap_or_else(|| format!("{x:e}"));
    let (mantissa, exponent) = shortest.split_once('e').unwrap_or((&shortest, "0"));
    if mantissa.contains('.') { format!("{mantissa}E{exponent}") } else { format!("{mantissa}.0E{exponent}") }
}

/// The canonical representation of the float, if `single`, or double `x`
/// [Schema2 §3.2.4.2, §3.2.5.2]: NaN, INF or -INF; else a mantissa of one
/// digit before its period and one after it at least, E, and an exponent,
/// 0.0E0 or -0.0E0 of a zero.
pub fn real_canonical(x: f64, single: bool) -> String {
    match x.classify() {
        FpCategory::Nan | FpCategory::Infinite => real_text(x, single),
        FpCategory::Zero => if x.is_sign_negative() { "-0.0E0" } else { "0.0E0" }.to_owned(),
        FpCategory::Subnormal | FpCategory::Normal => exponent_form(x, single),
    }
}

/// The `xs:boolean` of `text`, if it is a lexical form of one [Schema2
/// §3.2.2.1].
pub fn boolean(text: &str) -> Option<bool> {
    if TRUE.contains(&text) {
        Some(true)
    } else if FALSE.contains(&text) {
        Some(false)
    } else {
        None
    }
}

/// The canonical form of the `xs:boolean` `b` [Schema2 §3.2.2.2].
pub fn boolean_text(b: bool) -> String {
    if b { TRUE } else { FALSE }.first().copied().unwrap_or_default().to_owned()
}

/// A number or truth value, as a cast among them reads it.
enum Number {
    /// An `xs:decimal` or `xs:integer`: its string.
    Exact(Box<str>),
    /// An `xs:float` or `xs:double`.
    Real(f64),
    /// An `xs:boolean`.
    Truth(bool),
}

/// `value` as a number, if it is a number or a truth value.
fn number(value: &Atomic) -> Option<Number> {
    match value.kind.primitive() {
        Primitive::Decimal | Primitive::Integer => Some(Number::Exact(value.lexical.clone())),
        // A float is its single precision value [Schema2 §3.2.4].
        Primitive::Float => float(&value.lexical).map(Number::Real),
        Primitive::Double => double(&value.lexical).map(Number::Real),
        Primitive::Boolean => boolean(&value.lexical).map(Number::Truth),
        Primitive::Untyped
        | Primitive::String
        | Primitive::Duration
        | Primitive::YearMonthDuration
        | Primitive::DayTimeDuration
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::HexBinary
        | Primitive::Base64Binary
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => None,
    }
}

/// The error of a NaN or infinity `x` cast as an `xs:decimal` or
/// `xs:integer` [XQueryFO §17.1.3.3, §17.1.3.4].
fn not_finite(x: f64) -> crate::error::SqlError {
    xquery_error(&format!("{} has no value of a decimal", real_text(x, false)), "[XQueryFO §17.1.3.3] err:FOCA0002")
}

/// The truth value of `n` [XQueryFO §17.1.6]: false of zero and NaN.
fn truth(n: &Number) -> bool {
    match n {
        Number::Exact(t) => &**t != "0",
        Number::Real(x) => !matches!(x.classify(), FpCategory::Zero | FpCategory::Nan),
        Number::Truth(b) => *b,
    }
}

/// The string of `n` cast as an `xs:decimal` [XQueryFO §17.1.3.3]: of a
/// float or double, the shortest decimal that reads back as it; of a truth
/// value, 1 or 0.
fn exact(n: Number) -> Result<String> {
    match n {
        Number::Exact(t) => Ok(t.into_string()),
        Number::Real(x) => {
            if x.is_finite() {
                Ok(decimal(&x.to_string()).unwrap_or_default())
            } else {
                Err(not_finite(x))
            }
        }
        Number::Truth(b) => Ok(if b { "1" } else { "0" }.to_owned()),
    }
}

/// `n` as a double, or a float if `single` [XQueryFO §17.1.3.1,
/// §17.1.3.2]: of a decimal, the number its string writes; of a float or
/// double, as a float its mantissa truncated; of a truth value, 1 or 0.
fn real(n: &Number, single_precision: bool) -> f64 {
    let written = match n {
        Number::Exact(t) => t.to_string(),
        Number::Real(x) => {
            if single_precision {
                return truncated_single(*x);
            }
            format!("{x:e}")
        }
        Number::Truth(b) => if *b { "1" } else { "0" }.to_owned(),
    };
    if single_precision { single(&written) } else { double(&written) }.unwrap_or_default()
}

/// [XQueryFO §17.1.3.1] The bits of a double's fraction.
const FRACTION_BITS: u32 = 52;
/// The bits of a double's biased exponent, after its fraction.
const EXPONENT_MASK: u64 = 0x7FF;
/// The exponent of a double's least subnormal, 2^-1074, and what its
/// biased exponent less one is to an exponent of an integer mantissa.
const DOUBLE_LEAST_EXPONENT: i64 = -1074;
/// [XQueryFO §17.1.3.1] A float's mantissa is below 2^24.
const FLOAT_MANTISSA_BITS: u32 = 24;
/// [XQueryFO §17.1.3.1] The greatest exponent of a float's value space.
const FLOAT_MOST_EXPONENT: i32 = 104;
/// [XQueryFO §17.1.3.1] The least exponent of a float's value space.
const FLOAT_LEAST_EXPONENT: i32 = -149;
/// The radix of a mantissa's exponent.
const TWO: f64 = 2.0;

/// [XQueryFO §17.1.3.1] The float a double `x` casts to: `x` as m × 2^e,
/// m halved, its bits truncated on the right, until it is below 2^24, e
/// raised as often; an infinity past 2^104; below 2^-149 its bits
/// truncated further, so a float's subnormals are reached and a value below
/// them is a zero, of `x`'s sign. NaN, infinities and zeros are themselves.
fn truncated_single(x: f64) -> f64 {
    if !x.is_finite() || x.classify() == FpCategory::Zero {
        return x;
    }
    let bits = x.to_bits();
    let biased = i64::try_from(bits.checked_shr(FRACTION_BITS).unwrap_or(0) & EXPONENT_MASK).unwrap_or(0);
    let fraction = bits & 1_u64.checked_shl(FRACTION_BITS).unwrap_or(0).saturating_sub(1);
    let (mut mantissa, mut exponent) = if biased == 0 {
        (fraction, DOUBLE_LEAST_EXPONENT)
    } else {
        (fraction | 1_u64.checked_shl(FRACTION_BITS).unwrap_or(0), biased.saturating_sub(1).saturating_add(DOUBLE_LEAST_EXPONENT))
    };
    while mantissa >= 1_u64.checked_shl(FLOAT_MANTISSA_BITS).unwrap_or(u64::MAX) {
        mantissa = mantissa.checked_shr(1).unwrap_or(0);
        exponent = exponent.saturating_add(1);
    }
    let sign = if x.is_sign_negative() { -1.0_f64 } else { 1.0_f64 };
    let exponent = i32::try_from(exponent).unwrap_or(i32::MIN);
    if exponent > FLOAT_MOST_EXPONENT {
        return sign * f64::INFINITY;
    }
    if exponent < FLOAT_LEAST_EXPONENT {
        // A float below 2^-126 is a subnormal, m × 2^-149 of m below 2^23:
        // the same truncation of m's bits makes it one, or a zero below.
        let excess = u32::try_from(FLOAT_LEAST_EXPONENT.saturating_sub(exponent)).unwrap_or(u32::MAX);
        mantissa = mantissa.checked_shr(excess).unwrap_or(0);
        return sign * f64::from(u32::try_from(mantissa).unwrap_or(0)) * TWO.powi(FLOAT_LEAST_EXPONENT);
    }
    // m below 2^24 and 2^e within a float's range make m × 2^e exact.
    sign * f64::from(u32::try_from(mantissa).unwrap_or(0)) * TWO.powi(exponent)
}

/// `value`, a number or truth value, cast as `to`, a number or truth
/// value type not its own [XQueryFO §17.1.3, §17.1.6]; none if it is no
/// number or `to` none of them.
///
/// # Errors
///
/// SQLSTATE 10000, err:FOCA0002, of a NaN or infinity cast as a decimal
/// or integer.
pub fn convert(value: &Atomic, to: Primitive) -> Result<Option<String>> {
    let Some(n) = number(value) else { return Ok(None) };
    Ok(Some(match to {
        Primitive::Boolean => boolean_text(truth(&n)),
        Primitive::Decimal => exact(n)?,
        // [XQueryFO §17.1.3.4] the fraction discarded.
        Primitive::Integer => integer(exact(n)?.split('.').next().unwrap_or_default()).unwrap_or_default(),
        Primitive::Float => real_text(real(&n, true), true),
        Primitive::Double => real_text(real(&n, false), false),
        Primitive::Untyped
        | Primitive::String
        | Primitive::Duration
        | Primitive::YearMonthDuration
        | Primitive::DayTimeDuration
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::HexBinary
        | Primitive::Base64Binary
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => return Ok(None),
    }))
}
