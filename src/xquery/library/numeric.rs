//! The operators on numbers [XQueryFO §6.2, §6.3]: arithmetic and
//! comparison of `xs:integer`, `xs:decimal`, `xs:float` and `xs:double`,
//! two operands of different types promoted to one [XQueryFO §6.2]
//! [XQuery Appendix B.1].
//!
//! \[DESIGN\] RULES.md D86: an integer or decimal is reckoned with by
//! `Decimal`, of any digits up to `MAX_DIGITS`, a result past them
//! err:FOAR0002 [XQueryFO §6.2]; a float or double by IEEE 754 arithmetic,
//! a float's result rounded to single precision, an overflow infinity, as
//! [XQueryFO §6.2] allows. An untyped operand is a double, as the
//! arithmetic operators cast it [XQuery §3.4].

use std::cmp::Ordering;
use std::num::FpCategory;

use super::args::wrong_type;
use super::codes::{FOAR0001, FOAR0002, fail};
use super::decimal::Decimal;
use crate::error::Result;
use crate::value::round_to_single;
use crate::xml::{Atomic, AtomicType, Primitive, cast, double, real_text};

/// A number, of one of the four types the operators are defined on.
#[derive(Debug, Clone, PartialEq)]
pub enum Number {
    /// An `xs:integer`.
    Integer(Decimal),
    /// An `xs:decimal`.
    Decimal(Decimal),
    /// An `xs:float`, as the double it widens to.
    Float(f64),
    /// An `xs:double`.
    Double(f64),
}

/// The error of an integer or decimal past `MAX_DIGITS`.
fn overflow() -> crate::error::SqlError {
    fail(FOAR0002, &format!("a decimal has more than {} digits [XQueryFO §6.2]", super::decimal::MAX_DIGITS))
}

/// `d` if it is within `MAX_DIGITS`.
fn bounded(d: Decimal) -> Result<Decimal> {
    if d.is_too_long() { Err(overflow()) } else { Ok(d) }
}

impl Number {
    /// The number `a` is: a value of a numeric type, or of a type derived
    /// from one, as that type's; an untyped value cast as a double
    /// [XQuery §3.4].
    ///
    /// # Errors
    ///
    /// err:XPTY0004 of a value of another type; err:FORG0001 of an untyped
    /// value that is no double.
    pub fn of(a: &Atomic) -> Result<Number> {
        let real = |text: &str| double(text).ok_or_else(|| wrong_type("a number", &format!("'{text}' is none")));
        let exact = |text: &str| Decimal::parse(text).ok_or_else(|| wrong_type("a number", &format!("'{text}' is none")));
        match a.kind.primitive() {
            Primitive::Integer => Ok(Number::Integer(exact(&a.lexical)?)),
            Primitive::Decimal => Ok(Number::Decimal(exact(&a.lexical)?)),
            Primitive::Float => Ok(Number::Float(round_to_single(real(&a.lexical)?))),
            Primitive::Double => Ok(Number::Double(real(&a.lexical)?)),
            Primitive::Untyped => Ok(Number::Double(real(&cast(a, AtomicType::Double)?.lexical)?)),
            Primitive::String
            | Primitive::Boolean
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
            | Primitive::Notation => Err(wrong_type("a number", &format!("it is an {}", a.kind.name()))),
        }
    }

    /// Its value as an atomic value of its type, its lexical form the
    /// string casting it to `xs:string` gives [XQueryFO §17.1.2].
    ///
    /// # Errors
    ///
    /// err:FOAR0002 of an integer or decimal past `MAX_DIGITS`.
    pub fn atomic(&self) -> Result<Atomic> {
        Ok(match self {
            Number::Integer(d) => Atomic::new(AtomicType::Integer, bounded(d.clone())?.text()),
            Number::Decimal(d) => Atomic::new(AtomicType::Decimal, bounded(d.clone())?.text()),
            Number::Float(x) => Atomic::new(AtomicType::Float, real_text(*x, true)),
            Number::Double(x) => Atomic::new(AtomicType::Double, real_text(*x, false)),
        })
    }

    /// Its type, in the order of promotion.
    fn rank(&self) -> Rank {
        match self {
            Number::Integer(_) => Rank::Integer,
            Number::Decimal(_) => Rank::Decimal,
            Number::Float(_) => Rank::Float,
            Number::Double(_) => Rank::Double,
        }
    }

    /// It promoted to the type `rank`, not below its own: an integer
    /// substituted for a decimal, a decimal promoted to a float or double
    /// directly, a float to a double [XQueryFO §6.2] [XQuery Appendix B.1].
    fn promoted(self, rank: Rank) -> Number {
        match (self, rank) {
            (Number::Integer(d), Rank::Decimal) => Number::Decimal(d),
            (Number::Integer(d) | Number::Decimal(d), Rank::Float) => Number::Float(d.text().parse::<f32>().map_or(f64::NAN, f64::from)),
            (Number::Integer(d) | Number::Decimal(d), Rank::Double) => Number::Double(d.text().parse::<f64>().unwrap_or(f64::NAN)),
            (Number::Float(x), Rank::Double) => Number::Double(x),
            (n, Rank::Integer | Rank::Decimal | Rank::Float | Rank::Double) => n,
        }
    }

