//! Dates and times [XQueryFO §10.4, §10.5, §10.7]: their comparisons, a
//! value without a time zone taken to be in the implicit time zone; the
//! components of their local values; and their adjustment to a time zone.
//!
//! \[DESIGN\] RULES.md D54, D86: a value is a `Moment`, its local value
//! days from 0001-01-01 and microseconds within the day, a time's days
//! zero, the reference date [XQueryFO §10.4] being the same for both
//! values compared. A Gregorian value is its starting instant, of the
//! template its type reads [XQueryFO §10.4.17 to §10.4.21]. A value
//! adjusted or computed to a year before 0001 or after 9999 is err:FODT0001
//! [XQueryFO §10.1.1].

use std::cmp::Ordering;

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, as_type, optional, optional_of};
use super::durations::{day_time, micros_of};
use crate::error::Result;
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE, MICROS_PER_SECOND, civil_from_days};
use crate::xml::{Atomic, AtomicType, LAST_YEAR, Moment, Sequence, Shape};

/// The functions of [XQueryFO §10.5.7 to §10.5.21] and [XQueryFO §10.7].
pub(super) const FUNCTIONS: [Builtin; 18] = [
    Builtin::function("year-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Year)),
    Builtin::function("month-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Month)),
    Builtin::function("day-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Day)),
    Builtin::function("hours-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Hours)),
    Builtin::function("minutes-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Minutes)),
    Builtin::function("seconds-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Seconds)),
    Builtin::function("timezone-from-dateTime", 1, 1, |_, a| component(a, AtomicType::DateTime, Field::Zone)),
    Builtin::function("year-from-date", 1, 1, |_, a| component(a, AtomicType::Date, Field::Year)),
    Builtin::function("month-from-date", 1, 1, |_, a| component(a, AtomicType::Date, Field::Month)),
    Builtin::function("day-from-date", 1, 1, |_, a| component(a, AtomicType::Date, Field::Day)),
    Builtin::function("timezone-from-date", 1, 1, |_, a| component(a, AtomicType::Date, Field::Zone)),
    Builtin::function("hours-from-time", 1, 1, |_, a| component(a, AtomicType::Time, Field::Hours)),
    Builtin::function("minutes-from-time", 1, 1, |_, a| component(a, AtomicType::Time, Field::Minutes)),
    Builtin::function("seconds-from-time", 1, 1, |_, a| component(a, AtomicType::Time, Field::Seconds)),
    Builtin::function("timezone-from-time", 1, 1, |_, a| component(a, AtomicType::Time, Field::Zone)),
    Builtin::function("adjust-dateTime-to-timezone", 1, 2, |f, a| adjust_to_timezone(f, a, AtomicType::DateTime)),
    Builtin::function("adjust-date-to-timezone", 1, 2, |f, a| adjust_to_timezone(f, a, AtomicType::Date)),
    Builtin::function("adjust-time-to-timezone", 1, 2, |f, a| adjust_to_timezone(f, a, AtomicType::Time)),
];

/// [XQueryDM §3.3.2] The greatest size of a time zone, 14 hours, in
/// minutes.
const MAX_ZONE: i32 = 14 * 60;
/// Minutes in an hour.
const SIXTY: i64 = 60;
/// The digits of a second's fraction a value keeps: microseconds.
const SECOND_SCALE: usize = 6;

/// A component of a date or time [XQueryFO §10.5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Year,
    Month,
    Day,
    Hours,
    Minutes,
    Seconds,
    Zone,
}

/// The value `a` is, of the date or time type `kind`, as a parameter of
/// that type takes it.
///
/// # Errors
///
/// err:XPTY0004 of a value of another type.
pub(super) fn moment_of(a: &Atomic, kind: AtomicType) -> Result<Moment> {
    let typed = as_type(a.clone(), kind)?;
    let shape = Shape::of(kind.primitive()).ok_or_else(|| super::args::wrong_type("a date or time", kind.name()))?;
    Moment::parse(&typed.lexical, shape)?.ok_or_else(|| super::args::wrong_type(kind.name(), &format!("'{}' is none", typed.lexical)))
}

/// The instant of `m` in UTC, in microseconds, its time zone the implicit
/// `zone` if it has none [XQueryFO §10.4].
pub(super) fn instant(m: &Moment, zone: i32) -> i128 {
    let (local, own) = m.local();
    let minutes = own.map_or(zone, i32::from);
    local.saturating_sub(i128::from(minutes).saturating_mul(i128::from(MICROS_PER_MINUTE)))
}

/// The atomic value of `m`, of `kind`.
///
/// # Errors
///
/// err:FODT0001 if its year is before 0001 or after 9999.
pub(super) fn moment_value(m: Moment, kind: AtomicType) -> Result<Atomic> {
    let year = m.year();
    if m.shape != Shape::Time && (m.days < 0 || year > LAST_YEAR) {
        return Err(error(ErrorCode::FODT0001, &format!("a date's year {year} is outside 0001 to {LAST_YEAR} [XQueryFO §10.1.1]")));
    }
    Ok(Atomic::new(kind, m.text()))
}

/// The order of `a` and `b`, both of `kind`, by their instants in UTC, a
/// value without a time zone in the implicit `zone` [XQueryFO §10.4].
///
/// # Errors
///
/// err:XPTY0004 of an operand of another type.
pub fn moment_order(a: &Atomic, b: &Atomic, kind: AtomicType, zone: i32) -> Result<Ordering> {
    Ok(instant(&moment_of(a, kind)?, zone).cmp(&instant(&moment_of(b, kind)?, zone)))
}

/// [XQueryFO §10.4.8] `op:dateTime-equal`, in the implicit time zone
/// `zone`, in minutes.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:dateTime`.
pub fn date_time_equal(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::DateTime, zone)? == Ordering::Equal)
}

/// [XQueryFO §10.4.9] `op:dateTime-less-than`.
///
/// # Errors
///
/// As `date_time_equal`.
pub fn date_time_less_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::DateTime, zone)? == Ordering::Less)
}

