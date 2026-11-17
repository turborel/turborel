//! Durations [Schema2 §3.2.6] and the two totally ordered ones [XQueryFO
//! §10.3]: `xdt:yearMonthDuration`, a number of months, and
//! `xdt:dayTimeDuration`, a number of seconds, here microseconds; an
//! `xs:duration` both [XQueryFO §10.4.7]. Their lexical forms, their
//! canonical forms, and casts among them [XQueryFO §17.1.4].

use std::fmt::Write;

use super::types::Primitive;
use super::{Atomic, xquery_error};
use crate::error::Result;
use crate::value::datetime::MICROS_PER_SECOND;

/// The designators of a duration's components, in order [Schema2
/// §3.2.6.1]: years, months and days, then, after T, hours, minutes and
/// seconds.
const DESIGNATORS: [char; 6] = ['Y', 'M', 'D', 'H', 'M', 'S'];
/// The place in `DESIGNATORS` of the first component after T.
const TIME_PLACE: usize = 3;
/// The place of the seconds.
const SECOND_PLACE: usize = 5;
/// Months in a year.
const MONTHS: i128 = 12;
/// What one of each day-time component is in microseconds: a day, an
/// hour, a minute and a second.
const UNITS: [i128; 4] = [86_400_000_000, 3_600_000_000, 60_000_000, 1_000_000];
/// Digits of a second's fraction kept: microseconds.
const FRACTION_DIGITS: usize = 6;

/// The components of the duration `body`, after its P, each its place in
/// `DESIGNATORS` and its number's text, in order, none twice, T before
/// the time's and only if one follows; none if it is no lexical form of a
/// duration [Schema2 §3.2.6.1].
fn components(body: &str) -> Option<Vec<(usize, &str)>> {
    let (date, time) = body.split_once('T').map_or((body, None), |(d, t)| (d, Some(t)));
    if time == Some("") {
        return None;
    }
    let mut out = Vec::new();
    read(date, (DESIGNATORS.get(..TIME_PLACE)?, 0), &mut out)?;
    read(time.unwrap_or_default(), (DESIGNATORS.get(TIME_PLACE..)?, TIME_PLACE), &mut out)?;
    (!out.is_empty()).then_some(out)
}

/// The components of `text`, each a number and a designator of
/// `designators` after the last's, pushed to `out` with their places, the
/// first of `designators` at `offset`; none if it is not so.
fn read<'a>(text: &'a str, (designators, offset): (&[char], usize), out: &mut Vec<(usize, &'a str)>) -> Option<()> {
    let (mut rest, mut next) = (text, 0_usize);
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.')?;
        let (number, after) = rest.split_at(end);
        let designator = after.chars().next()?;
        let at = next.checked_add(designators.iter().skip(next).position(|&d| d == designator)?)?;
        if number.is_empty() {
            return None;
        }
        out.push((offset.checked_add(at)?, number));
        next = at.checked_add(1)?;
        rest = after.get(designator.len_utf8()..)?;
    }
    Some(())
}

/// The sign of `text` and its body after P.
fn signed(text: &str) -> Option<(bool, &str)> {
    let (negative, rest) = text.strip_prefix('-').map_or((false, text), |r| (true, r));
    rest.strip_prefix('P').map(|body| (negative, body))
}

/// The error of a duration too large to hold.
fn too_large(text: &str) -> crate::error::SqlError {
    xquery_error(&format!("'{text}' is a duration too large to hold"), "[XQueryFO §10.1.1, §17.1.1] err:FODT0002")
}

/// The months of the `xdt:yearMonthDuration` `text` is a lexical form of:
/// years and months alone [XQueryFO §10.3.1.1, §10.3.1.2]; none if it is
/// none.
///
/// # Errors
///
/// SQLSTATE 10000, err:FODT0002, if they are too many to hold.
pub fn year_month(text: &str) -> Result<Option<i128>> {
    let Some((negative, body)) = signed(text) else { return Ok(None) };
    let Some(parts) = components(body).filter(|p| p.iter().all(|&(place, n)| place < 2 && !n.contains('.'))) else { return Ok(None) };
    let mut months: i128 = 0;
    for (place, n) in parts {
        let unit = if place == 0 { MONTHS } else { 1 };
        let value = n.parse::<i128>().ok().and_then(|v| v.checked_mul(unit));
        months = value.and_then(|v| months.checked_add(v)).ok_or_else(|| too_large(text))?;
    }
    Ok(Some(if negative { months.saturating_neg() } else { months }))
}