    /// The value of a float or double, promoted to a double if it is an
    /// integer or decimal.
    #[must_use]
    pub fn real(&self) -> f64 {
        match self.clone().promoted(Rank::Double) {
            Number::Double(x) | Number::Float(x) => x,
            Number::Integer(_) | Number::Decimal(_) => f64::NAN,
        }
    }
}

/// The numeric types, in the order a number is promoted along [XQuery
/// Appendix B.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// `xs:integer`.
    Integer,
    /// `xs:decimal`.
    Decimal,
    /// `xs:float`.
    Float,
    /// `xs:double`.
    Double,
}

/// Whether `a` is a NaN of a float or double.
pub(super) fn is_nan(a: &Atomic) -> bool {
    match Number::of(a) {
        Ok(Number::Float(x) | Number::Double(x)) => x.is_nan(),
        Ok(Number::Integer(_) | Number::Decimal(_)) | Err(_) => false,
    }
}

/// `a` and `b` promoted to one type, the greater of theirs.
fn promote(a: Number, b: Number) -> (Number, Number) {
    let rank = a.rank().max(b.rank());
    (a.promoted(rank), b.promoted(rank))
}

/// The float `x`, rounded to single precision.
fn single(x: f64) -> Number {
    Number::Float(round_to_single(x))
}

/// The arithmetic operators [XQueryFO §6.2.1 to §6.2.6].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arithmetic {
    Add,
    Subtract,
    Multiply,
    Divide,
    IntegerDivide,
    Modulus,
}

/// `x` `op` `y` of floats or doubles, by IEEE 754 [XQueryFO §6.2]: of
/// `op:numeric-mod` the remainder of a truncating division, of its
/// dividend's sign, NaN of an infinite dividend or a zero divisor
/// [XQueryFO §6.2.6].
fn real_arithmetic(op: Arithmetic, x: f64, y: f64) -> f64 {
    match op {
        Arithmetic::Add => x + y,
        Arithmetic::Subtract => x - y,
        Arithmetic::Multiply => x * y,
        Arithmetic::Divide | Arithmetic::IntegerDivide => x / y,
        Arithmetic::Modulus => x % y,
    }
}

/// `x` `op` `y` of integers or decimals; none of a zero divisor.
fn exact_arithmetic(op: Arithmetic, x: &Decimal, y: &Decimal) -> Option<Decimal> {
    match op {
        Arithmetic::Add => Some(x.add(y)),
        Arithmetic::Subtract => Some(x.subtract(y)),
        Arithmetic::Multiply => Some(x.multiply(y)),
        Arithmetic::Divide => x.quotient(y),
        Arithmetic::IntegerDivide => x.divide(y, 0),
        Arithmetic::Modulus => x.remainder(y),
    }
}

/// The error of a division by zero [XQueryFO §6.2.4, §6.2.5, §6.2.6].
fn by_zero() -> crate::error::SqlError {
    fail(FOAR0001, "a divisor is zero [XQueryFO §6.2.4]")
}

/// `a` `op` `b`, promoted to one type [XQueryFO §6.2]: of two integers an
/// integer, but a decimal of `op:numeric-divide` [XQueryFO §6.2.4].
fn arithmetic(op: Arithmetic, a: &Atomic, b: &Atomic) -> Result<Number> {
    let (left, right) = promote(Number::of(a)?, Number::of(b)?);
    for operand in [&left, &right] {
        if let Number::Integer(digits) | Number::Decimal(digits) = operand {
            bounded(digits.clone())?;
        }
    }
    Ok(match (left, right) {
        (Number::Integer(first), Number::Integer(second)) => {
            let result = exact_arithmetic(op, &first, &second).ok_or_else(by_zero)?;
            if op == Arithmetic::Divide { Number::Decimal(result) } else { Number::Integer(result) }
        }
        (Number::Decimal(first), Number::Decimal(second)) => Number::Decimal(exact_arithmetic(op, &first, &second).ok_or_else(by_zero)?),
        (Number::Float(first), Number::Float(second)) => single(real_arithmetic(op, first, second)),
        (Number::Double(first), Number::Double(second)) => Number::Double(real_arithmetic(op, first, second)),
        (first, second) => return Err(wrong_type("numbers of one type", &format!("{first:?} and {second:?}"))),
    })
}

/// [XQueryFO §6.2.1] `op:numeric-add`: `$arg1 + $arg2`.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number; err:FOAR0002 of a result
/// past `MAX_DIGITS`.
pub fn numeric_add(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    arithmetic(Arithmetic::Add, a, b)?.atomic()
}

/// [XQueryFO §6.2.2] `op:numeric-subtract`: `$arg1 - $arg2`.
///
/// # Errors
///
/// As `numeric_add`.
pub fn numeric_subtract(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    arithmetic(Arithmetic::Subtract, a, b)?.atomic()
}