/// [XQueryFO §10.4.10] `op:dateTime-greater-than`.
///
/// # Errors
///
/// As `date_time_equal`.
pub fn date_time_greater_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::DateTime, zone)? == Ordering::Greater)
}

/// [XQueryFO §10.4.11] `op:date-equal`: of their starting instants.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:date`.
pub fn date_equal(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Date, zone)? == Ordering::Equal)
}

/// [XQueryFO §10.4.12] `op:date-less-than`.
///
/// # Errors
///
/// As `date_equal`.
pub fn date_less_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Date, zone)? == Ordering::Less)
}

/// [XQueryFO §10.4.13] `op:date-greater-than`.
///
/// # Errors
///
/// As `date_equal`.
pub fn date_greater_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Date, zone)? == Ordering::Greater)
}

/// [XQueryFO §10.4.14] `op:time-equal`: on one reference date.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:time`.
pub fn time_equal(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Time, zone)? == Ordering::Equal)
}

/// [XQueryFO §10.4.15] `op:time-less-than`.
///
/// # Errors
///
/// As `time_equal`.
pub fn time_less_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Time, zone)? == Ordering::Less)
}

/// [XQueryFO §10.4.16] `op:time-greater-than`.
///
/// # Errors
///
/// As `time_equal`.
pub fn time_greater_than(a: &Atomic, b: &Atomic, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, AtomicType::Time, zone)? == Ordering::Greater)
}

/// [XQueryFO §10.4.17 to §10.4.21] `op:gYearMonth-equal`, `op:gYear-equal`,
/// `op:gMonthDay-equal`, `op:gMonth-equal` and `op:gDay-equal`: whether
/// the starting instants of `a` and `b`, of the Gregorian type `kind`, are
/// equal.
///
/// # Errors
///
/// err:XPTY0004 of an operand of another type.
pub fn gregorian_equal(a: &Atomic, b: &Atomic, kind: AtomicType, zone: i32) -> Result<bool> {
    Ok(moment_order(a, b, kind, zone)? == Ordering::Equal)
}

/// The integer `n`.
fn integer(n: i64) -> Atomic {
    Atomic::new(AtomicType::Integer, n.to_string())
}

/// [XQueryFO §10.5.7 to §10.5.21] The component `field` of the local
/// value of `args`'s first, of `kind`; the empty sequence of the empty
/// sequence, and of a time zone it has none of.
fn component(args: &[Sequence], kind: AtomicType, field: Field) -> Result<Sequence> {
    let Some(a) = optional_of(arg(args, 0), kind)? else { return Ok(Sequence::empty()) };
    let m = moment_of(&a, kind)?;
    let (year, month, day) = civil_from_days(m.days);
    let minutes = m.micros.checked_div(MICROS_PER_MINUTE).unwrap_or(0);
    Ok(optional(match field {
        Field::Year => Some(integer(year)),
        Field::Month => Some(integer(month)),
        Field::Day => Some(integer(day)),
        Field::Hours => Some(integer(minutes.checked_div(SIXTY).unwrap_or(0))),
        Field::Minutes => Some(integer(minutes.checked_rem(SIXTY).unwrap_or(0))),
        Field::Seconds => Some(seconds(m.micros.checked_rem(MICROS_PER_MINUTE).unwrap_or(0))),
        Field::Zone => m.zone.map(|z| day_time(i128::from(z).saturating_mul(i128::from(MICROS_PER_MINUTE)))),
    }))
}