/// The microseconds of the `xdt:dayTimeDuration` `text` is a lexical form
/// of: days, hours, minutes and seconds alone [XQueryFO §10.3.2.1,
/// §10.3.2.2]; none if it is none. Digits of the seconds past the
/// microsecond are dropped.
///
/// # Errors
///
/// SQLSTATE 10000, err:FODT0002, if they are too many to hold.
pub fn day_time(text: &str) -> Result<Option<i128>> {
    let Some((negative, body)) = signed(text) else { return Ok(None) };
    let Some(parts) = components(body).filter(|p| p.iter().all(|&(place, _)| place >= 2)) else { return Ok(None) };
    let mut micros: i128 = 0;
    for (place, n) in parts {
        micros = match component_micros(place, n) {
            Component::Invalid => return Ok(None),
            Component::TooLarge => None,
            Component::Micros(v) => micros.checked_add(v),
        }
        .ok_or_else(|| too_large(text))?;
    }
    Ok(Some(if negative { micros.saturating_neg() } else { micros }))
}

/// The months and the microseconds of the `xs:duration` `text` is a
/// lexical form of [Schema2 §3.2.6.1]: years and months as months, days
/// to seconds as microseconds, both of its sign [XQueryFO §10.4.7]; none
/// if it is none.
///
/// # Errors
///
/// SQLSTATE 10000, err:FODT0002, if they are too many to hold.
pub fn general(text: &str) -> Result<Option<(i128, i128)>> {
    let Some((negative, body)) = signed(text) else { return Ok(None) };
    let Some(parts) = components(body) else { return Ok(None) };
    let (mut months, mut micros) = (0_i128, 0_i128);
    for (place, n) in parts {
        if place < 2 {
            let unit = if place == 0 { MONTHS } else { 1 };
            if n.contains('.') {
                return Ok(None);
            }
            let value = n.parse::<i128>().ok().and_then(|v| v.checked_mul(unit));
            months = value.and_then(|v| months.checked_add(v)).ok_or_else(|| too_large(text))?;
            continue;
        }
        micros = match component_micros(place, n) {
            Component::Invalid => return Ok(None),
            Component::TooLarge => None,
            Component::Micros(v) => micros.checked_add(v),
        }
        .ok_or_else(|| too_large(text))?;
    }
    Ok(Some(if negative { (months.saturating_neg(), micros.saturating_neg()) } else { (months, micros) }))
}

/// The string of the `xs:duration` of `months` and `micros`, both of one
/// sign [XQueryFO §17.1.2]: the day-time duration's if no months; the
/// year-month duration's if no microseconds; else both, the second
/// without its sign and P.
pub fn general_text(months: i128, micros: i128) -> String {
    let (year_month, day_time) = (year_month_text(months), day_time_text(micros));
    if months == 0 {
        return day_time;
    }
    if micros == 0 {
        return year_month;
    }
    let rest = day_time.trim_start_matches('-').trim_start_matches('P');
    format!("{year_month}{rest}")
}

/// What a component's number is.
enum Component {
    /// No number of its component.
    Invalid,
    /// Too many microseconds to hold.
    TooLarge,
    /// Its microseconds.
    Micros(i128),
}

