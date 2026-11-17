//! Dates and times [Schema2 §3.2.7 to §3.2.14]: `xs:dateTime`, `xs:time`,
//! `xs:date` and the Gregorian types (`xml/atomic/gregorian.rs`), their
//! lexical forms, the strings casting them to `xs:string` gives [XQueryFO
//! §17.1.2], casts among them [XQueryFO §17.1.5], and their adjustment to
//! UTC [XQueryFO §10.7].

use super::types::Primitive;
use super::{Atomic, xquery_error};
use crate::error::Result;
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE, MICROS_PER_SECOND, civil_from_days, days_from_civil, days_in};

/// \[DESIGN\] The greatest year a date has here, as SQL's [ISO/IEC
/// 9075:1992 §6.1 Table 10]: an implementation-defined limit [XQueryFO
/// §10.1.1].
pub const LAST_YEAR: i64 = 9999;
/// [Schema2 §3.2.7.1] The digits a year has at least.
const YEAR_DIGITS: usize = 4;
/// [XQueryDM §3.3.2] The greatest size of a time zone, 14 hours, in
/// minutes.
const MAX_ZONE: i64 = 14 * 60;
/// [Schema2 §3.2.7.1] The hour that ends a day, 24:00:00 the next day's
/// 00:00:00.
const END_HOUR: i64 = 24;
/// Digits of a second's fraction kept: microseconds.
const FRACTION_DIGITS: usize = 6;
/// Minutes in an hour, seconds in a minute.
const SIXTY: i64 = 60;
/// Months in a year.
const MONTHS: i64 = 12;

/// Which of the three types a value is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `xs:date`.
    Date,
    /// `xs:time`.
    Time,
    /// `xs:dateTime`.
    DateTime,
    /// `xs:gYearMonth`.
    GYearMonth,
    /// `xs:gYear`.
    GYear,
    /// `xs:gMonthDay`.
    GMonthDay,
    /// `xs:gDay`.
    GDay,
    /// `xs:gMonth`.
    GMonth,
}

/// A date, a time, a date and time or a Gregorian value [XQueryFO §10.2]:
/// its local value and its time zone, if it has one. A Gregorian value is
/// its starting instant (`xml/atomic/gregorian.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// Its type.
    pub shape: Shape,
    /// Its date, in days from 0001-01-01; 0 of a time.
    pub days: i64,
    /// Its time of day, in microseconds; 0 of a date.
    pub micros: i64,
    /// Its time zone, in minutes east of UTC, if it has one.
    pub zone: Option<i16>,
}

/// `count` digits of `text` from `at`, as a number, if they are digits.
fn number(text: &str, at: usize, count: usize) -> Option<i64> {
    let part = text.get(at..at.checked_add(count)?)?;
    part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse().ok()).flatten()
}

/// The error of a year beyond the limits.
fn beyond(text: &str) -> crate::error::SqlError {
    xquery_error(&format!("'{text}' is of a year past 0001 to {LAST_YEAR}"), "[XQueryFO §10.1.1, §17.1.1] err:FODT0001")
}

/// `text` split into what is before its time zone and the time zone: Z,
/// or a sign and hh:mm of at most 14 hours [Schema2 §3.2.7.1] [XQueryDM
/// §3.3.2]; none if it ends in no valid one.
fn zone_of(text: &str) -> Option<(&str, Option<i16>)> {
    if let Some(body) = text.strip_suffix('Z') {
        return Some((body, Some(0)));
    }
    let Some(at) = text.len().checked_sub(6) else { return Some((text, None)) };
    if text.as_bytes().get(at.saturating_add(3)) != Some(&b':') {
        return Some((text, None));
    }
    let sign = text.as_bytes().get(at).copied();
    let negative = match sign {
        Some(b'+') => false,
        Some(b'-') => true,
        Some(_) | None => return Some((text, None)),
    };
    let tz = text.get(at..)?;
    let (hours, minutes) = (number(tz, 1, 2)?, number(tz, 4, 2)?);
    let size = hours.checked_mul(SIXTY)?.checked_add(minutes)?;
    let valid = tz.as_bytes().get(3) == Some(&b':') && minutes < SIXTY && size <= MAX_ZONE;
    let zone = i16::try_from(if negative { size.checked_neg()? } else { size }).ok()?;
    valid.then(|| (text.get(..at).unwrap_or_default(), Some(zone)))
}

