//! The value spaces of the atomic types [Schema2 §2.2, §4.2.1, §4.2.2]:
//! the value an atomic value's string denotes, equality, the order of
//! each ordered type, partial where [Schema2 §3.2.6.2, §3.2.7.3] make it
//! so, the canonical representation [Schema2 §2.3.1], and the units the
//! length and digits facets count.
//!
//! \[DESIGN\] RULES.md D106: a value is held as its string (`Atomic`), so
//! each is read again here when compared; values of types derived from
//! different primitive types are never equal and never ordered [Schema2
//! §4.2.1].

use std::cmp::Ordering;

use super::binary::{base64, hex};
use super::duration::units;
use super::exact::Exact;
use super::moment::{Moment, Shape};
use super::number::{boolean, double, real_canonical};
use super::types::Primitive;
use super::{Atomic, AtomicType};
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE, days_from_civil};

/// [Schema2 §3.2.7.3] The time zone a value without one may have at most,
/// either way, in minutes: 14 hours.
const ZONE_RANGE: i128 = 14 * 60;
/// [Schema2 §3.2.6.2] The dateTimes the order of durations adds them to:
/// 1696-09-01, 1697-02-01, 1903-03-01 and 1903-07-01, each at 00:00:00Z.
const DURATION_STARTS: [(i64, i64); 4] = [(1696, 9), (1697, 2), (1903, 3), (1903, 7)];
/// Months in a year.
const MONTHS: i128 = 12;
/// Years in a cycle of the Gregorian calendar.
const CYCLE_YEARS: i128 = 400;
/// Days in a cycle of the Gregorian calendar.
const CYCLE_DAYS: i128 = 146_097;

#[cfg(test)]
mod tests;

/// A value of a value space.
#[derive(Debug, Clone)]
enum Datum {
    /// A string or URI: its characters.
    Text(Box<str>),
    /// A QName or NOTATION: its namespace URI and its local name, its
    /// prefix no part of it [XQueryDM §3.3.3].
    Name(Option<Box<str>>, Box<str>),
    /// A truth value.
    Truth(bool),
    /// A decimal.
    Exact(Exact),
    /// A float or double.
    Real(f64),
    /// A duration: months and microseconds.
    Duration(i128, i128),
    /// A date, time, date and time or Gregorian value.
    Moment(Moment),
    /// Octets.
    Octets(Vec<u8>),
}

/// The value `a` denotes; none if its string is no lexical form of its
/// type, which no value made by a cast has.
fn datum(a: &Atomic) -> Option<Datum> {
    let text = &*a.lexical;
    let p = a.kind.primitive();
    if let Some(shape) = Shape::of(p) {
        return Moment::parse(text, shape).ok().flatten().map(Datum::Moment);
    }
    Some(match p {
        Primitive::Boolean => Datum::Truth(boolean(text)?),
        Primitive::Decimal | Primitive::Integer => Datum::Exact(Exact::parse(text)?),
        Primitive::Float | Primitive::Double => Datum::Real(double(text)?),
        Primitive::Duration | Primitive::YearMonthDuration | Primitive::DayTimeDuration => {
            let (months, micros) = units(a)?;
            Datum::Duration(months, micros)
        }
        Primitive::HexBinary => Datum::Octets(hex(text)?),
        Primitive::Base64Binary => Datum::Octets(base64(text)?),
        Primitive::QName | Primitive::Notation => {
            let local = text.split_once(':').map_or(text, |(_, local)| local);
            Datum::Name(a.namespace.clone(), local.into())
        }
        Primitive::Untyped
        | Primitive::String
        | Primitive::AnyUri
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth => Datum::Text(text.into()),
    })
}

/// The order of `a` and `b` [Schema2 §4.2.2]: equal, less or greater;
/// none if they are not equal and neither is less, as values of a type
/// with no order, values of different value spaces, and the pairs the
/// partial orders of durations and of dates and times leave indeterminate
/// (`<>`) are.
#[must_use]
pub fn compare(a: &Atomic, b: &Atomic) -> Option<Ordering> {
    if a.kind.primitive().schema() != b.kind.primitive().schema() {
        return None;
    }
    match (datum(a)?, datum(b)?) {
        (Datum::Exact(x), Datum::Exact(y)) => Some(x.compare(&y)),
        (Datum::Real(x), Datum::Real(y)) => Some(real_order(x, y)),
        (Datum::Duration(xm, xu), Datum::Duration(ym, yu)) => duration_order((xm, xu), (ym, yu)),
        (Datum::Moment(x), Datum::Moment(y)) => moment_order(&x, &y),
        (x, y) => same(&x, &y).then_some(Ordering::Equal),
    }
}

/// A key of an atomic value's value, for hashing [Schema2 §4.2.1]: two
/// values `equal` holds equal have one key, and values of different keys
/// are not equal, so a value is found among others by its key alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EqualityKey(Primitive, Keyed);

