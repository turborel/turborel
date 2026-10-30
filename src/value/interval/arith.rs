//! [ISO/IEC 9075:1992 §6.14, §6.15] Arithmetic of intervals: of two of one
//! class, of one and a number, of a datetime and one, and the difference
//! of two datetimes in one. Split from interval.rs (RULES.md R-SIZE-2).

use super::{Interval, MAX_LEADING_PRECISION, Qualifier, unit};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::cast::approximate_to_exact;
use crate::value::datetime::{Datetime, Field, MICROS_PER_DAY, Moment, civil_from_days, days_from_civil, days_in};
use crate::value::{ArithOp, MAX_EXACT_PRECISION, Value, pow10};

/// The digits of fraction an approximate factor is taken to.
const FACTOR_SCALE: u32 = 9;

/// SQLSTATE 22015: `what` leaves `q`'s leading field.
fn overflow<T>(what: &str, q: Qualifier) -> Result<T> {
    fail_with(Code::Data, SqlState::INTERVAL_FIELD_OVERFLOW, format!("[ISO/IEC 9075:1992 §6.15 GR4, GR5] {what} does not fit INTERVAL {q}"))
}

/// `units` as an interval of `q`, truncated to its least field.
///
/// # Errors
///
/// SQLSTATE 22015 if its leading field cannot hold it.
fn within(units: Option<i128>, q: Qualifier, what: &str) -> Result<Value> {
    match units.and_then(|u| Interval { units: u, qualifier: q }.to(q)) {
        Some(i) => Ok(Value::Interval(i)),
        None => overflow(what, q),
    }
}

/// [ISO/IEC 9075:1992 §6.15 GR5] `a` plus, or minus, `b`, of one class:
/// of the fields of both, its leading field one digit more than either's,
/// up to `MAX_LEADING_PRECISION`, so no digit is lost.
///
/// # Errors
///
/// SQLSTATE 22015 if the result leaves its leading field.
pub fn add(a: &Interval, b: &Interval, subtract: bool) -> Result<Value> {
    let leading = u32::from(a.qualifier.leading.max(b.qualifier.leading)).saturating_add(1).min(MAX_LEADING_PRECISION);
    let q = a.qualifier.joined(&b.qualifier, leading);
    let units = if subtract { a.units.checked_sub(b.units) } else { a.units.checked_add(b.units) };
    within(units, q, "the sum")
}

