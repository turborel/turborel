//! `fn:dateTime` [XQueryFO §5.2], a date and time of a date and a time.
//! The constructor functions of the atomic types [XQueryFO §5.1] are a
//! query's casts, `$arg cast as xs:TYPE?` [XQuery §3.12.5], which the
//! analysis makes of a call of one.

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, one_of, single};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Moment, Sequence, Shape};

/// The functions of [XQueryFO §5.2].
pub(super) const FUNCTIONS: [Builtin; 1] = [Builtin::function("dateTime", 2, 2, date_time)];

/// The date, time or date and time of `shape` that `a`, of its type, is.
fn moment(a: &Atomic, shape: Shape) -> Result<Moment> {
    Moment::parse(&a.lexical, shape)?.ok_or_else(|| super::args::wrong_type("a date or time", &format!("'{}' is none", a.lexical)))
}

/// [XQueryFO §5.2] `fn:dateTime($arg1 as xs:date, $arg2 as xs:time) as
/// xs:dateTime`: the date and time of the date of `$arg1` and the time of
/// `$arg2`, of the time zone either has, or none if neither has.
///
/// # Errors
///
/// err:XPTY0004 of an argument not a date or a time; err:FORG0008 if they
/// have different time zones.
fn date_time(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let date = moment(&one_of(arg(args, 0), AtomicType::Date)?, Shape::Date)?;
    let time = moment(&one_of(arg(args, 1), AtomicType::Time)?, Shape::Time)?;
    if let (Some(d), Some(t)) = (date.zone, time.zone)
        && d != t
    {
        return Err(error(ErrorCode::FORG0008, "the date and the time have different time zones [XQueryFO §5.2]"));
    }
    let joined = Moment { shape: Shape::DateTime, days: date.days, micros: time.micros, zone: date.zone.or(time.zone) };
    Ok(single(Atomic::new(AtomicType::DateTime, joined.text())))
}

#[cfg(test)]
mod tests;
