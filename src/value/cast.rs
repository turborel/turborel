//! [ISO/IEC 9075:1992 §6.10] CAST, as the target type's kind casts
//! (`value::types`), and the literals it reads and writes, which the kinds
//! share.

use super::{DataType, Decimal, MAX_EXACT_PRECISION, REAL_PRECISION, Value, pow10, round_to_single};
use std::fmt::Write as _;

use crate::error::{Code, Result, SqlState, fail_with};

/// [ISO/IEC 9075:1992 §6.10 GR2-GR5] `v` cast to `to`, as its kind casts
/// (`SqlType::cast`): null stays null \[GR2a\]. Every one of SQL-86's types
/// may be cast to every other \[SR4\].
///
/// # Errors
///
/// SQLSTATE 22003 if a number does not fit \[GR3a ii, GR4a ii\]; 22018 if a
/// string is no numeric literal \[GR3b i, GR4b i\]; 22001 if a number's
/// literal is longer than the string type \[GR5a iv, GR5b iii 4\]; 22005 if
/// `v` is of a kind CAST does not take to `to` \[SR4\]; others as the
/// target's kind says, as of a datetime or an interval.
pub fn cast(v: Value, to: &DataType) -> Result<Value> {
    cast_warned(v, to).map(|(cast, _)| cast)
}

/// [ISO/IEC 9075:1992 §6.10] `cast`, and the warning the cast raises, a
/// completion condition [§3.3.4.1], if it raises one: of a string cut
/// \[GR5c ii, GR6c ii\], or a binary string [ISO/IEC 9075-2:2023 §6.13
/// GR13], as the target type's kind says (`SqlType::cast_warned`).
///
/// # Errors
///
/// As `cast`.
pub fn cast_warned(v: Value, to: &DataType) -> Result<(Value, Option<SqlState>)> {
    // [ISO/IEC 9075-2:2023 §6.13 GR11d, GR12d, GR13] A large object to a
    // type of no large object: what fits, read, and taken as a string.
    if let Value::Lob(l) = &v
        && let Some((lowered, cut)) = super::types::lob::lowered(l.clone(), to, super::types::lob::Lowering::Cast)?
    {
        let (cast, warning) = to.kind().cast_warned(lowered, to)?;
        return Ok((cast, cut.or(warning)));
    }
    // [EXTENSION] JSON to a character string: its JSON text, as
    // JSON_SERIALIZE gives it [ISO/IEC 9075-2:2023 §6.37] (`types::json`).
    if let Value::Json(j) = &v
        && to.is_character()
    {
        return crate::json::serialize_as(&j.item()?, to).map(|text| (text, None));
    }
    // [EXTENSION] A value of an extension kind, as its kind lowers it
    // to a predefined type (`SqlType::lowered`).
    if let Value::Extension(e) = &v
        && !matches!(to, DataType::Extension(_))
        && let Some(lowered) = e.kind.lowered(&v, to)
    {
        return to.kind().cast_warned(lowered, to);
    }
    to.kind().cast_warned(v, to)
}

/// [ISO/IEC 9075:1992 §6.10 SR4] `v` has no value of `t`: of a kind CAST
/// does not take to `t`, as a plug-in function might give.
///
/// # Errors
///
/// Always: SQLSTATE 22005.
pub(crate) fn uncastable(v: &Value, t: &DataType) -> Result<Value> {
    fail_with(Code::Data, SqlState::ERROR_IN_ASSIGNMENT, format!("[ISO/IEC 9075:1992 §6.10 SR4] the value {v} cannot be cast to {t}"))
}

/// `x` at an approximate type's precision: REAL's rounded to single.
pub(crate) fn at_precision(x: f64, precision: u32) -> f64 {
    if precision <= REAL_PRECISION { round_to_single(x) } else { x }
}

/// \[GR3a; §9.2 GR3k\] `x` as an exact number of `precision` and `scale`,
/// rounded to the scale, if it fits; `rule` is the rule applied, for the
/// error.
pub(crate) fn approximate_to_exact(x: f64, (precision, scale): (u32, u32), rule: &str) -> Result<Value> {
    let out_of_range =
        || fail_with(Code::Data, SqlState::NUMERIC_VALUE_OUT_OF_RANGE, format!("[{rule}] {x:E} does not fit NUMERIC({precision},{scale})"));
    let places = usize::try_from(scale).unwrap_or(usize::MAX);
    let text = format!("{x:.places$}");
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let Ok(units) = digits.parse::<i128>() else { return out_of_range() };
    let units = if text.starts_with('-') { units.saturating_neg() } else { units };
    let limit = pow10(precision.min(MAX_EXACT_PRECISION));
    if !x.is_finite() || limit.is_none_or(|l| units.unsigned_abs() >= l.unsigned_abs()) {
        return out_of_range();
    }
    Ok(Value::Exact(Decimal::new(units, scale)))
}

