//! Durations [XQueryFO §10.3 to §10.6]: the comparisons of durations, the
//! components of the two totally ordered ones, and their arithmetic. An
//! `xdt:yearMonthDuration` is a number of months, an `xdt:dayTimeDuration`
//! a number of seconds, here microseconds [XQueryFO §10.3] (RULES.md D86).
//!
//! \[DESIGN\] RULES.md D86: a duration multiplied or divided by a double
//! is reckoned exactly, the double as the shortest decimal that reads back
//! as it, then rounded to the month or microsecond, a half up [XQueryFO
//! §10.6.3]; a result past 128 bits of months or microseconds overflows,
//! err:FODT0002 [XQueryFO §10.1.1].

use std::cmp::Ordering;

use super::args::{arg, as_type, optional, optional_of};
use super::codes::{FOCA0005, FODT0002, fail};
use super::decimal::{Decimal, Rounding};
use super::numeric::Number;
use super::{Builtin, Focus};
use crate::error::{Result, SqlError};
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_SECOND};
use crate::xml::{Atomic, AtomicType, Sequence, day_time_text, duration_value, year_month_text};

/// The functions of [XQueryFO §10.5.1 to §10.5.6].
pub(super) const FUNCTIONS: [Builtin; 6] = [
    Builtin::function("years-from-duration", 1, 1, years_from_duration),
    Builtin::function("months-from-duration", 1, 1, months_from_duration),
    Builtin::function("days-from-duration", 1, 1, days_from_duration),
    Builtin::function("hours-from-duration", 1, 1, hours_from_duration),
    Builtin::function("minutes-from-duration", 1, 1, minutes_from_duration),
    Builtin::function("seconds-from-duration", 1, 1, seconds_from_duration),
];

/// Months in a year.
const MONTHS: i128 = 12;
/// Microseconds in an hour and in a minute.
const MICROS_PER_HOUR: i128 = 3_600_000_000;
const MICROS_PER_MINUTE: i128 = 60_000_000;
/// The digits of a second's fraction a duration keeps: microseconds.
const SECOND_SCALE: usize = 6;

/// The error of a duration too large to hold [XQueryFO §10.1.1].
fn overflow() -> SqlError {
    fail(FODT0002, "a duration is too large to hold [XQueryFO §10.1.1]")
}

/// The months of `a`, as a parameter of `xdt:yearMonthDuration` takes it.
///
/// # Errors
///
/// err:XPTY0004 of a value of another type.
pub(super) fn months_of(a: &Atomic) -> Result<i128> {
    let typed = as_type(a.clone(), AtomicType::YearMonthDuration)?;
    Ok(duration_value(&typed).map_or(0, |(months, _)| months))
}

/// The microseconds of `a`, as a parameter of `xdt:dayTimeDuration` takes
/// it.
///
/// # Errors
///
/// err:XPTY0004 of a value of another type.
pub(super) fn micros_of(a: &Atomic) -> Result<i128> {
    let typed = as_type(a.clone(), AtomicType::DayTimeDuration)?;
    Ok(duration_value(&typed).map_or(0, |(_, micros)| micros))
}

/// The `xdt:yearMonthDuration` of `months` [XQueryFO §10.3.1.3].
#[must_use]
pub fn year_month(months: i128) -> Atomic {
    Atomic::new(AtomicType::YearMonthDuration, year_month_text(months))
}

/// The `xdt:dayTimeDuration` of `micros` [XQueryFO §10.3.2.3].
#[must_use]
pub fn day_time(micros: i128) -> Atomic {
    Atomic::new(AtomicType::DayTimeDuration, day_time_text(micros))
}

/// [XQueryFO §10.4.1] `op:yearMonthDuration-equal`.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xdt:yearMonthDuration`.
pub fn year_month_duration_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(months_of(a)? == months_of(b)?)
}

/// [XQueryFO §10.4.2] `op:yearMonthDuration-less-than`.
///
/// # Errors
///
/// As `year_month_duration_equal`.
pub fn year_month_duration_less_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(months_of(a)? < months_of(b)?)
}

/// [XQueryFO §10.4.3] `op:yearMonthDuration-greater-than`.
///
/// # Errors
///
/// As `year_month_duration_equal`.
pub fn year_month_duration_greater_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(months_of(a)? > months_of(b)?)
}

/// [XQueryFO §10.4.4] `op:dayTimeDuration-equal`.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xdt:dayTimeDuration`.
pub fn day_time_duration_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(micros_of(a)? == micros_of(b)?)
}

/// [XQueryFO §10.4.5] `op:dayTimeDuration-less-than`.
///
/// # Errors
///
/// As `day_time_duration_equal`.
pub fn day_time_duration_less_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(micros_of(a)? < micros_of(b)?)
}

/// [XQueryFO §10.4.6] `op:dayTimeDuration-greater-than`.
///
/// # Errors
///
/// As `day_time_duration_equal`.
pub fn day_time_duration_greater_than(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(micros_of(a)? > micros_of(b)?)
}