/// The days from 0001-01-01 of the date `text`, yyyy-mm-dd, its year of
/// four digits or more, none leading if more, not 0000 [Schema2
/// §3.2.9.1]; none if it is no lexical form of one.
///
/// # Errors
///
/// SQLSTATE 10000, err:FODT0001, of a year negative or past 9999.
fn date_part(text: &str) -> Result<Option<i64>> {
    let (negative, body) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
    let Some((year, rest)) = body.split_once('-') else { return Ok(None) };
    let lexical = year.len() >= YEAR_DIGITS && (year.len() == YEAR_DIGITS || !year.starts_with('0')) && rest.len() == 5;
    let (Some(y), Some(month), Some(day)) = (number(year, 0, year.len()), number(rest, 0, 2), number(rest, 3, 2)) else { return Ok(None) };
    let valid = lexical && y > 0 && rest.as_bytes().get(2) == Some(&b'-') && (1..=MONTHS).contains(&month);
    if !valid || !(1..=days_in(y, month)).contains(&day) {
        return Ok(None);
    }
    if negative || y > LAST_YEAR {
        return Err(beyond(text));
    }
    Ok(Some(days_from_civil(y, month, day)))
}

/// The microseconds of the time `text`, hh:mm:ss with a fraction perhaps,
/// 24:00:00 the end of the day [Schema2 §3.2.8.1]; none if it is no
/// lexical form of one. Digits of the fraction past the microsecond are
/// dropped.
fn time_part(text: &str) -> Option<i64> {
    let (hour, minute, second) = (number(text, 0, 2)?, number(text, 3, 2)?, number(text, 6, 2)?);
    let colons = text.as_bytes().get(2) == Some(&b':') && text.as_bytes().get(5) == Some(&b':');
    let fraction = match text.get(8..)? {
        "" => "",
        rest => rest.strip_prefix('.').filter(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))?,
    };
    let micros: i64 = format!("{fraction:0<FRACTION_DIGITS$}").get(..FRACTION_DIGITS)?.parse().ok()?;
    let end = hour == END_HOUR && minute == 0 && second == 0 && micros == 0;
    if !colons || !(hour < END_HOUR || end) || minute >= SIXTY || second >= SIXTY {
        return None;
    }
    let seconds = hour.checked_mul(SIXTY)?.checked_add(minute)?.checked_mul(SIXTY)?.checked_add(second)?;
    seconds.checked_mul(MICROS_PER_SECOND)?.checked_add(micros)
}

impl Moment {
    /// The value of `shape` `text` is a lexical form of [Schema2 §3.2.7.1,
    /// §3.2.8.1, §3.2.9.1]; none if it is none. A time of 24:00:00 is
    /// 00:00:00, of the next day for a date and time [XQueryFO §10.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, err:FODT0001, of a year negative or past 9999.
    pub fn parse(text: &str, shape: Shape) -> Result<Option<Moment>> {
        let Some((body, zone)) = zone_of(text) else { return Ok(None) };
        let Some((date, time)) = split(body, shape) else { return Ok(None) };
        let days = match date {
            Some(d) => match date_part(&d)? {
                Some(days) => days,
                None => return Ok(None),
            },
            None => 0,
        };
        let Some(micros) = time.map_or(Some(0), time_part) else { return Ok(None) };
        let moment = Moment { shape, days, micros, zone }.normalized();
        if moment.year() > LAST_YEAR {
            return Err(beyond(text));
        }
        Ok(Some(moment))
    }