/// The part of an equality key a value space's value gives: the value
/// itself where equality is identity, a decimal's canonical
/// representation, which zero's minus is not part of, a float's bits,
/// not-a-number's one, and a date or
/// time's instant in UTC of one with a time zone, its local value of one
/// without, as those are never equal [Schema2 §3.2.7.3].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Keyed {
    /// A string or URI.
    Text(Box<str>),
    /// A QName or NOTATION: its namespace URI and local name.
    Name(Option<Box<str>>, Box<str>),
    /// A truth value.
    Truth(bool),
    /// A decimal's canonical representation [Schema2 §3.2.3.2].
    Exact(String),
    /// A float or double's bits.
    Real(u64),
    /// A duration's months and microseconds.
    Duration(i128, i128),
    /// A date or time of a time zone: its instant in UTC.
    Zoned(i128),
    /// A date or time of none: its local value.
    Local(i128),
    /// Octets.
    Octets(Vec<u8>),
}

/// The equality key of `a`; none if its string is no lexical form of its
/// type, so it is equal to no value.
#[must_use]
pub fn equality_key(a: &Atomic) -> Option<EqualityKey> {
    let keyed = match datum(a)? {
        Datum::Text(t) => Keyed::Text(t),
        Datum::Name(uri, local) => Keyed::Name(uri, local),
        Datum::Truth(b) => Keyed::Truth(b),
        Datum::Exact(x) => Keyed::Exact(x.canonical(false)),
        Datum::Real(x) => Keyed::Real(if x.is_nan() { f64::NAN.to_bits() } else { x.to_bits() }),
        Datum::Duration(months, micros) => Keyed::Duration(months, micros),
        Datum::Moment(m) => match m.local() {
            (local, Some(zone)) => Keyed::Zoned(local.saturating_sub(i128::from(zone).saturating_mul(i128::from(MICROS_PER_MINUTE)))),
            (local, None) => Keyed::Local(local),
        },
        Datum::Octets(o) => Keyed::Octets(o),
    };
    Some(EqualityKey(a.kind.primitive().schema(), keyed))
}

/// Whether `a` and `b` are equal [Schema2 §4.2.1].
#[must_use]
pub fn equal(a: &Atomic, b: &Atomic) -> bool {
    compare(a, b) == Some(Ordering::Equal)
}

/// Whether `x` and `y`, of a type with no order, are the same value.
fn same(x: &Datum, y: &Datum) -> bool {
    match (x, y) {
        (Datum::Text(s), Datum::Text(t)) => s == t,
        (Datum::Name(su, sl), Datum::Name(tu, tl)) => su == tu && sl == tl,
        (Datum::Truth(s), Datum::Truth(t)) => s == t,
        (Datum::Octets(s), Datum::Octets(t)) => s == t,
        (
            Datum::Text(_)
            | Datum::Name(..)
            | Datum::Truth(_)
            | Datum::Octets(_)
            | Datum::Exact(_)
            | Datum::Real(_)
            | Datum::Duration(..)
            | Datum::Moment(_),
            _,
        ) => false,
    }
}

/// [Schema2 §3.2.4, §3.2.5] The order of two floats or doubles: x < y iff
/// y - x is positive, positive zero greater than negative zero, and
/// not-a-number equal to itself and greater than every other value.
fn real_order(x: f64, y: f64) -> Ordering {
    match (x.is_nan(), y.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => x.total_cmp(&y),
    }
}

/// [Schema2 §3.2.7.3] The order of two dates, times or date and times of
/// one type: by their instants in UTC if both have a time zone or
/// neither has; else the one with a time zone less or greater only if it
/// is so of the other at every time zone from -14:00 to +14:00, and
/// indeterminate otherwise.
fn moment_order(p: &Moment, q: &Moment) -> Option<Ordering> {
    let utc = |m: &Moment| {
        let (local, zone) = m.local();
        zone.map(|z| local.saturating_sub(i128::from(z).saturating_mul(i128::from(MICROS_PER_MINUTE))))
    };
    match (utc(p), utc(q)) {
        (Some(x), Some(y)) => Some(x.cmp(&y)),
        (None, None) => Some(p.local().0.cmp(&q.local().0)),
        (Some(x), None) => zoned_order(x, q.local().0),
        (None, Some(y)) => zoned_order(y, p.local().0).map(Ordering::reverse),
    }
}

/// [Schema2 §3.2.7.3 C] The order of the instant `zoned` of a value with
/// a time zone and the local value `local` of one without: less if less
/// than it at +14:00, greater if greater than it at -14:00, else
/// indeterminate.
fn zoned_order(zoned: i128, local: i128) -> Option<Ordering> {
    let range = ZONE_RANGE.saturating_mul(i128::from(MICROS_PER_MINUTE));
    if zoned < local.saturating_sub(range) {
        Some(Ordering::Less)
    } else if zoned > local.saturating_add(range) {
        Some(Ordering::Greater)
    } else {
        None
    }
}