/// The `xs:decimal` seconds of `micros`.
fn seconds(micros: i64) -> Atomic {
    let whole = super::decimal::Decimal::integer(i128::from(micros));
    let value = whole.divide(&super::decimal::Decimal::integer(i128::from(MICROS_PER_SECOND)), SECOND_SCALE).unwrap_or(whole);
    Atomic::new(AtomicType::Decimal, value.text())
}

/// The time zone, in minutes, of the `xdt:dayTimeDuration` `a`.
///
/// # Errors
///
/// err:FODT0003 if it is more than 14 hours either way, or not of whole
/// minutes [XQueryFO §10.7.1].
pub(super) fn zone_of(a: &Atomic) -> Result<i32> {
    let micros = micros_of(a)?;
    let minutes = micros.checked_div(i128::from(MICROS_PER_MINUTE)).and_then(|m| i32::try_from(m).ok());
    let whole = micros.checked_rem(i128::from(MICROS_PER_MINUTE)) == Some(0);
    minutes
        .filter(|m| whole && m.abs() <= MAX_ZONE)
        .ok_or_else(|| error(ErrorCode::FODT0003, &format!("{} is no time zone [XQueryFO §10.7.1]", a.lexical)))
}

/// `m` adjusted to the time zone `zone`, or to none [XQueryFO §10.7.1]:
/// given a zone if it has none; of its local value if `zone` is none; else
/// the same instant in `zone`, a date's or time's other components as
/// they come out [XQueryFO §10.7.2, §10.7.3].
pub(super) fn adjusted(m: Moment, zone: Option<i32>) -> Moment {
    let (Some(own), Some(to)) = (m.zone, zone) else {
        return Moment { zone: zone.and_then(|z| i16::try_from(z).ok()), ..m };
    };
    let shift = i64::from(to).saturating_sub(i64::from(own)).saturating_mul(MICROS_PER_MINUTE);
    shifted(Moment { zone: i16::try_from(to).ok(), ..m }, i128::from(shift))
}

/// `m` with `micros` added to its local value: a date's time after it
/// dropped, a time's days.
pub(super) fn shifted(m: Moment, micros: i128) -> Moment {
    let day = i128::from(MICROS_PER_DAY);
    let total = i128::from(m.days).saturating_mul(day).saturating_add(i128::from(m.micros)).saturating_add(micros);
    let days = i64::try_from(total.div_euclid(day)).unwrap_or(i64::MIN);
    let within = i64::try_from(total.rem_euclid(day)).unwrap_or(0);
    match m.shape {
        Shape::Time => Moment { days: 0, micros: within, ..m },
        Shape::Date => Moment { days, micros: 0, ..m },
        Shape::DateTime | Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth => {
            Moment { days, micros: within, ..m }
        }
    }
}

/// [XQueryFO §10.7.1 to §10.7.3] `fn:adjust-dateTime-to-timezone`,
/// `fn:adjust-date-to-timezone` and `fn:adjust-time-to-timezone`, of
/// `kind`: the value adjusted to the zone given, or to none if it is the
/// empty sequence, or to the implicit time zone if none is given.
///
/// # Errors
///
/// err:FODT0003 of a zone that is none; err:FODT0001 of a date outside
/// 0001 to 9999.
fn adjust_to_timezone(focus: &Context<'_>, args: &[Sequence], kind: AtomicType) -> Result<Sequence> {
    let zone = match args.get(1) {
        Some(given) => optional_of(given, AtomicType::DayTimeDuration)?.map(|z| zone_of(&z)).transpose()?,
        None => Some(i32::from(focus.timezone())),
    };
    let Some(a) = optional_of(arg(args, 0), kind)? else { return Ok(Sequence::empty()) };
    Ok(optional(Some(moment_value(adjusted(moment_of(&a, kind)?, zone), kind)?)))
}

#[cfg(test)]
mod tests;