    /// Its local value in microseconds from 0001-01-01T00:00:00, and its
    /// time zone, as the order of [Schema2 §3.2.7.3] reads it.
    #[must_use]
    pub fn local(&self) -> (i128, Option<i16>) {
        let micros = i128::from(self.days).saturating_mul(i128::from(MICROS_PER_DAY)).saturating_add(i128::from(self.micros));
        (micros, self.zone)
    }

    /// It with its time within a day: a whole day of time carried to its
    /// date, or dropped of a time.
    fn normalized(self) -> Moment {
        let carried = self.micros.checked_div_euclid(MICROS_PER_DAY).unwrap_or(0);
        let micros = self.micros.checked_rem_euclid(MICROS_PER_DAY).unwrap_or(0);
        let days = if self.shape == Shape::Time { 0 } else { self.days.saturating_add(carried) };
        Moment { days, micros, ..self }
    }

    /// The year of its date.
    #[must_use]
    pub fn year(&self) -> i64 {
        civil_from_days(self.days).0
    }

    /// Its local value adjusted to UTC and its time zone removed, as
    /// `fn:adjust-dateTime-to-timezone` to PT0H then to none makes it
    /// [ISO/IEC 9075-14:2006 draft §6.6 GR4j i to iii] [XQueryFO §10.7]: a
    /// value with no time zone as it is; a date as the date of its start
    /// in UTC.
    #[must_use]
    pub fn in_utc(&self) -> Moment {
        let Some(zone) = self.zone else { return *self };
        let shift = i64::from(zone).saturating_mul(MICROS_PER_MINUTE);
        let utc = Moment { micros: self.micros.saturating_sub(shift), zone: None, ..*self }.normalized();
        if self.shape == Shape::Date { Moment { micros: 0, ..utc } } else { utc }
    }

    /// Its string [XQueryFO §17.1.2]: its local value, the year of four
    /// digits, the seconds' fraction without zeros trailing, and its time
    /// zone, Z if zero.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        match self.shape {
            Shape::Date | Shape::DateTime => out.push_str(&self.ymd()),
            Shape::Time => {}
            Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth => out.push_str(&self.gregorian_text()),
        }
        if self.shape == Shape::DateTime {
            out.push('T');
        }
        if matches!(self.shape, Shape::Time | Shape::DateTime) {
            out.push_str(&self.clock());
        }
        if let Some(zone) = self.zone {
            out.push_str(&zone_text(zone));
        }
        out
    }

    /// Its date, yyyy-mm-dd.
    fn ymd(&self) -> String {
        let (year, month, day) = civil_from_days(self.days);
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// Its time of day, hh:mm:ss, a fraction after if it has one.
    #[must_use]
    pub fn clock(&self) -> String {
        let seconds = self.micros.checked_div(MICROS_PER_SECOND).unwrap_or(0);
        let fraction = self.micros.checked_rem(MICROS_PER_SECOND).unwrap_or(0);
        let minutes = seconds.checked_div(SIXTY).unwrap_or(0);
        let (hour, minute, second) =
            (minutes.checked_div(SIXTY).unwrap_or(0), minutes.checked_rem(SIXTY).unwrap_or(0), seconds.checked_rem(SIXTY).unwrap_or(0));
        let fraction = format!("{fraction:06}");
        let fraction = fraction.trim_end_matches('0');
        let dot = if fraction.is_empty() { "" } else { "." };
        format!("{hour:02}:{minute:02}:{second:02}{dot}{fraction}")
    }

    /// The string of an SQL literal of it [ISO/IEC 9075:1992 §5.3]: its
    /// date, its time, a space between, then its time zone as ±hh:mm.
    #[must_use]
    pub fn sql_literal(&self) -> String {
        let mut out = String::new();
        if matches!(self.shape, Shape::Date | Shape::DateTime) {
            out.push_str(&self.ymd());
        }
        if self.shape == Shape::DateTime {
            out.push(' ');
        }
        if matches!(self.shape, Shape::Time | Shape::DateTime) {
            out.push_str(&self.clock());
        }
        if let Some(zone) = self.zone {
            out.push_str(&crate::value::datetime::Datetime::displacement_text(zone));
        }
        out
    }

    /// `value`, a date, time, date and time or Gregorian value, cast as
    /// `to`, another of them [XQueryFO §17.1.5]: a date and time's date or
    /// time, a date at 00:00:00, or the Gregorian value of a date's or a
    /// date and time's fields, each with its time zone; none of another
    /// cast.
    #[must_use]
    pub fn convert(value: &Atomic, to: Primitive) -> Option<String> {
        let from = Shape::of(value.kind.primitive()).filter(|s| matches!(s, Shape::Date | Shape::DateTime))?;
        let target = Shape::of(to)?;
        let m = Moment::parse(&value.lexical, from).ok().flatten()?;
        if target.gregorian() {
            return Some(m.to_gregorian(target).text());
        }
        let cast = match (from, target) {
            (Shape::DateTime, Shape::Date) => Moment { shape: Shape::Date, micros: 0, ..m },
            (Shape::DateTime, Shape::Time) => Moment { shape: Shape::Time, days: 0, ..m },
            (Shape::Date, Shape::DateTime) => Moment { shape: Shape::DateTime, ..m },
            (
                Shape::Date
                | Shape::Time
                | Shape::DateTime
                | Shape::GYearMonth
                | Shape::GYear
                | Shape::GMonthDay
                | Shape::GDay
                | Shape::GMonth,
                _,
            ) => return None,
        };
        Some(cast.text())
    }
}

