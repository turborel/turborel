//! The numeric value functions of Feature T621 [ISO/IEC 9075-2:2023
//! §6.31]: `<natural logarithm>` LN, `<exponential function>` EXP, `<power
//! function>` POWER, `<square root>` SQRT, `<floor function>` FLOOR and
//! `<ceiling function>` CEIL or CEILING. Their names are no key words of
//! SQL-92, so the parser reads each before "(" as the function
//! (`parser/numeric.rs`); each is registered, so the binder finds it by
//! name.
//!
//! \[DESIGN\] Of exact and approximate operands LN, EXP, POWER and SQRT
//! are found in double precision, and their result is DOUBLE PRECISION
//! (IV146, IV148, IV150). Of a DECFLOAT operand the result is DECFLOAT(34)
//! (IV145, IV147, IV149): SQRT is found to its 34 digits, LN, EXP and
//! POWER in double precision through the operand's decimal exponent, so
//! a value past the range of a double is still taken, and are kept to
//! 16 digits, what double precision holds (`numeric/decimal.rs`).

use std::cmp::Ordering;

use super::ScalarFunction;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::value::{DOUBLE_PRECISION, DataType, DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Spelled, Value, compare, pow10};

mod decimal;
pub(super) use decimal::square_root;

/// [ISO/IEC 9075-2:2023 §6.31] LN, no key word of SQL-92.
pub const LN: &str = "LN";
/// [§6.31] EXP, no key word of SQL-92.
pub const EXP: &str = "EXP";
/// [§6.31] POWER, no key word of SQL-92.
pub const POWER: &str = "POWER";
/// [§6.31] SQRT, no key word of SQL-92.
pub const SQRT: &str = "SQRT";
/// [§6.31] FLOOR, no key word of SQL-92.
pub const FLOOR: &str = "FLOOR";
/// [§6.31] CEILING, no key word of SQL-92.
pub const CEILING: &str = "CEILING";
/// [§6.31] CEIL, CEILING by another name.
pub const CEIL: &str = "CEIL";

/// The double precision result of LN, EXP, POWER and SQRT (IV146, IV148,
/// IV150): FLOAT(53), as SUM's approximate result.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };
/// The DECFLOAT result of LN, EXP, POWER and SQRT of a DECFLOAT operand
/// (IV145, IV147, IV149).
const DECFLOAT: DataType = DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION };
/// The exponent of SQRT as a power [§6.31 SR19]: 0.5.
const ONE_HALF: f64 = 0.5;

/// Which of the functions it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numeric {
    /// LN.
    Ln,
    /// EXP.
    Exp,
    /// POWER.
    Power,
    /// SQRT.
    Sqrt,
    /// FLOOR.
    Floor,
    /// CEILING, or CEIL.
    Ceiling,
}

impl Numeric {
    /// Each, in the order of §6.31's Format.
    pub const EVERY: [Numeric; 6] = [Numeric::Ln, Numeric::Exp, Numeric::Power, Numeric::Sqrt, Numeric::Floor, Numeric::Ceiling];

    /// The function of name `name`, CEIL CEILING's; none if it is none.
    pub fn named(name: &str) -> Option<Numeric> {
        if name == CEIL {
            return Some(Numeric::Ceiling);
        }
        Numeric::EVERY.into_iter().find(|n| n.name() == name)
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Numeric::Ln => LN,
            Numeric::Exp => EXP,
            Numeric::Power => POWER,
            Numeric::Sqrt => SQRT,
            Numeric::Floor => FLOOR,
            Numeric::Ceiling => CEILING,
        }
    }

    /// How many operands it takes: POWER two, the others one.
    pub fn operands(self) -> usize {
        if self == Numeric::Power { 2 } else { 1 }
    }
}

/// The functions, to be registered.
pub(super) fn functions() -> Vec<std::sync::Arc<dyn ScalarFunction>> {
    Numeric::EVERY.into_iter().map(|n| -> std::sync::Arc<dyn ScalarFunction> { std::sync::Arc::new(n) }).collect()
}

impl ScalarFunction for Numeric {
    fn name(&self) -> &str {
        Numeric::name(*self)
    }