/// \[GR5b i\] The shortest `<approximate numeric literal>` of `x`: its
/// mantissa one digit not zero, a period and digits; 0E0 for zero.
pub(crate) fn approximate_literal(x: f64) -> String {
    if x == 0.0_f64 {
        return "0E0".to_owned();
    }
    let text = format!("{x:E}");
    if let Some((mantissa, exponent)) = text.split_once('E')
        && !mantissa.contains('.')
    {
        return format!("{mantissa}.0E{exponent}");
    }
    text
}

/// [GR5a ii-iv] `text` padded to `length`, if it is no longer.
pub(crate) fn padded(text: &str, length: u32) -> Result<Value> {
    let n = text.chars().count();
    match usize::try_from(length).ok().and_then(|l| l.checked_sub(n)) {
        Some(spaces) => Ok(Value::Char(super::CharString::padded(text, spaces))),
        None => fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[ISO/IEC 9075:1992 §6.10 GR5a iv] '{text}' is longer than CHARACTER({length})"),
        ),
    }
}

/// [ISO/IEC 9075:1992 §6.10 GR3b, GR4b] The `<signed numeric literal>` `s`
/// is, its spaces trimmed, to cast to a number. It is read where it lies,
/// with nothing allocated, as every cast of a string to a number reads one
/// (RULES.md D1148).
///
/// # Errors
///
/// SQLSTATE 22018 if it is none \[GR3b i, GR4b i\]; 22003 if it is one
/// past the range of every number of this implementation, as the literal
/// it is cast as fits no target [ISO/IEC 9075-2:2023 §6.13 GR8b ii, GR8a
/// ii, GR9b ii, GR9a ii], where written in a statement it breaks a Syntax
/// Rule [§5.3 SR30] (RULES.md D970).
pub(crate) fn numeric_literal(s: &str) -> Result<Value> {
    let Some(lit) = literal_parts(s.trim_matches(' ')) else {
        return fail_with(
            Code::Data,
            SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
            format!("[ISO/IEC 9075:1992 §6.10 GR3b i, GR4b i] '{s}' is not a <signed numeric literal>"),
        );
    };
    literal_value(&lit).map_or_else(
        || {
            fail_with(
                Code::Data,
                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                format!("[ISO/IEC 9075-2:2023 §6.13 GR8a ii, GR9a ii] '{s}' is outside the range of this implementation's numbers"),
            )
        },
        Ok,
    )
}

/// The separator a `<signed numeric literal>`'s digits may hold [ISO/IEC
/// 9075-2:2023 §5.3 SR22].
const UNDERSCORE: char = '_';

/// [ISO/IEC 9075-2:2023 §5.3 SR22] Whether `t` is an `<unsigned decimal
/// integer>`, or empty, an `<underscore>` only between two digits.
fn placed(t: &str) -> bool {
    let between = !t.starts_with(UNDERSCORE) && !t.ends_with(UNDERSCORE) && !t.contains("__");
    between && t.chars().all(|c| c.is_ascii_digit() || c == UNDERSCORE)
}

/// The digits of `t`, an `<unsigned decimal integer>` `placed` approves,
/// its underscores passed over: no string is made of them.
fn digits_in(t: &str) -> impl Iterator<Item = u8> + Clone + '_ {
    t.bytes().filter(u8::is_ascii_digit)
}

/// [ISO/IEC 9075:1992 §5.3] A `<signed numeric literal>`, read: its
/// sign, its digits, whole and fraction together without underscores, how
/// many are of the fraction, and its exponent, if it is approximate,
/// saturated past the range of an `i64`.
pub(crate) struct NumericLiteral {
    /// Whether it is negative.
    pub negative: bool,
    /// Its digits.
    pub digits: String,
    /// How many of them are after the period.
    pub fraction: usize,
    /// Its exponent, if it is an approximate numeric literal.
    pub exponent: Option<i64>,
}

/// A `<signed numeric literal>` read where it lies: its sign, its
/// mantissa's text, its whole digits' and its fraction's, each perhaps
/// with underscores, and its exponent, saturated, if it is approximate.
struct LiteralParts<'a> {
    /// Whether it is negative.
    negative: bool,
    /// Its text without its sign.
    body: &'a str,
    /// Its digits before the period.
    whole: &'a str,
    /// Its digits after the period.
    fraction: &'a str,
    /// Its exponent, if it is an approximate numeric literal.
    exponent: Option<i64>,
}

/// The value of an `<exponent>`'s digits `unsigned`, of the sign
/// `negative` says, saturated past an `i64`: as many digits no `i64`
/// holds are its greatest.
fn exponent_value(negative: bool, unsigned: &str) -> i64 {
    let magnitude =
        digits_in(unsigned).fold(0_i64, |m, d| m.saturating_mul(i64::from(DECIMAL_BASE)).saturating_add(i64::from(d.wrapping_sub(b'0'))));
    if negative { magnitude.saturating_neg() } else { magnitude }
}

