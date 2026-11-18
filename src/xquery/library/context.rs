//! The context functions [XQueryFO §16]: the focus's position and size,
//! the current date and time, the implicit time zone, the default
//! collation and the static base URI.
//!
//! \[DESIGN\] RULES.md D54: the current date and time is the instant the
//! statement read its clock at, the same for every call in it, as
//! [XQueryFO §16.3] requires of a stable function, in the implicit time
//! zone.

use super::args::{context_item, optional};
use super::durations::day_time;
use super::moments::moment_value;
use super::{Builtin, Focus};
use crate::error::Result;
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE};
use crate::xml::{Atomic, AtomicType, Moment, Sequence, Shape};

/// The functions of [XQueryFO §16].
pub(super) const FUNCTIONS: [Builtin; 8] = [
    Builtin::function("position", 0, 0, position),
    Builtin::function("last", 0, 0, last),
    Builtin::function("current-dateTime", 0, 0, |f, _| now(f, Shape::DateTime, AtomicType::DateTime)),
    Builtin::function("current-date", 0, 0, |f, _| now(f, Shape::Date, AtomicType::Date)),
    Builtin::function("current-time", 0, 0, |f, _| now(f, Shape::Time, AtomicType::Time)),
    Builtin::function("implicit-timezone", 0, 0, |f, _| Ok(implicit_timezone(f))),
    Builtin::function("default-collation", 0, 0, |f, _| Ok(optional(Some(Atomic::new(AtomicType::String, f.collation.as_str()))))),
    Builtin::function("static-base-uri", 0, 0, |f, _| Ok(optional(f.base_uri.as_deref().map(|u| Atomic::new(AtomicType::AnyUri, u))))),
];

/// The integer `n`.
fn integer(n: usize) -> Sequence {
    optional(Some(Atomic::new(AtomicType::Integer, n.to_string())))
}

/// [XQueryFO §16.1] `fn:position() as xs:integer`: the context position.
///
/// # Errors
///
/// err:FONC0001 if there is no context item.
fn position(focus: &Focus, _: &[Sequence]) -> Result<Sequence> {
    context_item(focus)?;
    Ok(integer(focus.position))
}

/// [XQueryFO §16.2] `fn:last() as xs:integer`: the context size.
///
/// # Errors
///
/// err:FONC0001 if there is no context item.
fn last(focus: &Focus, _: &[Sequence]) -> Result<Sequence> {
    context_item(focus)?;
    Ok(integer(focus.size))
}

/// [XQueryFO §16.3 to §16.5] `fn:current-dateTime`, `fn:current-date` and
/// `fn:current-time`: the current date and time, or its date or time, in
/// the implicit time zone, of `kind`.
fn now(focus: &Focus, shape: Shape, kind: AtomicType) -> Result<Sequence> {
    let shift = i64::from(focus.timezone_minutes).saturating_mul(MICROS_PER_MINUTE);
    let local = focus.now.utc.saturating_add(shift);
    let days = local.div_euclid(MICROS_PER_DAY);
    let micros = local.rem_euclid(MICROS_PER_DAY);
    let zone = i16::try_from(focus.timezone_minutes).ok();
    let moment = match shape {
        Shape::Date => Moment { shape, days, micros: 0, zone },
        Shape::Time => Moment { shape, days: 0, micros, zone },
        Shape::DateTime | Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth => {
            Moment { shape, days, micros, zone }
        }
    };
    Ok(optional(Some(moment_value(moment, kind)?)))
}

/// [XQueryFO §16.6] `fn:implicit-timezone() as xdt:dayTimeDuration`.
fn implicit_timezone(focus: &Focus) -> Sequence {
    optional(Some(day_time(i128::from(focus.timezone_minutes).saturating_mul(i128::from(MICROS_PER_MINUTE)))))
}

#[cfg(test)]
mod tests;