    /// [§6.31 SR16 to SR20] Each operand a number. Of FLOOR and CEILING
    /// the result is exact of scale 0 of an exact operand \[SR20a\], of
    /// precision enough for its whole part, one digit more than it has
    /// (IV151); else the operand's type \[SR20b, SR20c\]. Of the others
    /// DECFLOAT if an operand is, else DOUBLE PRECISION.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.len() != self.operands() {
            return fail(Code::Api, format!("[DESIGN] {} takes {} operands", self.name(), self.operands()));
        }
        if let Some(t) = args.iter().find(|t| !t.is_numeric()) {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-2:2023 §6.31] the operand of {} is a <numeric value expression>, not {t}", self.name()),
            );
        }
        let decimal = args.iter().any(|t| matches!(t, DataType::DecFloat { .. }));
        match (self, args.first()) {
            (Numeric::Floor | Numeric::Ceiling, Some(DataType::Exact { precision, scale, spelled })) => Ok(if *scale == 0 {
                DataType::Exact { precision: *precision, scale: 0, spelled: *spelled }
            } else {
                DataType::Exact { precision: precision.saturating_sub(*scale).saturating_add(1), scale: 0, spelled: Spelled::Numeric }
            }),
            (Numeric::Floor | Numeric::Ceiling, Some(t)) => Ok(t.clone()),
            (Numeric::Ln | Numeric::Exp | Numeric::Power | Numeric::Sqrt | Numeric::Floor | Numeric::Ceiling, _) => {
                Ok(if decimal { DECFLOAT } else { DOUBLE })
            }
        }
    }

    /// [§6.31 GR12 to GR16] Null if an operand is; else the function's
    /// value.
    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let decimal = args.iter().any(|v| matches!(v, Value::DecFloat(_)));
        match (self, args) {
            (Numeric::Ln, [v]) => natural_logarithm(v),
            (Numeric::Exp, [v]) => exponential(v),
            (Numeric::Power, [base, exponent]) => power(base, exponent, decimal),
            (Numeric::Sqrt, [v]) => square_root_of(v),
            (Numeric::Floor, [v]) => rounded(v, false),
            (Numeric::Ceiling, [v]) => rounded(v, true),
            (Numeric::Ln | Numeric::Exp | Numeric::Power | Numeric::Sqrt | Numeric::Floor | Numeric::Ceiling, _) => {
                fail(Code::Api, format!("[DESIGN] {} takes {} operands", self.name(), self.operands()))
            }
        }
    }
}

/// SQLSTATE 22003, `why` failing.
fn out_of_range<T>(why: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::NUMERIC_VALUE_OUT_OF_RANGE, format!("[ISO/IEC 9075-2:2023 {why}] the result is out of range"))
}

/// SQLSTATE 2201F, of `why`.
fn invalid_power<T>(why: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::INVALID_ARGUMENT_FOR_POWER_FUNCTION, format!("[ISO/IEC 9075-2:2023 §6.31 {why}"))
}

/// An approximate result `x`, finite, else 22003 by `why`.
fn finite(x: f64, why: &str) -> Result<Value> {
    if x.is_finite() { Ok(Value::Approx(x)) } else { out_of_range(why) }
}

/// The number `v` as an `f64`.
fn double(v: &Value) -> Result<f64> {
    v.as_f64().map_or_else(|| fail(Code::Api, format!("[DESIGN] {v} is not a number")), Ok)
}

/// Which side of 0 the number `v` is on.
fn sign(v: &Value) -> Result<Ordering> {
    compare(v, &Value::Exact(Decimal::new(0, 0))).map_or_else(|| fail(Code::Api, format!("[DESIGN] {v} is not a number")), Ok)
}

/// [§6.31 GR12] LN: 2201E of 0 or below, else the natural logarithm.
fn natural_logarithm(v: &Value) -> Result<Value> {
    if sign(v)? != Ordering::Greater {
        return fail_with(
            Code::Data,
            SqlState::INVALID_ARGUMENT_FOR_NATURAL_LOGARITHM,
            "[ISO/IEC 9075-2:2023 §6.31 GR12b] the natural logarithm is of a number above 0",
        );
    }
    if let Value::DecFloat(d) = v {
        return decimal::of_double(decimal::ln(*d)).map_or_else(|| out_of_range("§6.31 GR12c"), Ok);
    }
    finite(double(v)?.ln(), "§6.31 GR12c")
}

/// [§6.31 GR13] EXP: e raised to the power `v`, 22003 if out of range.
fn exponential(v: &Value) -> Result<Value> {
    if let Value::DecFloat(d) = v {
        return decimal::exp10(d.to_f64() * std::f64::consts::LOG10_E).map_or_else(|| out_of_range("§6.31 GR13b"), Ok);
    }
    finite(double(v)?.exp(), "§6.31 GR13b")
}