/// [XQueryFO §6.2.3] `op:numeric-multiply`: `$arg1 * $arg2`.
///
/// # Errors
///
/// As `numeric_add`.
pub fn numeric_multiply(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    arithmetic(Arithmetic::Multiply, a, b)?.atomic()
}

/// [XQueryFO §6.2.4] `op:numeric-divide`: `$arg1 div $arg2`, a decimal of
/// two integers, truncated to `DIVISION_SCALE` digits after the period if
/// it does not end before.
///
/// # Errors
///
/// As `numeric_add`; err:FOAR0001 of an integer or decimal divisor zero.
pub fn numeric_divide(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    arithmetic(Arithmetic::Divide, a, b)?.atomic()
}

/// [XQueryFO §6.2.5] `op:numeric-integer-divide`: `$arg1 idiv $arg2`, the
/// integer of their quotient with its fraction truncated.
///
/// # Errors
///
/// As `numeric_add`; err:FOAR0001 of a divisor zero; err:FOAR0002 of an
/// operand NaN, or a dividend or quotient infinite.
pub fn numeric_integer_divide(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    let integer = match promote(Number::of(a)?, Number::of(b)?) {
        (Number::Float(dividend), Number::Float(divisor)) => real_integer_divide(dividend, divisor, true)?,
        (Number::Double(dividend), Number::Double(divisor)) => real_integer_divide(dividend, divisor, false)?,
        (dividend, divisor) => match arithmetic(Arithmetic::IntegerDivide, &dividend.atomic()?, &divisor.atomic()?)? {
            Number::Integer(d) | Number::Decimal(d) => d,
            Number::Float(_) | Number::Double(_) => return Err(wrong_type("numbers of one type", "they are not")),
        },
    };
    Number::Integer(integer).atomic()
}

/// [XQueryFO §6.2.5] `x idiv y` of floats, if `single`, or doubles: their
/// quotient, of that precision, truncated.
fn real_integer_divide(x: f64, y: f64, single_precision: bool) -> Result<Decimal> {
    if y.classify() == FpCategory::Zero {
        return Err(by_zero());
    }
    let quotient = if single_precision { round_to_single(x / y) } else { x / y };
    if x.is_nan() || y.is_nan() || !x.is_finite() || !quotient.is_finite() {
        return Err(fail(FOAR0002, "an operand of idiv is NaN, or its dividend or quotient infinite [XQueryFO §6.2.5]"));
    }
    Decimal::parse(&format!("{}", quotient.trunc())).ok_or_else(overflow)
}

/// [XQueryFO §6.2.6] `op:numeric-mod`: `$arg1 mod $arg2`, of the sign of
/// the dividend, `(a idiv b) * b + (a mod b)` being `a`.
///
/// # Errors
///
/// As `numeric_add`; err:FOAR0001 of an integer or decimal divisor zero.
pub fn numeric_mod(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    arithmetic(Arithmetic::Modulus, a, b)?.atomic()
}

/// [XQueryFO §6.2.7] `op:numeric-unary-plus`: `+$arg`, its value as its
/// numeric type's.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_unary_plus(a: &Atomic) -> Result<Atomic> {
    Number::of(a)?.atomic()
}

/// [XQueryFO §6.2.8] `op:numeric-unary-minus`: `-$arg`; 0 of 0, -0 of 0
/// of a float or double.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_unary_minus(a: &Atomic) -> Result<Atomic> {
    match Number::of(a)? {
        Number::Integer(d) => Number::Integer(d.negated()),
        Number::Decimal(d) => Number::Decimal(d.negated()),
        Number::Float(x) => Number::Float(-x),
        Number::Double(x) => Number::Double(-x),
    }
    .atomic()
}

/// [XQueryFO §6.3] The order of `a` and `b`, promoted to one type; none if
/// either is NaN.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_order(a: &Atomic, b: &Atomic) -> Result<Option<Ordering>> {
    Ok(match promote(Number::of(a)?, Number::of(b)?) {
        (Number::Integer(first) | Number::Decimal(first), Number::Integer(second) | Number::Decimal(second)) => {
            Some(first.compare(&second))
        }
        (first, second) => first.real().partial_cmp(&second.real()),
    })
}

/// [XQueryFO §6.3.1] `op:numeric-equal`: whether `a` and `b` are equal,
/// zeros of either sign equal, NaN equal to nothing.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(numeric_order(a, b)? == Some(Ordering::Equal))
}

/// [XQueryFO §6.3.2] `op:numeric-less-than`: whether `a` is less than
/// `b`; false if either is NaN.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_less_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(numeric_order(a, b)? == Some(Ordering::Less))
}

/// [XQueryFO §6.3.3] `op:numeric-greater-than`: whether `a` is greater
/// than `b`; false if either is NaN.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no number.
pub fn numeric_greater_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(numeric_order(a, b)? == Some(Ordering::Greater))
}

#[cfg(test)]
mod tests;