/// The microseconds of the component at `place` whose number is `n`.
fn component_micros(place: usize, n: &str) -> Component {
    let (whole, fraction) = n.split_once('.').unwrap_or((n, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let number = !whole.is_empty() && digits(whole) && digits(fraction);
    let point = !n.contains('.') || (!fraction.is_empty() && place == SECOND_PLACE);
    let unit = place.checked_sub(2).and_then(|u| UNITS.get(u)).copied();
    let kept = format!("{fraction:0<FRACTION_DIGITS$}");
    let micros = kept.get(..FRACTION_DIGITS).and_then(|f| f.parse::<i128>().ok());
    let (Some(unit), Some(micros), true, true) = (unit, micros, number, point) else { return Component::Invalid };
    match whole.parse::<i128>().ok().and_then(|w| w.checked_mul(unit)).and_then(|w| w.checked_add(micros)) {
        Some(total) => Component::Micros(total),
        None => Component::TooLarge,
    }
}

/// The canonical form of the `xdt:yearMonthDuration` of `months` [XQueryFO
/// §10.3.1.3]: years, then months below 12, a zero one left out; P0M of
/// none.
pub fn year_month_text(months: i128) -> String {
    let size = months.unsigned_abs();
    let (years, rest) = (size.checked_div(12).unwrap_or(0), size.checked_rem(12).unwrap_or(0));
    if size == 0 {
        return "P0M".to_owned();
    }
    let sign = if months < 0 { "-" } else { "" };
    let years = if years == 0 { String::new() } else { format!("{years}Y") };
    let rest = if rest == 0 { String::new() } else { format!("{rest}M") };
    format!("{sign}P{years}{rest}")
}

/// The canonical form of the `xdt:dayTimeDuration` of `micros` [XQueryFO
/// §10.3.2.3]: days, then hours below 24, minutes below 60 and seconds
/// below 60 after T, a zero one left out, the seconds a decimal's string;
/// PT0S of none.
pub fn day_time_text(micros: i128) -> String {
    let mut size = micros.unsigned_abs();
    if size == 0 {
        return "PT0S".to_owned();
    }
    let mut counts = [0_u128; 3];
    for (count, unit) in counts.iter_mut().zip(UNITS) {
        let unit = unit.unsigned_abs();
        *count = size.checked_div(unit).unwrap_or(0);
        size = size.checked_rem(unit).unwrap_or(0);
    }
    let [days, hours, minutes] = counts;
    let seconds = size;
    let second = MICROS_PER_SECOND.unsigned_abs().into();
    let fraction = format!("{:06}", seconds.checked_rem(second).unwrap_or(0));
    let fraction = fraction.trim_end_matches('0');
    let whole = seconds.checked_div(second).unwrap_or(0);
    let mut out = String::from(if micros < 0 { "-P" } else { "P" });
    if days > 0 {
        let _ = write!(out, "{days}D");
    }
    if hours > 0 || minutes > 0 || seconds > 0 {
        out.push('T');
    }
    for (count, designator) in [(hours, 'H'), (minutes, 'M')] {
        if count > 0 {
            let _ = write!(out, "{count}{designator}");
        }
    }
    if seconds > 0 {
        let dot = if fraction.is_empty() { "" } else { "." };
        let _ = write!(out, "{whole}{dot}{fraction}S");
    }
    out
}

/// The months and microseconds of `value`, a duration of any of the
/// three types; none if it is none.
#[must_use]
pub fn units(value: &Atomic) -> Option<(i128, i128)> {
    match value.kind.primitive() {
        Primitive::YearMonthDuration => year_month(&value.lexical).ok().flatten().map(|m| (m, 0)),
        Primitive::DayTimeDuration => day_time(&value.lexical).ok().flatten().map(|u| (0, u)),
        Primitive::Duration => general(&value.lexical).ok().flatten(),
        Primitive::Untyped
        | Primitive::String
        | Primitive::Boolean
        | Primitive::Decimal
        | Primitive::Integer
        | Primitive::Float
        | Primitive::Double
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
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => None,
    }
}

/// `value`, a duration, cast as `to`, another duration type [XQueryFO
/// §17.1.4]: as a year-month duration its months alone, as a day-time
/// duration its microseconds alone, as an `xs:duration` both; none of
/// another cast.
#[must_use]
pub fn convert(value: &Atomic, to: Primitive) -> Option<String> {
    let (months, micros) = units(value)?;
    match to {
        Primitive::YearMonthDuration => Some(year_month_text(months)),
        Primitive::DayTimeDuration => Some(day_time_text(micros)),
        Primitive::Duration => Some(general_text(months, micros)),
        Primitive::Untyped
        | Primitive::String
        | Primitive::Boolean
        | Primitive::Decimal
        | Primitive::Integer
        | Primitive::Float
        | Primitive::Double
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
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => None,
    }
}
