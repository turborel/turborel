//! Dates, times and durations, their comparisons [XQueryFO §10.4] and
//! arithmetic [XQueryFO §10.6, §10.8]: a value of no time zone given the
//! implicit timezone to compare or subtract [XQueryFO §10.4]; durations
//! added to dates and times by the algorithm of [Schema2 Appendix E], a
//! day past a month's last pinned to it.
//!
//! \[DESIGN\] RULES.md D86: a date's year stays in 0001 to 9999, else
//! err:FODT0001; a duration in 128 bits of months or microseconds, else
//! err:FODT0002 (`xml/atomic.rs`).

use std::cmp::Ordering;

use super::super::error::{ErrorCode, error};
use super::super::plan::Arith;
use super::numeric::{self, Num};
use crate::error::Result;
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE, civil_from_days, days_from_civil, days_in};
use crate::xml::{Atomic, AtomicType, LAST_YEAR, Moment, Primitive, Shape, day_time_text, duration_units, year_month_text};

/// Months in a year.
const MONTHS: i64 = 12;
/// [XQueryFO §10.6.3] Half a month, the least a product or quotient of a
/// duration rounds up from.
const HALF: f64 = 0.5;

/// The moment `a` is, of a date or time type.
fn moment(a: &Atomic) -> Option<Moment> {
    let shape = Shape::of(a.kind.primitive())?;
    Moment::parse(&a.lexical, shape).ok().flatten()
}

/// The moment `a` is, of a date, a time or a date and time, the types
/// durations are added to [XQuery Appendix B.2].
fn dated(a: &Atomic) -> Option<Moment> {
    moment(a).filter(|m| !m.shape.gregorian())
}

/// How the dates or times `a` and `b`, of one type, compare by their
/// instants [XQueryFO §10.4]; none if either is of none.
pub(super) fn compare(a: &Atomic, b: &Atomic, timezone: i16) -> Option<Ordering> {
    moment(a).zip(moment(b)).map(|(x, y)| instant(&x, timezone).cmp(&instant(&y, timezone)))
}

/// The instant of `m` in microseconds from 0001-01-01T00:00:00Z, of the
/// implicit timezone `timezone` if it has none [XQueryFO §10.4]; a time's
/// on one reference day.
fn instant(m: &Moment, timezone: i16) -> i128 {
    let local = i128::from(m.days).saturating_mul(i128::from(MICROS_PER_DAY)).saturating_add(i128::from(m.micros));
    local.saturating_sub(i128::from(m.zone.unwrap_or(timezone)).saturating_mul(i128::from(MICROS_PER_MINUTE)))
}

/// The error of a date or time past the years turborel holds [XQueryFO
/// §10.1.1].
fn date_overflow() -> crate::error::SqlError {
    error(ErrorCode::FODT0001, &format!("the date's year is past 0001 to {LAST_YEAR} [XQueryFO §10.1.1]"))
}

/// The error of a duration past 128 bits [XQueryFO §10.1.1].
fn duration_overflow() -> crate::error::SqlError {
    error(ErrorCode::FODT0002, "the duration is past 128 bits of months or microseconds [XQueryFO §10.1.1]")
}

/// `m` as a value, its year within 0001 to 9999.
fn moment_value(m: Moment, kind: AtomicType) -> Result<Atomic> {
    let year = m.year();
    if !(1..=LAST_YEAR).contains(&year) {
        return Err(date_overflow());
    }
    Ok(Atomic::new(kind, m.text()))
}

/// `m` plus `months` [Schema2 Appendix E]: its year and month moved, its
/// day pinned to the last of the month.
fn plus_months(m: Moment, months: i128) -> Result<Moment> {
    let (year, month, day) = civil_from_days(m.days);
    let total = i128::from(year).checked_mul(i128::from(MONTHS)).and_then(|t| t.checked_add(i128::from(month.saturating_sub(1))));
    let total = total.and_then(|t| t.checked_add(months)).and_then(|t| i64::try_from(t).ok()).ok_or_else(date_overflow)?;
    let (new_year, new_month) = (total.div_euclid(MONTHS), total.rem_euclid(MONTHS).saturating_add(1));
    if !(1..=LAST_YEAR).contains(&new_year) {
        return Err(date_overflow());
    }
    let day = day.min(days_in(new_year, new_month));
    Ok(Moment { days: days_from_civil(new_year, new_month, day), ..m })
}

/// `m` plus `micros` [Schema2 Appendix E]: a time's wrapped within its
/// day, a date's added to its starting instant and its time dropped.
fn plus_micros(m: Moment, micros: i128) -> Result<Moment> {
    let day = i128::from(MICROS_PER_DAY);
    let local =
        i128::from(m.days).saturating_mul(day).saturating_add(i128::from(m.micros)).checked_add(micros).ok_or_else(date_overflow)?;
    let (days, time) = (local.div_euclid(day), local.rem_euclid(day));
    let days = i64::try_from(days).map_err(|_| date_overflow())?;
    let time = i64::try_from(time).map_err(|_| date_overflow())?;
    Ok(match m.shape {
        Shape::Time => Moment { micros: time, ..m },
        Shape::Date => Moment { days, micros: 0, ..m },
        // No Gregorian value is added to (`dated`); were one, it would
        // move as its starting instant.
        Shape::DateTime | Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth => {
            Moment { days, micros: time, ..m }
        }
    })
}

