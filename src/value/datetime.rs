//! [ISO/IEC 9075:1992 §4.5.1] A datetime's value: a date, a time, or a
//! timestamp, of the Gregorian calendar, years 0001 to 9999 [§6.1 Table
//! 10], seconds to the microsecond.
//!
//! A time or timestamp is kept in UTC [§4.5.1], with a time zone
//! displacement: its own, of a type WITH TIME ZONE; else the SQL-session's,
//! implied [§6.1 GR5], which the value carries only as it is shown. A date
//! has none. So two values of one instant compare equal whatever their
//! displacements.
//!
//! `utc` is microseconds: of a timestamp, from 0001-01-01 00:00:00 UTC; of a
//! time, from midnight UTC, within a day; of a date, its midnight's, from
//! 0001-01-01.

use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;

/// Microseconds in a second.
pub const MICROS_PER_SECOND: i64 = 1_000_000;
/// Microseconds in a minute.
pub const MICROS_PER_MINUTE: i64 = 60 * MICROS_PER_SECOND;
/// Microseconds in a day.
pub const MICROS_PER_DAY: i64 = 24 * 60 * MICROS_PER_MINUTE;
/// [ISO/IEC 9075:1992 §6.1 SR26] The greatest `<time fractional seconds
/// precision>`, implementation-defined, at least 6: microseconds.
pub const MAX_TIME_PRECISION: u32 = 6;
/// [ISO/IEC 9075:1992 §5.3 SR25] The least time zone displacement, -12:59,
/// in minutes.
pub const MIN_DISPLACEMENT: i16 = -(12 * 60 + 59);
/// [ISO/IEC 9075:1992 §5.3 SR25] The greatest, +13:00.
pub const MAX_DISPLACEMENT: i16 = 13 * 60;
/// Days from 0001-01-01 to 1970-01-01.
const UNIX_EPOCH_DAYS: i64 = 719_162;
/// The last day of 9999, from 0001-01-01.
const LAST_DAY: i64 = 3_652_058;

/// What the SQL-session gives a statement's datetimes: the instant it
/// runs at, as a timestamp's `utc` [ISO/IEC 9075:1992 §6.8 GR3], and its
/// time zone displacement, in minutes [§4.5.1]. By default, 0001-01-01 in
/// UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Now {
    /// The instant.
    pub utc: i64,
    /// The displacement.
    pub zone: i16,
}

/// \[DESIGN\] `Now` as a statement is bound with it: reading its instant
/// notes, where a note is asked for, that the binding holds the instant, so
/// a plan made of it is not kept for another execution (RULES.md D45). Its
/// displacement is read freely: what keeps a plan compares it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clocked<'a> {
    now: Now,
    read: Option<&'a Cell<bool>>,
}

impl<'a> Clocked<'a> {
    /// `now`, its instant's reads noted in `read`, if given.
    pub fn new(now: Now, read: Option<&'a Cell<bool>>) -> Clocked<'a> {
        Clocked { now, read }
    }

    /// The instant and the displacement, the read noted.
    pub fn instant(self) -> Now {
        self.note();
        self.now
    }

    /// Notes the instant read: of a value made of it elsewhere, as a
    /// column's default.
    pub fn note(self) {
        if let Some(read) = self.read {
            read.set(true);
        }
    }

    /// The displacement alone.
    pub fn zone(self) -> i16 {
        self.now.zone
    }

    /// Where its reads are noted, if anywhere.
    pub fn noted(self) -> Option<&'a Cell<bool>> {
        self.read
    }
}

impl From<Now> for Clocked<'_> {
    fn from(now: Now) -> Self {
        Clocked { now, read: None }
    }
}

/// Which datetime type a value is of [ISO/IEC 9075:1992 §4.5.1]: which
/// fields it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Moment {
    /// YEAR, MONTH and DAY.
    Date,
    /// HOUR, MINUTE and SECOND.
    Time,
    /// YEAR to SECOND.
    Timestamp,
}

/// A datetime value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Datetime {
    /// Which fields it has.
    pub moment: Moment,
    /// Microseconds in UTC, as the module's documentation says.
    pub utc: i64,
    /// Its time zone displacement in minutes: its own if `zoned`, else the
    /// one it is shown in.
    pub zone: i16,
    /// Whether it is of a type WITH TIME ZONE.
    pub zoned: bool,
    /// The digits of its seconds' fraction shown.
    pub precision: u8,
}

/// The fields of a datetime as written: its local microseconds, as `utc`
/// holds them, and its time zone displacement, if written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// Which fields.
    pub moment: Moment,
    /// Local microseconds.
    pub local: i64,
    /// The `<time zone interval>`, in minutes, if written.
    pub zone: Option<i16>,
    /// The digits of its `<seconds fraction>`.
    pub precision: u32,
}