/// The months and microseconds of `a`, a duration of any of the three
/// types.
fn units(a: &Atomic) -> Result<(i128, i128)> {
    let typed = as_type(a.clone(), AtomicType::Duration)?;
    Ok(duration_value(&typed).unwrap_or_default())
}

/// [XQueryFO §10.4.7.1] `op:duration-equal`: whether the year-month and
/// day-time parts of `a` and `b` are equal.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no duration.
pub fn duration_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(units(a)? == units(b)?)
}

/// The order of two durations of one of the totally ordered types, the
/// order the value comparisons of durations read.
///
/// # Errors
///
/// err:XPTY0004 unless both are year-month or both day-time durations.
pub fn duration_order(a: &Atomic, b: &Atomic) -> Result<Ordering> {
    if a.kind.derives_from(AtomicType::YearMonthDuration) {
        Ok(months_of(a)?.cmp(&months_of(b)?))
    } else {
        Ok(micros_of(a)?.cmp(&micros_of(b)?))
    }
}

/// [XQueryFO §10.6.1] `op:add-yearMonthDurations`.
///
/// # Errors
///
/// err:XPTY0004 of an operand of another type; err:FODT0002 of a sum too
/// large to hold.
pub fn add_year_month_durations(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    months_of(a)?.checked_add(months_of(b)?).map(year_month).ok_or_else(overflow)
}

/// [XQueryFO §10.6.2] `op:subtract-yearMonthDurations`.
///
/// # Errors
///
/// As `add_year_month_durations`.
pub fn subtract_year_month_durations(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    months_of(a)?.checked_sub(months_of(b)?).map(year_month).ok_or_else(overflow)
}

/// [XQueryFO §10.6.6] `op:add-dayTimeDurations`.
///
/// # Errors
///
/// As `add_year_month_durations`, of day-time durations.
pub fn add_day_time_durations(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    micros_of(a)?.checked_add(micros_of(b)?).map(day_time).ok_or_else(overflow)
}

/// [XQueryFO §10.6.7] `op:subtract-dayTimeDurations`.
///
/// # Errors
///
/// As `add_day_time_durations`.
pub fn subtract_day_time_durations(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    micros_of(a)?.checked_sub(micros_of(b)?).map(day_time).ok_or_else(overflow)
}

/// The double `b`, a number promoted, as an exact decimal; none of an
/// infinity.
///
/// # Errors
///
/// err:XPTY0004 of a value that is no number; err:FOCA0005 of NaN
/// [XQueryFO §10.6.3].
fn factor(b: &Atomic) -> Result<Option<Decimal>> {
    let x = Number::of(b)?.real();
    if x.is_nan() {
        return Err(fail(FOCA0005, "a duration is multiplied or divided by NaN [XQueryFO §10.6.3]"));
    }
    // Display writes the shortest decimal that reads back as the double.
    Ok(x.is_finite().then(|| Decimal::parse(&x.to_string())).flatten())
}

/// `units` times, or divided by if `divide`, the double `b`, rounded to
/// a unit, a half up.
fn scaled(units: i128, b: &Atomic, divide: bool) -> Result<i128> {
    let product = match (factor(b)?, divide) {
        (None, true) => Decimal::zero(),
        (None, false) => return Err(overflow()),
        (Some(x), false) => Decimal::integer(units).multiply(&x),
        (Some(x), true) => Decimal::integer(units).divide(&x, SECOND_SCALE).ok_or_else(overflow)?,
    };
    product.round(0, Rounding::HalfUp).to_i128().ok_or_else(overflow)
}

/// [XQueryFO §10.6.3] `op:multiply-yearMonthDuration`: `$arg1 * $arg2`,
/// rounded to the nearest month, a half up.
///
/// # Errors
///
/// err:FOCA0005 of NaN; err:FODT0002 of an infinite factor or a result
/// too large.
pub fn multiply_year_month_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    Ok(year_month(scaled(months_of(a)?, b, false)?))
}

/// [XQueryFO §10.6.4] `op:divide-yearMonthDuration`: `$arg1 div $arg2`,
/// rounded to the nearest month; zero of an infinite divisor.
///
/// # Errors
///
/// err:FOCA0005 of NaN; err:FODT0002 of a zero divisor or a result too
/// large.
pub fn divide_year_month_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    Ok(year_month(scaled(months_of(a)?, b, true)?))
}

/// [XQueryFO §10.6.5] `op:divide-yearMonthDuration-by-yearMonthDuration`:
/// their months divided as `op:numeric-divide` divides integers, a
/// decimal.
///
/// # Errors
///
/// err:FOAR0001 of a zero divisor.
pub fn divide_year_month_duration_by_year_month_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    quotient(months_of(a)?, months_of(b)?)
}