/// [Schema2 §3.2.6.2] The order of two durations of months and
/// microseconds: x < y iff s + x < s + y for each of the four dateTimes s
/// that deviate most, by the addition of [Schema2 Appendix E]; equal of
/// the same months and microseconds [XQueryFO §10.4.7]; else
/// indeterminate.
fn duration_order(x: (i128, i128), y: (i128, i128)) -> Option<Ordering> {
    if x == y {
        return Some(Ordering::Equal);
    }
    let mut found = None;
    for start in DURATION_STARTS {
        let order = end(start, x)?.cmp(&end(start, y)?);
        if order == Ordering::Equal || found.is_some_and(|f| f != order) {
            return None;
        }
        found = Some(order);
    }
    found
}

/// [Schema2 Appendix E] The instant, in microseconds from 0001-01-01, of
/// the first day of the month `start` plus the duration of `months` and
/// `micros`: months added first, the day kept within the month, then the
/// microseconds.
fn end((year, month): (i64, i64), (months, micros): (i128, i128)) -> Option<i128> {
    let temp = i128::from(month).checked_sub(1)?.checked_add(months)?;
    let end_month = temp.rem_euclid(MONTHS).checked_add(1)?;
    let end_year = i128::from(year).checked_add(temp.div_euclid(MONTHS))?;
    let day = MICROS_PER_DAY.into();
    let days = civil_days(end_year, end_month)?.checked_add(micros.div_euclid(day))?;
    days.checked_mul(day)?.checked_add(micros.rem_euclid(day))
}

/// The days from 0001-01-01 of the first of the month `month` of `year`,
/// of any size, by the 400 years after which the calendar repeats.
fn civil_days(year: i128, month: i128) -> Option<i128> {
    let cycles = year.checked_sub(1)?.div_euclid(CYCLE_YEARS);
    let within = i64::try_from(year.checked_sub(cycles.checked_mul(CYCLE_YEARS)?)?).ok()?;
    let month = i64::try_from(month).ok()?;
    cycles.checked_mul(CYCLE_DAYS)?.checked_add(i128::from(days_from_civil(within, month, 1)))
}

/// [Schema2 §2.3.1] The canonical representation of `a`'s value: of a
/// decimal its period and a digit either side [§3.2.3.2], of an integer
/// none [§3.3.13.2], -0 the zero of `xs:nonPositiveInteger` [§3.3.14.2];
/// of a float or double its mantissa and exponent [§3.2.4.2, §3.2.5.2];
/// of a date and time or a time with a time zone, the value in UTC and Z
/// [§3.2.7.2, §3.2.8.2]; else its string, as for each other type XQuery
/// writes that [XQueryFO §17.1.2].
#[must_use]
pub fn canonical(a: &Atomic) -> String {
    let text = || a.lexical.to_string();
    let Some(d) = datum(a) else { return text() };
    match d {
        Datum::Exact(x) => {
            let integer = a.kind.primitive() == Primitive::Integer;
            if a.kind == AtomicType::NonPositiveInteger && x.is_zero() { "-0".to_owned() } else { x.canonical(integer) }
        }
        Datum::Real(x) => real_canonical(x, a.kind.primitive() == Primitive::Float),
        Datum::Moment(m) => {
            let utc = m.zone.is_some() && matches!(m.shape, Shape::DateTime | Shape::Time);
            if utc { Moment { zone: Some(0), ..m.in_utc() }.text() } else { text() }
        }
        Datum::Text(_) | Datum::Name(..) | Datum::Truth(_) | Datum::Duration(..) | Datum::Octets(_) => text(),
    }
}

/// [Schema2 §4.3.1] The length of `a`'s value, in the units of the
/// length facets: characters of a string or URI, octets of binary data;
/// none of a type they count nothing of.
#[must_use]
pub fn length(a: &Atomic) -> Option<usize> {
    match datum(a)? {
        Datum::Octets(o) => Some(o.len()),
        Datum::Text(s) => {
            matches!(a.kind.primitive(), Primitive::String | Primitive::Untyped | Primitive::AnyUri).then(|| s.chars().count())
        }
        Datum::Name(..) | Datum::Truth(_) | Datum::Exact(_) | Datum::Real(_) | Datum::Duration(..) | Datum::Moment(_) => None,
    }
}

/// [Schema2 §4.3.11.3, §4.3.12.3] The total digits and fraction digits of
/// `a`'s value, of a decimal or integer; none of another.
#[must_use]
pub fn digits(a: &Atomic) -> Option<(usize, usize)> {
    match datum(a)? {
        Datum::Exact(x) => Some((x.total_digits(), x.fraction_digits())),
        Datum::Text(_) | Datum::Name(..) | Datum::Truth(_) | Datum::Real(_) | Datum::Duration(..) | Datum::Moment(_) | Datum::Octets(_) => {
            None
        }
    }
}
