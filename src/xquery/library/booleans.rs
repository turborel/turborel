//! Booleans [XQueryFO §9]: `fn:true`, `fn:false`, `fn:not` and their
//! operators, and the effective boolean value of a sequence, `fn:boolean`
//! [XQueryFO §15.1.1].

use std::num::FpCategory;

use super::args::{arg, as_type, boolean};
use super::codes::{FORG0006, fail};
use super::numeric::Number;
use super::{Builtin, Focus};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Primitive, Sequence};

/// The functions of [XQueryFO §9] and `fn:boolean`.
pub(super) const FUNCTIONS: [Builtin; 4] = [
    Builtin::function("true", 0, 0, |_, _| Ok(boolean(true))),
    Builtin::function("false", 0, 0, |_, _| Ok(boolean(false))),
    Builtin::function("not", 1, 1, not),
    Builtin::function("boolean", 1, 1, boolean_of),
];

/// The truth value of `a`, an `xs:boolean`.
fn truth(a: &Atomic) -> Result<bool> {
    Ok(matches!(&*as_type(a.clone(), AtomicType::Boolean)?.lexical, "true" | "1"))
}

/// [XQueryFO §9.2.1] `op:boolean-equal`.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:boolean`.
pub fn boolean_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(truth(a)? == truth(b)?)
}

/// [XQueryFO §9.2.2] `op:boolean-less-than`: false less than true.
///
/// # Errors
///
/// As `boolean_equal`.
pub fn boolean_less_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(!truth(a)? && truth(b)?)
}

/// [XQueryFO §9.2.3] `op:boolean-greater-than`.
///
/// # Errors
///
/// As `boolean_equal`.
pub fn boolean_greater_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(truth(a)? && !truth(b)?)
}

/// [XQueryFO §15.1.1] The effective boolean value of `value`: false of
/// the empty sequence; true of one whose first item is a node; of one
/// boolean itself; of one string, untyped value or URI whether it is not
/// zero-length; of one number whether it is neither zero nor NaN.
///
/// # Errors
///
/// err:FORG0006 of any other sequence.
pub fn effective_boolean_value(value: &Sequence) -> Result<bool> {
    let a = match value.items() {
        [] => return Ok(false),
        [Item::Node(_), ..] => return Ok(true),
        [Item::Atomic(a)] => a,
        [Item::Atomic(_), _, ..] => return Err(no_value("a sequence of more than one atomic value")),
    };
    match a.kind.primitive() {
        Primitive::Boolean => truth(a),
        Primitive::String | Primitive::Untyped | Primitive::AnyUri => Ok(!a.lexical.is_empty()),
        Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double => {
            let n = Number::of(a)?;
            Ok(match n {
                Number::Integer(d) | Number::Decimal(d) => !d.is_zero(),
                Number::Float(x) | Number::Double(x) => !matches!(x.classify(), FpCategory::Zero | FpCategory::Nan),
            })
        }
        Primitive::Duration
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
        | Primitive::QName
        | Primitive::Notation => Err(no_value(&format!("an {}", a.kind.name()))),
    }
}

/// The error of a sequence of no effective boolean value, `what` it is.
fn no_value(what: &str) -> crate::error::SqlError {
    fail(FORG0006, &format!("{what} has no effective boolean value [XQueryFO §15.1.1]"))
}

/// [XQueryFO §15.1.1] `fn:boolean($arg as item()*) as xs:boolean`.
///
/// # Errors
///
/// err:FORG0006 of a sequence of no effective boolean value.
fn boolean_of(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(boolean(effective_boolean_value(arg(args, 0))?))
}

/// [XQueryFO §9.3.1] `fn:not($arg as item()*) as xs:boolean`: the
/// negation of its effective boolean value.
///
/// # Errors
///
/// As `boolean_of`.
fn not(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(boolean(!effective_boolean_value(arg(args, 0))?))
}

#[cfg(test)]
mod tests;