/// `a` divided by `b`, rounded down; `b` is a positive constant.
fn floor_div(a: i64, b: i64) -> i64 {
    a.checked_div_euclid(b).unwrap_or(0)
}

/// What is left of `a` after `floor_div(a, b)`, from 0 to below `b`.
fn floor_rem(a: i64, b: i64) -> i64 {
    a.checked_rem_euclid(b).unwrap_or(0)
}

/// Whether `year` is a leap year of the Gregorian calendar.
fn leap(year: i64) -> bool {
    floor_rem(year, 4) == 0 && (floor_rem(year, 100) != 0 || floor_rem(year, 400) == 0)
}

/// Days in `month` of `year`.
pub(crate) fn days_in(year: i64, month: i64) -> i64 {
    match month {
        2 => {
            if leap(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 0001-01-01 to `year`-`month`-`day`, valid.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Years before it, then months of this year before it.
    let y = year.saturating_sub(1);
    let before = y.saturating_mul(365).saturating_add(floor_div(y, 4)).saturating_sub(floor_div(y, 100)).saturating_add(floor_div(y, 400));
    let months: i64 = (1..month).map(|m| days_in(year, m)).sum();
    before.saturating_add(months).saturating_add(day).saturating_sub(1)
}

/// The year, month and day of `days` from 0001-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Days from 0000-03-01, the start of a 400-year era's first year.
    let z = days.saturating_add(306);
    let era = floor_div(z, 146_097);
    let doe = floor_rem(z, 146_097);
    let yoe = floor_div(
        doe.saturating_sub(floor_div(doe, 1460)).saturating_add(floor_div(doe, 36_524)).saturating_sub(floor_div(doe, 146_096)),
        365,
    );
    let doy = doe.saturating_sub(yoe.saturating_mul(365).saturating_add(floor_div(yoe, 4)).saturating_sub(floor_div(yoe, 100)));
    let mp = floor_div(doy.saturating_mul(5).saturating_add(2), 153);
    let day = doy.saturating_sub(floor_div(mp.saturating_mul(153).saturating_add(2), 5)).saturating_add(1);
    let month = if mp < 10 { mp.saturating_add(3) } else { mp.saturating_sub(9) };
    let year = era.saturating_mul(400).saturating_add(yoe).saturating_add(i64::from(month <= 2));
    (year, month, day)
}

/// `count` digits of `text` from `at`, as a number; none if one is not a
/// digit.
fn digits(text: &[u8], at: usize, count: usize) -> Option<i64> {
    let part = text.get(at..at.checked_add(count)?)?;
    part.iter().try_fold(0_i64, |n, &b| b.is_ascii_digit().then(|| n.saturating_mul(10).saturating_add(i64::from(b.saturating_sub(b'0')))))
}

/// [ISO/IEC 9075:1992 §5.3] `<date value>`, YYYY-MM-DD [SR21, SR22], at
/// the start of `text`: its days from 0001-01-01.
fn date_value(text: &[u8]) -> Option<i64> {
    let (year, month, day) = (digits(text, 0, 4)?, digits(text, 5, 2)?, digits(text, 8, 2)?);
    let dashes = text.get(4) == Some(&b'-') && text.get(7) == Some(&b'-');
    let valid = dashes && (1..=12).contains(&month) && (1..=days_in(year, month)).contains(&day) && year >= 1;
    valid.then(|| days_from_civil(year, month, day))
}

/// [ISO/IEC 9075:1992 §5.3] `<time value>`, HH:MM:SS\[.F...\], then an
/// optional `<time zone interval>`, ±HH:MM, all of `text`: its
/// microseconds, its displacement and its fraction's digits. Seconds run
/// to 59: no leap second is taken.
fn time_value(text: &[u8]) -> Option<(i64, Option<i16>, u32)> {
    let (hour, minute, second) = (digits(text, 0, 2)?, digits(text, 3, 2)?, digits(text, 6, 2)?);
    if text.get(2) != Some(&b':') || text.get(5) != Some(&b':') || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let rest = text.get(8..)?;
    let (fraction, rest) = match rest.split_first() {
        Some((b'.', after)) => after.split_at(after.iter().position(|b| !b.is_ascii_digit()).unwrap_or(after.len())),
        _ => (&[][..], rest),
    };
    let precision = u32::try_from(fraction.len()).ok()?;
    // Digits past the microsecond are dropped.
    let micros = fraction
        .iter()
        .chain(std::iter::repeat(&b'0'))
        .take(6)
        .fold(0_i64, |n, &b| n.saturating_mul(10).saturating_add(i64::from(b.saturating_sub(b'0'))));
    let zone = if rest.is_empty() { None } else { Some(displacement(rest)?) };
    let seconds = hour.saturating_mul(3600).saturating_add(minute.saturating_mul(60)).saturating_add(second);
    Some((seconds.saturating_mul(MICROS_PER_SECOND).saturating_add(micros), zone, precision))
}

/// [ISO/IEC 9075:1992 §5.3 SR25] `<time zone interval>`, ±HH:MM, all of
/// `text`, from -12:59 to +13:00: in minutes.
fn displacement(text: &[u8]) -> Option<i16> {
    let negative = match text.first()? {
        b'+' => false,
        b'-' => true,
        _ => return None,
    };
    let (hours, minutes) = (digits(text, 1, 2)?, digits(text, 4, 2)?);
    if text.len() != 6 || text.get(3) != Some(&b':') || minutes > 59 {
        return None;
    }
    let size = i16::try_from(hours.saturating_mul(60).saturating_add(minutes)).ok()?;
    let zone = if negative { size.saturating_neg() } else { size };
    (MIN_DISPLACEMENT..=MAX_DISPLACEMENT).contains(&zone).then_some(zone)
}

/// [ISO/IEC 9075:1992 §5.3 SR25] `text` as a time zone displacement,
/// ±HH:MM, in minutes; none if it is not one in range.
pub fn parse_displacement(text: &str) -> Option<i16> {
    displacement(text.as_bytes())
}

/// [ISO/IEC 9075:1992 §5.3] The fields `text` writes, the body of a
/// literal of `moment`: a `<date string>`'s, a `<time string>`'s or a
/// `<timestamp string>`'s, without its quotes; none if it writes no valid
/// datetime [SR21, SR22, SR25].
pub fn parse(moment: Moment, text: &str) -> Option<Written> {
    let bytes = text.as_bytes();
    let (local, zone, precision) = match moment {
        Moment::Date => (date_value(bytes)?.saturating_mul(MICROS_PER_DAY), None, 0),
        Moment::Time => time_value(bytes)?,
        Moment::Timestamp => {
            let days = date_value(bytes)?;
            if bytes.get(10) != Some(&b' ') {
                return None;
            }
            let (micros, zone, precision) = time_value(bytes.get(11..)?)?;
            (days.saturating_mul(MICROS_PER_DAY).saturating_add(micros), zone, precision)
        }
    };
    if moment == Moment::Date && bytes.len() != 10 {
        return None;
    }
    Some(Written { moment, local, zone, precision })
}

impl Datetime {
    /// A value of `moment` of `local` microseconds in a displacement of
    /// `zone` minutes, its own if `zoned`, showing `precision` digits.
    pub fn of_local(moment: Moment, local: i64, zone: i16, zoned: bool, precision: u32) -> Datetime {
        let shift = if moment == Moment::Date { 0 } else { i64::from(zone).saturating_mul(MICROS_PER_MINUTE) };
        let utc = local.saturating_sub(shift);
        let utc = if moment == Moment::Time { floor_rem(utc, MICROS_PER_DAY) } else { utc };
        let precision = u8::try_from(precision.min(MAX_TIME_PRECISION)).unwrap_or(0);
        Datetime { moment, utc, zone: if moment == Moment::Date { 0 } else { zone }, zoned, precision }
    }

    /// Its local microseconds: `utc` in its displacement.
    pub fn local(&self) -> i64 {
        let local = self.utc.saturating_add(i64::from(self.zone).saturating_mul(MICROS_PER_MINUTE));
        if self.moment == Moment::Time { floor_rem(local, MICROS_PER_DAY) } else { local }
    }

    /// [ISO/IEC 9075:1992 §6.1 GR6] Whether its year, in UTC and as shown,
    /// is from 0001 to 9999.
    pub fn in_range(&self) -> bool {
        self.moment == Moment::Time || [self.utc, self.local()].iter().all(|m| (0..=LAST_DAY).contains(&floor_div(*m, MICROS_PER_DAY)))
    }

    /// It with no more than `precision` digits of the second's fraction,
    /// the rest dropped, and shown with that many.
    #[must_use]
    pub fn truncated(&self, precision: u32) -> Datetime {
        let unit = 10_i64.checked_pow(MAX_TIME_PRECISION.saturating_sub(precision)).unwrap_or(1);
        let utc = self.utc.saturating_sub(floor_rem(self.utc, unit));
        Datetime { utc, precision: u8::try_from(precision.min(MAX_TIME_PRECISION)).unwrap_or(0), ..*self }
    }

    /// The value of `field` of it, as shown: YEAR, MONTH, DAY, HOUR or
    /// MINUTE a whole number, SECOND in microseconds [ISO/IEC 9075:1992
    /// §6.6 GR3a i]; none if it has no such field.
    pub fn field(&self, field: Field) -> Option<i64> {
        let local = self.local();
        let (date, time) = (floor_div(local, MICROS_PER_DAY), floor_rem(local, MICROS_PER_DAY));
        let (year, month, day) = civil_from_days(date);
        let has_date = self.moment != Moment::Time;
        let has_time = self.moment != Moment::Date;
        match field {
            Field::Year => has_date.then_some(year),
            Field::Month => has_date.then_some(month),
            Field::Day => has_date.then_some(day),
            Field::Hour => has_time.then(|| floor_div(time, 60 * MICROS_PER_MINUTE)),
            Field::Minute => has_time.then(|| floor_rem(floor_div(time, MICROS_PER_MINUTE), 60)),
            Field::Second => has_time.then(|| floor_rem(time, MICROS_PER_MINUTE)),
        }
    }

    /// The date it falls on, as shown: its local days from 0001-01-01.
    pub fn days(&self) -> i64 {
        floor_div(self.local(), MICROS_PER_DAY)
    }

    /// The time of day it is, as shown: local microseconds from midnight.
    pub fn time_of_day(&self) -> i64 {
        floor_rem(self.local(), MICROS_PER_DAY)
    }

    /// The SQL key word of its type: DATE, TIME or TIMESTAMP.
    pub fn key_word(&self) -> &'static str {
        match self.moment {
            Moment::Date => "DATE",
            Moment::Time => "TIME",
            Moment::Timestamp => "TIMESTAMP",
        }
    }

    /// [ISO/IEC 9075:1992 §8.2 GR5] Its order against `other`'s, of the
    /// same fields: of their instants in UTC.
    pub fn compare(&self, other: &Datetime) -> Option<Ordering> {
        (self.moment == other.moment).then(|| self.utc.cmp(&other.utc))
    }

    /// The time zone displacement `zone` as written, ±HH:MM.
    pub fn displacement_text(zone: i16) -> String {
        let size = zone.unsigned_abs();
        format!("{}{:02}:{:02}", if zone < 0 { '-' } else { '+' }, size.checked_div(60).unwrap_or(0), size.checked_rem(60).unwrap_or(0))
    }
}

/// A `<datetime field>` [ISO/IEC 9075:1992 §4.5.1, Table 4].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// YEAR.
    Year,
    /// MONTH.
    Month,
    /// DAY.
    Day,
    /// HOUR.
    Hour,
    /// MINUTE.
    Minute,
    /// SECOND.
    Second,
}

