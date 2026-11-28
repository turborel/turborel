//! \[DESIGN\] (RULES.md D1320) The bodies over vectors of the functions
//! of datetimes a statement binds (`binder/datetime.rs`): EXTRACT of a
//! field [ISO/IEC 9075:1992 §6.6] and CAST of a datetime to a datetime
//! type [§6.10 GR9, GR10, GR11]. Each reads a vector's microseconds in
//! UTC and its displacements as integers and gives integers, no value
//! made of a row, and gives of each row what the function's call gives
//! of that row's value, which the unit tests compare over the edges of
//! the types.
//!
//! A field of the time of day is arithmetic alone. A day is 24 whole
//! hours and an hour 60 whole minutes, so the hour is the local hours
//! modulo 24, the minute the local minutes modulo 60 and the second what
//! is left of the local microseconds after its minutes, each as
//! `Datetime::field` gives it after it has taken the day off. YEAR, MONTH
//! and DAY ask the calendar of each row's day (`civil_from_days`). The
//! fields of the displacement read the displacement alone.
//!
//! A cast whose value would leave the years 0001 to 9999 [§6.1 GR6], of
//! a row not null, gives no vector, so the rows are cast one by one and
//! the first of them raises as it does a row at a time.

use crate::ast::ExtractField;
use crate::storage::Datetimes;
use crate::value::datetime::{
    Field, LAST_INSTANT, MAX_TIME_PRECISION, MICROS_PER_DAY, MICROS_PER_MINUTE, Moment, Now, civil_from_days, floor_div, floor_rem,
};

/// Microseconds in an hour.
const MICROS_PER_HOUR: i64 = 60 * MICROS_PER_MINUTE;

/// The microseconds a displacement of `zone` minutes moves a datetime by.
fn shift(zone: i16) -> i64 {
    i64::from(zone).saturating_mul(MICROS_PER_MINUTE)
}

/// What `f` gives of each row's microseconds in UTC and displacement, the
/// displacement 0 of a vector that keeps none: none if the vector keeps
/// displacements of other rows than it has.
fn of_each<T>(datetimes: &Datetimes, mut f: impl FnMut(i64, i16) -> T) -> Option<Vec<T>> {
    match &datetimes.zones {
        None => Some(datetimes.utc.iter().map(|&utc| f(utc, 0)).collect()),
        Some(zones) => {
            (zones.len() == datetimes.utc.len()).then(|| datetimes.utc.iter().zip(zones).map(|(&utc, &zone)| f(utc, zone)).collect())
        }
    }
}

/// [ISO/IEC 9075:1992 §6.6 GR3a] `field` of each of `datetimes`, as
/// `Datetime::field` gives it of the value as shown, SECOND in units of
/// `unit` microseconds; of TIMEZONE_HOUR and TIMEZONE_MINUTE, the hours
/// and minutes of each displacement, of its sign [GR3a ii]. A null's is
/// of no meaning. None if the type has no such field, or the vector keeps
/// displacements of other rows than it has.
pub(crate) fn fields(datetimes: &Datetimes, field: ExtractField, unit: i64) -> Option<Vec<i64>> {
    let (dated, timed) = (datetimes.moment != Moment::Time, datetimes.moment != Moment::Date);
    let local = |utc: i64, zone: i16| utc.saturating_add(shift(zone));
    let day = |utc: i64, zone: i16| civil_from_days(floor_div(local(utc, zone), MICROS_PER_DAY));
    match field {
        ExtractField::TimezoneHour => of_each(datetimes, |_, zone| i64::from(zone.checked_div(60).unwrap_or(0))),
        ExtractField::TimezoneMinute => of_each(datetimes, |_, zone| i64::from(zone.checked_rem(60).unwrap_or(0))),
        ExtractField::Datetime(Field::Year) => dated.then(|| of_each(datetimes, |utc, zone| day(utc, zone).0))?,
        ExtractField::Datetime(Field::Month) => dated.then(|| of_each(datetimes, |utc, zone| day(utc, zone).1))?,
        ExtractField::Datetime(Field::Day) => dated.then(|| of_each(datetimes, |utc, zone| day(utc, zone).2))?,
        ExtractField::Datetime(Field::Hour) => {
            timed.then(|| of_each(datetimes, |utc, zone| floor_rem(floor_div(local(utc, zone), MICROS_PER_HOUR), 24)))?
        }
        ExtractField::Datetime(Field::Minute) => {
            timed.then(|| of_each(datetimes, |utc, zone| floor_rem(floor_div(local(utc, zone), MICROS_PER_MINUTE), 60)))?
        }
        ExtractField::Datetime(Field::Second) => {
            timed.then(|| of_each(datetimes, |utc, zone| floor_rem(local(utc, zone), MICROS_PER_MINUTE).checked_div(unit).unwrap_or(0)))?
        }
    }
}

/// Which rule of [ISO/IEC 9075:1992 §6.10] a cast of a datetime to a
/// datetime type follows.
#[derive(Clone, Copy)]
enum Rule {
    /// [GR9] To a date: its day as shown.
    Day,
    /// [GR11b] A date to a timestamp: its midnight in the session's
    /// displacement.
    Midnight,
    /// [GR10b, c; GR11d] A time or timestamp to a time, or a timestamp to
    /// a timestamp: its instant.
    Instant,
    /// [GR11c] A time to a timestamp: on the current date.
    Today,
}