/// [§6.31 SR19] SQRT ( NVE ), POWER ( NVE, 0.5 ): of a DECFLOAT, found
/// to its 34 digits.
fn square_root_of(v: &Value) -> Result<Value> {
    let Value::DecFloat(d) = v else { return power(v, &Value::Approx(ONE_HALF), false) };
    if sign(v)? == Ordering::Less {
        return invalid_power("GR14e] SQRT of a negative number raises it to 0.5, not a whole number");
    }
    square_root(*d).map(Value::DecFloat).map_or_else(|| out_of_range("§6.31 GR14h"), Ok)
}

/// [§6.31 GR14] POWER ( `base`, `exponent` ), in DECFLOAT if `decimal`.
fn power(base: &Value, exponent: &Value, decimal: bool) -> Result<Value> {
    let number = |x: f64| if decimal { DecFloat::of_f64(x, MAX_DECFLOAT_PRECISION).map(Value::DecFloat) } else { Some(Value::Approx(x)) };
    let odd = match (sign(base)?, sign(exponent)?) {
        (Ordering::Equal, Ordering::Less) => return invalid_power("GR14b] 0 is raised to no negative power"),
        // [GR14c, GR14d]
        (Ordering::Equal, Ordering::Equal) => return number(1.0).map_or_else(|| out_of_range("§6.31 GR14c"), Ok),
        (Ordering::Equal, Ordering::Greater) => return number(0.0).map_or_else(|| out_of_range("§6.31 GR14d"), Ok),
        (Ordering::Less, _) => match whole(exponent)? {
            None => return invalid_power("GR14e] a negative base is raised only to a whole number"),
            // [GR14f, GR14g]
            Some(odd) => odd,
        },
        (Ordering::Greater, _) => false,
    };
    let magnitude = if decimal {
        // A whole exponent, by products, each of 34 digits: so POWER (-2,
        // 3) is -8; else through the base's logarithm.
        if let Some(raised) = decimal::whole_power(base, exponent) {
            return raised.map(Value::DecFloat);
        }
        let log10 = double(exponent)? * decimal::log10(base)?;
        let Some(Value::DecFloat(m)) = decimal::exp10(log10) else { return out_of_range("§6.31 GR14h") };
        Value::DecFloat(if odd { m.negated() } else { m })
    } else {
        let raised = double(base)?.abs().powf(double(exponent)?);
        finite(if odd { -raised } else { raised }, "§6.31 GR14h")?
    };
    Ok(magnitude)
}

/// Whether the number `v` equals an exact number of scale 0: if it does,
/// whether that is odd; none if it does not.
fn whole(v: &Value) -> Result<Option<bool>> {
    if let Value::Exact(Decimal { units, scale }) = v {
        let unit = pow10(*scale).unwrap_or(i128::MAX);
        return Ok((units.checked_rem(unit) == Some(0)).then(|| units.checked_div(unit).is_some_and(|w| w.checked_rem(2) != Some(0))));
    }
    // Its coefficient has no trailing zeros: of a positive exponent it is
    // a multiple of 10, even.
    if let Value::DecFloat(d) = v {
        return Ok(match d.exponent().cmp(&0) {
            Ordering::Less => None,
            Ordering::Equal => Some(d.coefficient().checked_rem(2) != Some(0)),
            Ordering::Greater => Some(false),
        });
    }
    // A double past 2^53 is even: its last bit is above the units.
    let f = double(v)?;
    Ok((compare(&Value::Approx(f.fract()), &Value::Approx(0.0)) == Some(Ordering::Equal))
        .then(|| compare(&Value::Approx(f.rem_euclid(2.0)), &Value::Approx(1.0)) == Some(Ordering::Equal)))
}

/// [§6.31 GR15, GR16] FLOOR, or CEILING if `up`: the greatest whole
/// number not above `v`, or the least not below it, exact of an exact
/// `v`.
fn rounded(v: &Value, up: bool) -> Result<Value> {
    let why = if up { "§6.31 GR16b" } else { "§6.31 GR15b" };
    if let Value::Exact(Decimal { units, scale }) = v {
        let whole = pow10(*scale).and_then(|unit| {
            if up { units.checked_neg()?.checked_div_euclid(unit)?.checked_neg() } else { units.checked_div_euclid(unit) }
        });
        return whole.map(|w| Value::Exact(Decimal::new(w, 0))).map_or_else(|| out_of_range(why), Ok);
    }
    if let Value::DecFloat(d) = v {
        return decimal::rounded(*d, up).map(Value::DecFloat).map_or_else(|| out_of_range(why), Ok);
    }
    let f = double(v)?;
    finite(if up { f.ceil() } else { f.floor() }, why)
}

// Unit tests: each rule of §6.31 GR12 to GR16, of each type (RULES.md
// R-TEST-2).
#[cfg(test)]
mod tests;