/// [XQueryFO §10.6.8] `op:multiply-dayTimeDuration`: `$arg1 * $arg2`, to
/// the nearest microsecond.
///
/// # Errors
///
/// As `multiply_year_month_duration`.
pub fn multiply_day_time_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    Ok(day_time(scaled(micros_of(a)?, b, false)?))
}

/// [XQueryFO §10.6.9] `op:divide-dayTimeDuration`: `$arg1 div $arg2`, to
/// the nearest microsecond.
///
/// # Errors
///
/// As `divide_year_month_duration`.
pub fn divide_day_time_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    Ok(day_time(scaled(micros_of(a)?, b, true)?))
}

/// [XQueryFO §10.6.10] `op:divide-dayTimeDuration-by-dayTimeDuration`:
/// their seconds divided as `op:numeric-divide` divides decimals.
///
/// # Errors
///
/// err:FOAR0001 of a zero divisor.
pub fn divide_day_time_duration_by_day_time_duration(a: &Atomic, b: &Atomic) -> Result<Atomic> {
    quotient(micros_of(a)?, micros_of(b)?)
}

/// `a` divided by `b` as `op:numeric-divide` divides decimals.
fn quotient(a: i128, b: i128) -> Result<Atomic> {
    let integer = |n: i128| Atomic::new(AtomicType::Decimal, n.to_string());
    super::numeric::numeric_divide(&integer(a), &integer(b))
}

/// The argument of a function of [XQueryFO §10.5.1 to §10.5.6], of
/// `kind`; none of the empty sequence.
fn duration_arg(args: &[Sequence], kind: AtomicType) -> Result<Option<i128>> {
    let Some(a) = optional_of(arg(args, 0), kind)? else { return Ok(None) };
    Ok(Some(if kind == AtomicType::YearMonthDuration { months_of(&a)? } else { micros_of(&a)? }))
}

/// An `xs:integer` of `n`.
fn integer(n: i128) -> Sequence {
    optional(Some(Atomic::new(AtomicType::Integer, n.to_string())))
}

/// [XQueryFO §10.5.1] `fn:years-from-duration($arg as
/// xdt:yearMonthDuration?) as xs:integer?`: the years of its canonical
/// form, of its sign.
fn years_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(duration_arg(args, AtomicType::YearMonthDuration)?.map_or_else(Sequence::empty, |m| integer(m.checked_div(MONTHS).unwrap_or(0))))
}

/// [XQueryFO §10.5.2] `fn:months-from-duration($arg as
/// xdt:yearMonthDuration?) as xs:integer?`: the months of its canonical
/// form, of its sign.
fn months_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(duration_arg(args, AtomicType::YearMonthDuration)?.map_or_else(Sequence::empty, |m| integer(m.checked_rem(MONTHS).unwrap_or(0))))
}

/// The component of `micros`, of its canonical form [XQueryFO §10.3.2.3]:
/// whole `unit`s below `whole`, of its sign.
fn component(micros: i128, unit: i128, whole: i128) -> Sequence {
    integer(micros.checked_rem(whole).unwrap_or(0).checked_div(unit).unwrap_or(0))
}

/// [XQueryFO §10.5.3] `fn:days-from-duration($arg as
/// xdt:dayTimeDuration?) as xs:integer?`.
fn days_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let day = i128::from(MICROS_PER_DAY);
    Ok(duration_arg(args, AtomicType::DayTimeDuration)?.map_or_else(Sequence::empty, |u| component(u, day, i128::MAX)))
}

/// [XQueryFO §10.5.4] `fn:hours-from-duration($arg as
/// xdt:dayTimeDuration?) as xs:integer?`.
fn hours_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let day = i128::from(MICROS_PER_DAY);
    Ok(duration_arg(args, AtomicType::DayTimeDuration)?.map_or_else(Sequence::empty, |u| component(u, MICROS_PER_HOUR, day)))
}

/// [XQueryFO §10.5.5] `fn:minutes-from-duration($arg as
/// xdt:dayTimeDuration?) as xs:integer?`.
fn minutes_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(duration_arg(args, AtomicType::DayTimeDuration)?.map_or_else(Sequence::empty, |u| component(u, MICROS_PER_MINUTE, MICROS_PER_HOUR)))
}

/// [XQueryFO §10.5.6] `fn:seconds-from-duration($arg as
/// xdt:dayTimeDuration?) as xs:decimal?`: the seconds of its canonical
/// form, with their fraction, of its sign.
fn seconds_from_duration(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let Some(micros) = duration_arg(args, AtomicType::DayTimeDuration)? else { return Ok(Sequence::empty()) };
    let within = micros.checked_rem(MICROS_PER_MINUTE).unwrap_or(0);
    let seconds = Decimal::integer(within).divide(&Decimal::integer(i128::from(MICROS_PER_SECOND)), SECOND_SCALE);
    Ok(optional(seconds.map(|s| Atomic::new(AtomicType::Decimal, s.text()))))
}

#[cfg(test)]
mod tests;