/// [ISO/IEC 9075:1992 §6.15 GR4] `interval` times, or divided by, the
/// number `factor`: of its fields, in units of its least, truncated, its
/// leading field of `MAX_LEADING_PRECISION` digits.
///
/// # Errors
///
/// SQLSTATE 22012 for a divisor 0; 22015 if the result leaves its
/// leading field; 22003 if an approximate factor does not fit
/// `MAX_EXACT_PRECISION` digits at scale `FACTOR_SCALE`; 22000 if `factor`
/// is no number.
pub fn scale(interval: &Interval, op: ArithOp, factor: &Value) -> Result<Value> {
    let result = Qualifier { leading: u8::try_from(MAX_LEADING_PRECISION).unwrap_or(9), ..interval.qualifier };
    let exact = match factor {
        Value::Approx(x) => approximate_to_exact(*x, (MAX_EXACT_PRECISION, FACTOR_SCALE), "ISO/IEC 9075:1992 §6.15 GR4")?,
        Value::Null | Value::Char(_) | Value::Exact(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => {
            factor.clone()
        }
    };
    let Value::Exact(number) = exact else {
        return fail_with(Code::Data, SqlState::DATA_EXCEPTION, format!("[ISO/IEC 9075:1992 §6.15] {factor} is no number"));
    };
    let shift = pow10(number.scale).unwrap_or(i128::MAX);
    let units = match op {
        ArithOp::Div => {
            if number.units == 0 {
                return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[ISO/IEC 9075:1992 §6.15 GR4] an interval is divided by 0");
            }
            interval.units.checked_mul(shift).and_then(|n| n.checked_div(number.units))
        }
        ArithOp::Add | ArithOp::Sub | ArithOp::Mul => interval.units.checked_mul(number.units).and_then(|n| n.checked_div(shift)),
    };
    within(units, result, "the product")
}

/// SQLSTATE 22008: `d` plus, or minus, `i` leaves the datetimes.
fn datetime_overflow<T>(d: &Datetime, i: &Interval, subtract: bool) -> Result<T> {
    let sign = if subtract { '-' } else { '+' };
    let interval = format!("INTERVAL '{i}' {}", i.qualifier);
    fail_with(
        Code::Data,
        SqlState::DATETIME_FIELD_OVERFLOW,
        format!("[ISO/IEC 9075:1992 §6.14 GR3c] {} '{d}' {sign} {interval} is no valid datetime", d.key_word()),
    )
}

/// [ISO/IEC 9075:1992 §6.14 GR3] `d` plus, or minus, `i`: a year-month
/// interval's months added to its year and month, its day kept; a day-time
/// interval's microseconds to its instant, a time's modulo 24 hours; of
/// `d`'s precision \[SR2\], a fraction past it dropped.
///
/// # Errors
///
/// SQLSTATE 22008 if a field of the result leaves its range, or the date
/// is no date \[GR3c\].
pub fn shift(d: &Datetime, i: &Interval, subtract: bool) -> Result<Value> {
    let units = if subtract { i.units.checked_neg() } else { Some(i.units) };
    let Some(units) = units.and_then(|u| i64::try_from(u).ok()) else { return datetime_overflow(d, i, subtract) };
    let moved = if i.qualifier.year_month() {
        let (was_year, was_month, day) = civil_from_days(d.days());
        let months = was_year.saturating_mul(12).saturating_add(was_month.saturating_sub(1)).saturating_add(units);
        let (year, month) = (months.checked_div_euclid(12).unwrap_or(0), months.checked_rem_euclid(12).unwrap_or(0).saturating_add(1));
        if !(1..=9999).contains(&year) || day > days_in(year, month) {
            return datetime_overflow(d, i, subtract);
        }
        let local = days_from_civil(year, month, day).saturating_mul(MICROS_PER_DAY).saturating_add(d.time_of_day());
        Datetime::of_local(d.moment, local, d.zone, d.zoned, u32::from(d.precision))
    } else if d.moment == Moment::Time {
        Datetime::of_local(Moment::Time, d.local().saturating_add(units), d.zone, d.zoned, u32::from(d.precision))
    } else {
        Datetime { utc: d.utc.saturating_add(units), ..*d }
    };
    // [§6.14 SR2] of the datetime's precision.
    let moved = moved.truncated(u32::from(d.precision));
    if !moved.in_range() {
        return datetime_overflow(d, i, subtract);
    }
    Ok(Value::Datetime(moved))
}

/// [ISO/IEC 9075:1992 §6.15 GR6] `a` minus `b`, of the same fields, as an
/// interval of `q`: of a year-month interval, the months between their
/// years and months as shown; of a day-time one, the microseconds between
/// their instants; truncated to `q`'s least field.
///
/// # Errors
///
/// SQLSTATE 22015 if `q`'s leading field cannot hold it.
pub fn difference(a: &Datetime, b: &Datetime, q: Qualifier) -> Result<Value> {
    let units = if q.year_month() {
        let months = |d: &Datetime| {
            let (year, month, _) = civil_from_days(d.days());
            i128::from(year).saturating_mul(12).saturating_add(i128::from(month))
        };
        months(a).checked_sub(months(b))
    } else {
        i128::from(a.utc).checked_sub(i128::from(b.utc))
    };
    within(units, q, "the difference")
}

/// The interval `units` microseconds, of whole minutes, a time zone
/// displacement is: its minutes; none if not whole.
pub fn minutes(i: &Interval) -> Option<i16> {
    let per = unit(Field::Minute);
    (i.units.checked_rem(per) == Some(0)).then(|| i16::try_from(i.units.checked_div(per).unwrap_or(0)).ok()).flatten()
}
