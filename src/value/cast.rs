//! [ISO/IEC 9075:1992 §6.10] CAST, as the target type's kind casts
//! (`value::types`), and the literals it reads and writes, which the kinds
//! share.

use super::{DataType, Decimal, MAX_EXACT_PRECISION, REAL_PRECISION, Value, pow10, round_to_single};
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
    to.kind().cast(v, to)
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
        Some(spaces) => Ok(Value::Char(format!("{text}{}", " ".repeat(spaces)))),
        None => fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[ISO/IEC 9075:1992 §6.10 GR5a iv] '{text}' is longer than CHARACTER({length})"),
        ),
    }
}

/// [ISO/IEC 9075:1992 §6.10 GR3b, GR4b] The `<signed numeric literal>` `s`
/// is, its spaces trimmed, to cast to a number.
///
/// # Errors
///
/// SQLSTATE 22018 if it is none \[GR3b i, GR4b i\].
pub(crate) fn numeric_literal(s: &str) -> Result<Value> {
    literal_value(s.trim_matches(' ')).map_or_else(
        || {
            fail_with(
                Code::Data,
                SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
                format!("[ISO/IEC 9075:1992 §6.10 GR3b i, GR4b i] '{s}' is not a <signed numeric literal>"),
            )
        },
        Ok,
    )
}

/// [ISO/IEC 9075-2:2023 §5.3 SR22] The digits of `t` if it is an
/// `<unsigned decimal integer>`, or empty, an `<underscore>` only between
/// two digits: the same without its underscores.
fn digits_of(t: &str) -> Option<String> {
    let placed = !t.starts_with('_') && !t.ends_with('_') && !t.contains("__");
    (placed && t.chars().all(|c| c.is_ascii_digit() || c == '_')).then(|| t.replace('_', ""))
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

/// The value of an `<exponent>`'s digits `unsigned`, of the sign
/// `negative` says, saturated past an `i64`.
fn exponent_value(negative: bool, unsigned: &str) -> i64 {
    let magnitude =
        unsigned.trim_start_matches('0').parse::<i64>().unwrap_or(if unsigned.trim_start_matches('0').is_empty() { 0 } else { i64::MAX });
    if negative { magnitude.saturating_neg() } else { magnitude }
}

/// [ISO/IEC 9075:1992 §5.3] `s` read, if it is a `<signed numeric
/// literal>`: an exact one, [+|-] digits with a period among or before
/// them, or an approximate one, that followed by E and a signed integer;
/// its digits perhaps with underscores [ISO/IEC 9075-2:2023 §5.3,
/// §6.13 GR8b, GR9b, GR10b].
pub(crate) fn numeric_parts(s: &str) -> Option<NumericLiteral> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (mantissa, exponent) = match body.split_once(['E', 'e']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let (whole, fraction) = (digits_of(whole)?, digits_of(fraction)?);
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let exponent = match exponent {
        Some(e) => {
            let e = e.strip_prefix('+').unwrap_or(e);
            let (minus, unsigned) = e.strip_prefix('-').map_or((false, e), |u| (true, u));
            Some(exponent_value(minus, &digits_of(unsigned).filter(|u| !u.is_empty())?))
        }
        None => None,
    };
    Some(NumericLiteral { negative, fraction: fraction.len(), digits: format!("{whole}{fraction}"), exponent })
}

/// [ISO/IEC 9075:1992 §5.3] The value of `s` if it is a `<signed numeric
/// literal>` (`numeric_parts`): of an exact one exact, of an approximate
/// one approximate.
fn literal_value(s: &str) -> Option<Value> {
    let lit = numeric_parts(s)?;
    let (whole, fraction) = lit.digits.split_at(lit.digits.len().checked_sub(lit.fraction)?);
    if let Some(e) = lit.exponent {
        let x: f64 = format!("{whole}.{fraction}E{e}").parse().ok()?;
        return Some(Value::Approx(if lit.negative { -x } else { x }));
    }
    let scale = u32::try_from(lit.fraction).ok()?;
    let units: i128 = lit.digits.parse().ok()?;
    Some(Value::Exact(Decimal::new(if lit.negative { units.checked_neg()? } else { units }, scale)))
}

// Unit tests: the literals a cast reads and writes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3, §6.10 GR3b, GR5b i] what is a
    /// `<signed numeric literal>`, and the shortest approximate literal;
    /// [ISO/IEC 9075-2:2023 §5.3 SR22] its digits with underscores between them.
    ///
    /// `digits_of`'s decision, MC/DC pairs against "1_0", true: "_1" shows
    /// the first underscore, "1_" the last, "1__0" two together, "A1" a
    /// character neither digit nor underscore.
    #[test]
    fn numeric_literals() {
        let exact = |s: &str| if let Some(Value::Exact(d)) = literal_value(s) { Some((d.units, d.scale)) } else { None };
        assert_eq!(exact("-12.50"), Some((-1250, 2)));
        assert_eq!(exact(".5"), Some((5, 1)));
        let Some(Value::Approx(x)) = literal_value("+1E-2") else { panic!("+1E-2 is approximate") };
        assert!((x - 0.01_f64).abs() < 1e-12_f64);
        assert_eq!(exact("1_000.000_5"), Some((10_000_005, 4)), "underscores between digits");
        let Some(Value::Approx(y)) = literal_value("5.E1_0") else { panic!("5.E1_0 is approximate") };
        assert!((y - 5e10_f64).abs() < 1.0_f64);
        for bad in ["", ".", "1.2.3", "E1", "1E", "1E+", "A1", "--1", "_1", "1_", "1__0", "1_.5", "1._5", "1E_1", "_"] {
            assert!(literal_value(bad).is_none(), "{bad}");
        }
        assert_eq!(approximate_literal(0.0), "0E0");
        assert_eq!(approximate_literal(100.0), "1.0E2");
        assert_eq!(approximate_literal(-1.5e-3), "-1.5E-3");
    }
}