/// [ISO/IEC 9075:1992 §6.10 GR5e] Its shortest literal's string, the
/// fields as shown: YYYY-MM-DD, HH:MM:SS with as many digits of the
/// fraction as its precision, and its displacement if of a type WITH TIME
/// ZONE.
impl fmt::Display for Datetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.days());
        let time = self.time_of_day();
        if self.moment != Moment::Time {
            write!(f, "{year:04}-{month:02}-{day:02}")?;
        }
        if self.moment == Moment::Date {
            return Ok(());
        }
        if self.moment == Moment::Timestamp {
            f.write_str(" ")?;
        }
        let seconds = floor_div(time, MICROS_PER_SECOND);
        let (hour, minute, second) = (floor_div(seconds, 3600), floor_rem(floor_div(seconds, 60), 60), floor_rem(seconds, 60));
        write!(f, "{hour:02}:{minute:02}:{second:02}")?;
        if self.precision > 0 {
            let fraction = format!("{:06}", floor_rem(time, MICROS_PER_SECOND));
            write!(f, ".{}", fraction.get(..usize::from(self.precision)).unwrap_or_default())?;
        }
        if self.zoned {
            f.write_str(&Datetime::displacement_text(self.zone))?;
        }
        Ok(())
    }
}

/// [ISO/IEC 9075:1992 §4.5.1, §6.1 GR5] Each time and timestamp of a type
/// without time zone in `row`, as a value retrieved shows it: in the
/// SQL-session's displacement, `zone`.
pub fn show_in(row: &mut [crate::value::Value], zone: i16) {
    for v in row {
        if let crate::value::Value::Datetime(d) = v
            && !d.zoned
            && d.moment != Moment::Date
        {
            d.zone = zone;
        }
    }
}

/// The instant `unix_micros`, microseconds from 1970-01-01 00:00:00 UTC,
/// as a timestamp's `utc`: from 0001-01-01.
pub fn utc_of_unix(unix_micros: i64) -> i64 {
    unix_micros.saturating_add(UNIX_EPOCH_DAYS.saturating_mul(MICROS_PER_DAY))
}

#[cfg(test)]
mod tests;
