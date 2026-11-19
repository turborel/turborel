//! The arithmetic of dates and times [XQueryFO §10.8]: the difference of
//! two, and a duration added to or subtracted from one by the algorithm
//! of [Schema2 Appendix E], the months first, the day kept within its
//! month, then the days and time; a result keeps its operand's time zone,
//! or its having none.

use super::durations::{day_time, micros_of, months_of};
use super::moments::{instant, moment_of, moment_value, shifted};
use crate::error::Result;
use crate::value::datetime::{civil_from_days, days_from_civil, days_in};
use crate::xml::{Atomic, AtomicType, Moment};

/// Months in a year.
const MONTHS: i64 = 12;

/// [XQueryFO §10.8.1 to §10.8.3] The difference of two values of `kind`,
/// their instants in UTC, a value without a time zone in the implicit
/// `zone` [XQueryFO §10.4], as an `xdt:dayTimeDuration`.
fn difference(a: &Atomic, b: &Atomic, kind: AtomicType, zone: i32) -> Result<Atomic> {
    let (x, y) = (instant(&moment_of(a, kind)?, zone), instant(&moment_of(b, kind)?, zone));
    Ok(day_time(x.saturating_sub(y)))
}

/// [XQueryFO §10.8.1] `op:subtract-dateTimes`, in the implicit time zone
/// `zone`, in minutes.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:dateTime`.
pub fn subtract_date_times(a: &Atomic, b: &Atomic, zone: i32) -> Result<Atomic> {
    difference(a, b, AtomicType::DateTime, zone)
}

/// [XQueryFO §10.8.2] `op:subtract-dates`: of their starting instants.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:date`.
pub fn subtract_dates(a: &Atomic, b: &Atomic, zone: i32) -> Result<Atomic> {
    difference(a, b, AtomicType::Date, zone)
}

/// [XQueryFO §10.8.3] `op:subtract-times`: on one reference date.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:time`.
pub fn subtract_times(a: &Atomic, b: &Atomic, zone: i32) -> Result<Atomic> {
    difference(a, b, AtomicType::Time, zone)
}

/// [Schema2 Appendix E] `m` with `months` added: its year and month
/// moved, its day kept within the month it lands in.
fn plus_months(m: Moment, months: i128) -> Moment {
    let (from_year, from_month, day) = civil_from_days(m.days);
    let before = i128::from(from_month.saturating_sub(1));
    let total = i128::from(from_year).saturating_mul(i128::from(MONTHS)).saturating_add(before).saturating_add(months);
    let year = i64::try_from(total.div_euclid(i128::from(MONTHS))).unwrap_or(i64::MAX);
    let month = i64::try_from(total.rem_euclid(i128::from(MONTHS))).unwrap_or(0).saturating_add(1);
    Moment { days: days_from_civil(year, month, day.min(days_in(year, month))), ..m }
}

/// `a`, of `kind`, with the year-month duration `d` added, negated if
/// `minus`.
fn with_months(a: &Atomic, d: &Atomic, kind: AtomicType, minus: bool) -> Result<Atomic> {
    let months = months_of(d)?;
    let months = if minus { months.saturating_neg() } else { months };
    moment_value(plus_months(moment_of(a, kind)?, months), kind)
}

/// `a`, of `kind`, with the day-time duration `d` added, negated if
/// `minus`: a time's days dropped, a date's time.
fn with_micros(a: &Atomic, d: &Atomic, kind: AtomicType, minus: bool) -> Result<Atomic> {
    let micros = micros_of(d)?;
    moment_value(shifted(moment_of(a, kind)?, if minus { micros.saturating_neg() } else { micros }), kind)
}

/// [XQueryFO §10.8.4] `op:add-yearMonthDuration-to-dateTime`.
///
/// # Errors
///
/// err:XPTY0004 of an operand of another type; err:FODT0001 of a year
/// outside 0001 to 9999.
pub fn add_year_month_duration_to_date_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_months(a, d, AtomicType::DateTime, false)
}

/// [XQueryFO §10.8.5] `op:add-dayTimeDuration-to-dateTime`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn add_day_time_duration_to_date_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::DateTime, false)
}

/// [XQueryFO §10.8.6] `op:subtract-yearMonthDuration-from-dateTime`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn subtract_year_month_duration_from_date_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_months(a, d, AtomicType::DateTime, true)
}

/// [XQueryFO §10.8.7] `op:subtract-dayTimeDuration-from-dateTime`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn subtract_day_time_duration_from_date_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::DateTime, true)
}

/// [XQueryFO §10.8.8] `op:add-yearMonthDuration-to-date`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn add_year_month_duration_to_date(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_months(a, d, AtomicType::Date, false)
}

/// [XQueryFO §10.8.9] `op:add-dayTimeDuration-to-date`: of its starting
/// instant, the time of the result discarded.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn add_day_time_duration_to_date(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::Date, false)
}

/// [XQueryFO §10.8.10] `op:subtract-yearMonthDuration-from-date`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn subtract_year_month_duration_from_date(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_months(a, d, AtomicType::Date, true)
}

/// [XQueryFO §10.8.11] `op:subtract-dayTimeDuration-from-date`.
///
/// # Errors
///
/// As `add_year_month_duration_to_date_time`.
pub fn subtract_day_time_duration_from_date(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::Date, true)
}

/// [XQueryFO §10.8.12] `op:add-dayTimeDuration-to-time`: the duration
/// modulo a day added, the time wrapping round a day.
///
/// # Errors
///
/// err:XPTY0004 of an operand of another type.
pub fn add_day_time_duration_to_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::Time, false)
}

/// [XQueryFO §10.8.13] `op:subtract-dayTimeDuration-from-time`.
///
/// # Errors
///
/// As `add_day_time_duration_to_time`.
pub fn subtract_day_time_duration_from_time(a: &Atomic, d: &Atomic) -> Result<Atomic> {
    with_micros(a, d, AtomicType::Time, true)
}

#[cfg(test)]
mod tests;