/// A duration of the two kinds the operators compute with: its units, and
/// whether they are months.
fn duration(a: &Atomic) -> Result<Option<(i128, bool)>> {
    let p = a.kind.primitive();
    if p == Primitive::YearMonthDuration || p == Primitive::DayTimeDuration {
        let months = p == Primitive::YearMonthDuration;
        return Ok(Some((duration_units(&a.lexical, months)?, months)));
    }
    Ok(None)
}

/// The duration of `units`, months if `months`.
fn duration_value(units: i128, months: bool) -> Atomic {
    if months {
        Atomic::new(AtomicType::YearMonthDuration, year_month_text(units))
    } else {
        Atomic::new(AtomicType::DayTimeDuration, day_time_text(units))
    }
}

/// `a op b` of dates, times and durations, and durations and numbers, as
/// the operator mapping has it [XQuery Appendix B.2] [XQueryFO §10.6,
/// §10.8]; none if the mapping has no such combination.
pub(super) fn arithmetic(op: Arith, a: &Atomic, b: &Atomic, timezone: i16) -> Result<Option<Atomic>> {
    let (da, db) = (duration(a)?, duration(b)?);
    match (dated(a), dated(b), da, db) {
        (Some(x), Some(y), None, None) => Ok(difference(op, &x, &y, a.kind.primitive() == b.kind.primitive(), timezone)),
        (Some(x), None, None, Some(d)) => shifted(op, x, d, a.kind),
        (None, Some(y), Some(d), None) => {
            if op == Arith::Add {
                shifted(op, y, d, b.kind)
            } else {
                Ok(None)
            }
        }
        (None, None, Some(x), Some(y)) => durations(op, x, y),
        (None, None, Some(d), None) => scaled(op, d, numeric::of(b)?, false),
        (None, None, None, Some(d)) => scaled(op, d, numeric::of(a)?, true),
        (Some(_) | None, Some(_) | None, Some(_) | None, Some(_) | None) => Ok(None),
    }
}

/// `x - y` of two dates, times or dates and times of one type, a
/// `xdt:dayTimeDuration` [XQueryFO §10.8.1 to §10.8.3].
fn difference(op: Arith, x: &Moment, y: &Moment, alike: bool, timezone: i16) -> Option<Atomic> {
    (op == Arith::Subtract && alike).then(|| duration_value(instant(x, timezone).saturating_sub(instant(y, timezone)), false))
}

/// `m` of type `kind` plus or minus the duration `(units, months)`
/// [XQueryFO §10.8.4 to §10.8.13]: months to a date or date and time
/// alone.
fn shifted(op: Arith, m: Moment, (units, months): (i128, bool), kind: AtomicType) -> Result<Option<Atomic>> {
    let units = match op {
        Arith::Add => units,
        Arith::Subtract => units.checked_neg().ok_or_else(duration_overflow)?,
        Arith::Multiply | Arith::Divide | Arith::IntegerDivide | Arith::Modulo => return Ok(None),
    };
    let moved = match (months, m.shape) {
        (true, Shape::Time | Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth) => return Ok(None),
        (true, Shape::Date | Shape::DateTime) => plus_months(m, units)?,
        (false, _) => plus_micros(m, units)?,
    };
    moment_value(moved, kind).map(Some)
}

/// `x op y` of two durations of one type [XQueryFO §10.6]: a sum or
/// difference of the type, a quotient a decimal.
fn durations(op: Arith, (x, mx): (i128, bool), (y, my): (i128, bool)) -> Result<Option<Atomic>> {
    if mx != my {
        return Ok(None);
    }
    Ok(match op {
        Arith::Add => Some(duration_value(x.checked_add(y).ok_or_else(duration_overflow)?, mx)),
        Arith::Subtract => Some(duration_value(x.checked_sub(y).ok_or_else(duration_overflow)?, mx)),
        // [XQueryFO §10.6.5, §10.6.10] as op:numeric-divide of integers or
        // decimals.
        Arith::Divide => Some(numeric::atomic(numeric::arithmetic(Arith::Divide, Num::Integer(x), Num::Integer(y))?)),
        Arith::Multiply | Arith::IntegerDivide | Arith::Modulo => None,
    })
}

/// The duration `(units, months)` multiplied or divided by the number
/// `n`, promoted to `xs:double` [XQueryFO §10.6.3, §10.6.4, §10.6.8,
/// §10.6.9]: months rounded to the nearest, a half up; NaN err:FOCA0005;
/// an infinite or too large result err:FODT0002. A number first divides
/// none.
fn scaled(op: Arith, (units, months): (i128, bool), n: Option<Num>, number_first: bool) -> Result<Option<Atomic>> {
    let Some(n) = n else { return Ok(None) };
    let factor = numeric::to_f64(n);
    let product = match (op, number_first) {
        (Arith::Multiply, _) => to_f64(units) * factor,
        (Arith::Divide, false) => to_f64(units) / factor,
        (Arith::Add | Arith::Subtract | Arith::Divide | Arith::IntegerDivide | Arith::Modulo, _) => return Ok(None),
    };
    if factor.is_nan() {
        return Err(error(ErrorCode::FOCA0005, "a duration is multiplied or divided by NaN [XQueryFO §10.6.3]"));
    }
    let rounded = if months { (product + HALF).floor() } else { product.trunc() };
    let product_units = numeric::whole(rounded).map_err(|_| duration_overflow())?;
    Ok(Some(duration_value(product_units, months)))
}

/// `units` as a double.
fn to_f64(units: i128) -> f64 {
    numeric::to_f64(Num::Integer(units))
}