/// [ISO/IEC 9075:1992 §5.3] `s` read, if it is a `<signed numeric
/// literal>`: an exact one, [+|-] digits with a period among or before
/// them, or an approximate one, that followed by E and a signed integer;
/// its digits perhaps with underscores [ISO/IEC 9075-2:2023 §5.3,
/// §6.13 GR8b, GR9b, GR10b].
fn literal_parts(s: &str) -> Option<LiteralParts<'_>> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (mantissa, exponent) = match body.split_once(['E', 'e']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if !placed(whole) || !placed(fraction) || (whole.is_empty() && fraction.is_empty()) {
        return None;
    }
    let exponent = match exponent {
        Some(e) => {
            let e = e.strip_prefix('+').unwrap_or(e);
            let (minus, unsigned) = e.strip_prefix('-').map_or((false, e), |u| (true, u));
            if !placed(unsigned) || unsigned.is_empty() {
                return None;
            }
            Some(exponent_value(minus, unsigned))
        }
        None => None,
    };
    Some(LiteralParts { negative, body, whole, fraction, exponent })
}

/// [ISO/IEC 9075:1992 §5.3] `s` read, if it is a `<signed numeric
/// literal>`, as `literal_parts` reads it, its digits gathered without
/// underscores.
pub(crate) fn numeric_parts(s: &str) -> Option<NumericLiteral> {
    let lit = literal_parts(s)?;
    let mut digits = String::with_capacity(lit.whole.len().saturating_add(lit.fraction.len()));
    digits.extend(digits_in(lit.whole).chain(digits_in(lit.fraction)).map(char::from));
    Some(NumericLiteral { negative: lit.negative, fraction: digits_in(lit.fraction).count(), digits, exponent: lit.exponent })
}

/// The nearest `f64` of the decimal `lit` writes, if finite. Of digits
/// with no underscores, `body`, the literal's own text, is read where
/// `f64`'s parsing takes its form, which writes the same number, so no
/// text is made; else the digits are gathered, the exponent saturated.
fn nearest(lit: &LiteralParts<'_>) -> Option<f64> {
    let read = if lit.body.contains(UNDERSCORE) { None } else { lit.body.parse::<f64>().ok() };
    let x = read.or_else(|| gathered(lit))?;
    x.is_finite().then(|| if lit.negative { -x } else { x })
}

/// The nearest `f64` of the decimal `lit` writes, of its digits gathered
/// without underscores and its exponent, saturated, written after them.
fn gathered(lit: &LiteralParts<'_>) -> Option<f64> {
    let mut text = String::with_capacity(lit.body.len().saturating_add(EXPONENT_ROOM));
    text.extend(digits_in(lit.whole).map(char::from));
    text.push('.');
    text.extend(digits_in(lit.fraction).map(char::from));
    write!(text, "E{}", lit.exponent.unwrap_or(0)).ok()?;
    text.parse().ok()
}

/// The base of a literal's digits.
const DECIMAL_BASE: u8 = 10;

/// Room for an `i64` exponent's text and its sign and letter.
const EXPONENT_ROOM: usize = 22;

/// [ISO/IEC 9075:1992 §5.3] The value of the `<signed numeric literal>`
/// `lit`, if a number of this implementation holds it. Of an exact one it
/// is exact, its fraction's digits past those a number of its whole
/// digits keeps within the maximum precision truncated, as every exact
/// target truncates them \[GR3a, implementation-defined\]; of one whose
/// whole digits are more than that, the approximate number nearest, as no
/// exact one holds it. Of an approximate one it is approximate, if finite.
/// Its units are summed digit by digit: no text is made of them.
fn literal_value(lit: &LiteralParts<'_>) -> Option<Value> {
    if lit.exponent.is_some() {
        return nearest(lit).map(Value::Approx);
    }
    let whole = digits_in(lit.whole).skip_while(|&d| d == b'0');
    let Some(room) = usize::try_from(MAX_EXACT_PRECISION).ok()?.checked_sub(whole.clone().count()) else {
        return nearest(lit).map(Value::Approx);
    };
    let kept = digits_in(lit.fraction).take(room);
    let scale = u32::try_from(kept.clone().count()).ok()?;
    // At most `MAX_EXACT_PRECISION` digits, so an `i128` holds them.
    let units = whole
        .chain(kept)
        .try_fold(0_i128, |u, d| u.checked_mul(i128::from(DECIMAL_BASE))?.checked_add(i128::from(d.wrapping_sub(b'0'))))?;
    Some(Value::Exact(Decimal::new(if lit.negative { units.checked_neg()? } else { units }, scale)))
}

// Unit tests: the literals a cast reads and writes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