impl Shape {
    /// The shape of values of `p`; none if it is no date or time type.
    #[must_use]
    pub fn of(p: Primitive) -> Option<Shape> {
        Some(match p {
            Primitive::Date => Shape::Date,
            Primitive::Time => Shape::Time,
            Primitive::DateTime => Shape::DateTime,
            Primitive::GYearMonth => Shape::GYearMonth,
            Primitive::GYear => Shape::GYear,
            Primitive::GMonthDay => Shape::GMonthDay,
            Primitive::GDay => Shape::GDay,
            Primitive::GMonth => Shape::GMonth,
            Primitive::Untyped
            | Primitive::String
            | Primitive::Boolean
            | Primitive::Decimal
            | Primitive::Integer
            | Primitive::Float
            | Primitive::Double
            | Primitive::Duration
            | Primitive::YearMonthDuration
            | Primitive::DayTimeDuration
            | Primitive::HexBinary
            | Primitive::Base64Binary
            | Primitive::AnyUri
            | Primitive::QName
            | Primitive::Notation => return None,
        })
    }

    /// Whether it is a Gregorian type's.
    #[must_use]
    pub fn gregorian(self) -> bool {
        matches!(self, Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth)
    }
}

/// The date and the time of `body`, a value of `shape` without its time
/// zone; none if it has not the parts of one.
fn split(body: &str, shape: Shape) -> Option<(Option<String>, Option<&str>)> {
    Some(match shape {
        Shape::Date => (Some(body.to_owned()), None),
        Shape::Time => (None, Some(body)),
        Shape::DateTime => {
            let (date, time) = body.split_once('T')?;
            (Some(date.to_owned()), Some(time))
        }
        Shape::GYearMonth | Shape::GYear | Shape::GMonthDay | Shape::GDay | Shape::GMonth => {
            (Some(super::gregorian::as_date(body, shape)?), None)
        }
    })
}

/// The time zone `zone`, in minutes: Z if zero [Schema2 §3.2.7.2], else
/// its sign and hh:mm [XQueryFO §17.1.2].
fn zone_text(zone: i16) -> String {
    if zone == 0 {
        return "Z".to_owned();
    }
    let size = zone.unsigned_abs();
    let (hours, minutes) = (size.checked_div(60).unwrap_or(0), size.checked_rem(60).unwrap_or(0));
    format!("{}{hours:02}:{minutes:02}", if zone < 0 { '-' } else { '+' })
}
