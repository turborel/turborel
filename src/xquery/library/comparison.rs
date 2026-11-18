//! The value comparison of two atomic values [XQuery §3.5.1], by the
//! operators [XQuery Appendix B.2] maps it to: numbers by
//! `op:numeric-equal`, strings by a collation, booleans, durations, dates
//! and times in the implicit time zone, binary values, QNames and NOTATION
//! [XQueryFO §6.3, §7.3, §9.2, §10.4, §11.2.1, §12.1, §13.1]. It is what
//! `fn:index-of`, `fn:distinct-values`, `fn:deep-equal`, `fn:max` and
//! `fn:min` compare by [XQueryFO §15]; an untyped value is compared as a
//! string, as `fn:distinct-values` says [XQueryFO §15.1.6].

use std::cmp::Ordering;

use super::binary::{base64_binary_equal, hex_binary_equal, notation_equal};
use super::booleans::{boolean_equal, boolean_less_than};
use super::collation::Collator;
use super::durations::{duration_equal, duration_order};
use super::moments::moment_order;
use super::numeric::numeric_order;
use super::qnames::qname_equal;
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Primitive};

/// What a value is compared as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A number of any numeric type.
    Number,
    /// A string, an untyped value or a URI.
    Text,
    /// A boolean.
    Truth,
    /// A year-month duration, ordered.
    YearMonth,
    /// A day-time duration, ordered.
    DayTime,
    /// Another duration, of equality alone.
    Duration,
    /// A date, time or Gregorian value of the type given.
    Moment(AtomicType),
    /// An `xs:hexBinary`.
    Hex,
    /// An `xs:base64Binary`.
    Base64,
    /// A QName.
    QName,
    /// A NOTATION.
    Notation,
}

/// The class of `a`.
fn class(a: &Atomic) -> Class {
    let p = a.kind.primitive();
    match p {
        Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double => Class::Number,
        Primitive::String | Primitive::Untyped | Primitive::AnyUri => Class::Text,
        Primitive::Boolean => Class::Truth,
        Primitive::YearMonthDuration => Class::YearMonth,
        Primitive::DayTimeDuration => Class::DayTime,
        Primitive::Duration => Class::Duration,
        Primitive::DateTime => Class::Moment(AtomicType::DateTime),
        Primitive::Date => Class::Moment(AtomicType::Date),
        Primitive::Time => Class::Moment(AtomicType::Time),
        Primitive::GYearMonth => Class::Moment(AtomicType::GYearMonth),
        Primitive::GYear => Class::Moment(AtomicType::GYear),
        Primitive::GMonthDay => Class::Moment(AtomicType::GMonthDay),
        Primitive::GDay => Class::Moment(AtomicType::GDay),
        Primitive::GMonth => Class::Moment(AtomicType::GMonth),
        Primitive::HexBinary => Class::Hex,
        Primitive::Base64Binary => Class::Base64,
        Primitive::QName => Class::QName,
        Primitive::Notation => Class::Notation,
    }
}

/// Whether `x` and `y` are compared with each other: of one class, any
/// durations together for equality.
fn comparable(x: Class, y: Class) -> bool {
    let duration = |c| matches!(c, Class::YearMonth | Class::DayTime | Class::Duration);
    x == y || (duration(x) && duration(y))
}

/// The string `a` is compared as.
fn text(a: &Atomic) -> &str {
    &a.lexical
}

/// Whether `a eq b` [XQuery §3.5.1]; none if `eq` is not defined for their
/// types, so they are distinct [XQueryFO §15.1.3, §15.1.6]. NaN equals
/// nothing.
///
/// # Errors
///
/// As the operators compared by.
pub fn value_equal(a: &Atomic, b: &Atomic, collation: &Collator, zone: i32) -> Result<Option<bool>> {
    let (x, y) = (class(a), class(b));
    if !comparable(x, y) {
        return Ok(None);
    }
    Ok(Some(match x {
        Class::Number => numeric_order(a, b)? == Some(Ordering::Equal),
        Class::Text => collation.equal(text(a), text(b)),
        Class::Truth => boolean_equal(a, b)?,
        Class::YearMonth | Class::DayTime | Class::Duration => duration_equal(a, b)?,
        Class::Moment(kind) => moment_order(a, b, kind, zone)? == Ordering::Equal,
        Class::Hex => hex_binary_equal(a, b)?,
        Class::Base64 => base64_binary_equal(a, b)?,
        Class::QName => qname_equal(a, b, collation)?,
        Class::Notation => notation_equal(a, b)?,
    }))
}

/// The order of `a` and `b` by `lt` and `gt` [XQuery §3.5.1]; none if
/// they are not ordered: of types not ordered together, or a NaN.
///
/// # Errors
///
/// As the operators compared by.
pub fn value_order(a: &Atomic, b: &Atomic, collation: &Collator, zone: i32) -> Result<Option<Ordering>> {
    let (x, y) = (class(a), class(b));
    if x != y {
        return Ok(None);
    }
    Ok(match x {
        Class::Number => numeric_order(a, b)?,
        Class::Text => Some(collation.compare(text(a), text(b))),
        Class::Truth => Some(if boolean_equal(a, b)? {
            Ordering::Equal
        } else if boolean_less_than(a, b)? {
            Ordering::Less
        } else {
            Ordering::Greater
        }),
        Class::YearMonth | Class::DayTime => Some(duration_order(a, b)?),
        Class::Moment(kind) => ordered_moment(kind).then(|| moment_order(a, b, kind, zone)).transpose()?,
        Class::Duration | Class::Hex | Class::Base64 | Class::QName | Class::Notation => None,
    })
}

/// Whether values of the date or time type `kind` are ordered: a date and
/// time, a date or a time; a Gregorian value has equality alone [XQueryFO
/// §10.1].
fn ordered_moment(kind: AtomicType) -> bool {
    matches!(kind, AtomicType::DateTime | AtomicType::Date | AtomicType::Time)
}

#[cfg(test)]
mod tests;