/// A cast of datetimes to a datetime type, as `convert` casts one
/// (`value/types/datetime.rs`).
struct Cast {
    rule: Rule,
    /// Whether the values are times of day, and whether the type cast to is.
    times: (bool, bool),
    /// Whether a value keeps the displacement it has as it is shown: one
    /// of a type WITH TIME ZONE, or a date.
    keeps: bool,
    /// Whether the type cast to is WITH TIME ZONE.
    zoned: bool,
    /// The session's displacement, and its current date in days.
    session: (i16, i64),
    /// The microseconds the precision cast to keeps whole.
    unit: i64,
}

impl Cast {
    /// The cast of datetimes of `from` to `(moment, precision, zoned)` in
    /// the session `now` gives: none of a date to a time or a time to a
    /// date, which no cast takes [§6.10 SR4].
    fn of(from: (Moment, bool), (moment, precision, zoned): (Moment, u32, bool), now: Now) -> Option<Cast> {
        let rule = match (from.0, moment) {
            (Moment::Date | Moment::Timestamp, Moment::Date) => Rule::Day,
            (Moment::Date, Moment::Timestamp) => Rule::Midnight,
            (Moment::Time | Moment::Timestamp, Moment::Time) | (Moment::Timestamp, Moment::Timestamp) => Rule::Instant,
            (Moment::Time, Moment::Timestamp) => Rule::Today,
            (Moment::Date, Moment::Time) | (Moment::Time, Moment::Date) => return None,
        };
        let today = floor_div(now.utc.saturating_add(shift(now.zone)), MICROS_PER_DAY);
        Some(Cast {
            rule,
            times: (from.0 == Moment::Time, moment == Moment::Time),
            keeps: from.1 || from.0 == Moment::Date,
            zoned,
            session: (now.zone, today),
            unit: 10_i64.checked_pow(MAX_TIME_PRECISION.saturating_sub(precision)).unwrap_or(1),
        })
    }

    /// The microseconds in UTC and the displacement of the value of `utc`
    /// and `zone` cast, and whether its year, in UTC and as shown, is from
    /// 0001 to 9999 [§6.1 GR6], as every time is.
    fn row(&self, utc: i64, zone: i16) -> (i64, i16, bool) {
        let shown = if self.keeps { zone } else { self.session.0 };
        let taken = if self.zoned { shown } else { self.session.0 };
        let local = utc.saturating_add(shift(shown));
        let local = if self.times.0 { floor_rem(local, MICROS_PER_DAY) } else { local };
        let midnight = floor_div(local, MICROS_PER_DAY).saturating_mul(MICROS_PER_DAY);
        let (made, carried) = match self.rule {
            Rule::Day => (midnight, 0),
            Rule::Midnight => (midnight.saturating_sub(shift(self.session.0)), self.session.0),
            Rule::Instant => (utc.saturating_add(shift(taken)).saturating_sub(shift(taken)), taken),
            Rule::Today => {
                let at = self.session.1.saturating_mul(MICROS_PER_DAY).saturating_add(floor_rem(local, MICROS_PER_DAY));
                (at.saturating_sub(shift(taken)), taken)
            }
        };
        let made = if self.times.1 { floor_rem(made, MICROS_PER_DAY) } else { made };
        let cut = made.saturating_sub(floor_rem(made, self.unit));
        let within = |micros: i64| (0..=LAST_INSTANT).contains(&micros);
        (cut, carried, self.times.1 || (within(cut) && within(cut.saturating_add(shift(carried)))))
    }
}

/// [ISO/IEC 9075:1992 §6.10 GR9, GR10, GR11] `datetimes` CAST to the type
/// `to`, its fields, precision and whether WITH TIME ZONE, in the session
/// `now` gives: each row's as `cast_at` casts its value, with the
/// displacement that value carries. None if a row not null would raise,
/// its year past 0001 to 9999 or the cast one no datetime takes, or the
/// vector keeps displacements of other rows than it has.
pub(crate) fn cast(datetimes: &Datetimes, to: (Moment, u32, bool), now: Now) -> Option<Datetimes> {
    let cast = Cast::of((datetimes.moment, datetimes.zoned), to, now)?;
    let (mut sound, mut zones) = (true, Vec::with_capacity(datetimes.utc.len()));
    let utc = of_each(datetimes, |utc, zone| {
        let (made, carried, within) = cast.row(utc, zone);
        sound &= within;
        zones.push(carried);
        made
    })?;
    if !sound && !sound_but_nulls(datetimes, &cast) {
        return None;
    }
    let precision = u8::try_from(to.1.min(MAX_TIME_PRECISION)).unwrap_or(0);
    Some(match cast.rule {
        Rule::Day => Datetimes::new(utc, None, (Moment::Date, false, precision), datetimes.nulls.clone()),
        Rule::Midnight | Rule::Instant | Rule::Today => Datetimes::new(utc, Some(zones), (to.0, to.2, precision), datetimes.nulls.clone()),
    })
}

/// Whether every row of `datetimes` whose cast leaves the years is null,
/// its slot of no meaning.
fn sound_but_nulls(datetimes: &Datetimes, cast: &Cast) -> bool {
    let Some(nulls) = &datetimes.nulls else { return false };
    let zone = |n: usize| datetimes.zones.as_ref().and_then(|z| z.get(n).copied()).unwrap_or(0);
    datetimes.utc.iter().enumerate().all(|(n, &utc)| nulls.get(n) || cast.row(utc, zone(n)).2)
}

// Unit tests: each body against the function's call of each value, over
// the edges of the types (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
