//! [ISO/IEC 9075-2:2023 §9.50] A datetime as a template formats it: each
//! field its digits, each delimiter itself.

use super::template::{Field, Part, Template};
use crate::value::datetime::{Datetime, Field as Of, MAX_TIME_PRECISION, MICROS_PER_SECOND, days_from_civil};

/// Seconds in a minute, and minutes in an hour.
const SIXTY: i64 = 60;
/// Hours in half a day.
const HALF_DAY_HOURS: i64 = 12;
/// The digits of a year written whole [GR5b i].
const YEAR_DIGITS: usize = 4;
/// The most digits of a fraction a template writes [§9.52].
const MOST_FRACTION_DIGITS: usize = 9;

/// The last `width` digits of `n`, at least `width` of them.
fn digits(n: i64, width: usize) -> String {
    let all = format!("{:0width$}", n.unsigned_abs());
    all.chars().skip(all.chars().count().saturating_sub(width)).collect()
}

impl Template {
    /// [§9.50 GR3-GR6] `d`, its fields as shown, as this template writes
    /// it; the seconds truncated to its fractional precision (IA074).
    pub(super) fn format(&self, d: &Datetime) -> String {
        self.parts().iter().map(|part| written(*part, d, self.precision())).collect()
    }
}

/// [§9.50 GR5] What `part` writes of `d`, its seconds to `precision`
/// digits: a delimiter itself \[GR5a\].
fn written(part: Part, d: &Datetime, precision: usize) -> String {
    match part {
        Part::Delimiter(c) => c.to_string(),
        Part::Field(f) => field_text(f, d, precision),
    }
}

/// [§9.50 GR5b-GR5n] What the field `f` writes of `d`.
fn field_text(f: Field, d: &Datetime, precision: usize) -> String {
    let field = |of: Of| d.field(of).unwrap_or(0);
    let micros = field(Of::Second);
    let second = micros.checked_div(MICROS_PER_SECOND).unwrap_or(0);
    match f {
        // [GR5b] the last digits of the year's four.
        Field::Year(n) | Field::RoundedYear(n) => digits(field(Of::Year), n.min(YEAR_DIGITS)),
        // [GR5c, GR5d]
        Field::Month => digits(field(Of::Month), 2),
        Field::DayOfMonth => digits(field(Of::Day), 2),
        // [GR5e]
        Field::DayOfYear => {
            let first = days_from_civil(field(Of::Year), 1, 1);
            digits(d.days().saturating_sub(first).saturating_add(1), 3)
        }
        // [GR5f]
        Field::Hour12 => digits(twelve(field(Of::Hour)), 2),
        // [GR5g, GR5h, GR5i]
        Field::Hour24 => digits(field(Of::Hour), 2),
        Field::Minute => digits(field(Of::Minute), 2),
        Field::Second => digits(second, 2),
        // [GR5j]
        Field::SecondOfDay => {
            let minutes = field(Of::Hour).saturating_mul(SIXTY).saturating_add(field(Of::Minute));
            digits(minutes.saturating_mul(SIXTY).saturating_add(second), 5)
        }
        // [GR5k]
        Field::Fraction(n) => fraction(micros.checked_rem(MICROS_PER_SECOND).unwrap_or(0), precision.min(n)),
        // [GR5l]
        Field::AmPm => (if field(Of::Hour) < HALF_DAY_HOURS { "A.M." } else { "P.M." }).to_owned(),
        // [GR5m]
        Field::ZoneHour => {
            let sign = if d.zone < 0 { '-' } else { '+' };
            format!("{sign}{}", digits(i64::from(d.zone).checked_div(SIXTY).unwrap_or(0), 2))
        }
        // [GR5n]
        Field::ZoneMinute => digits(i64::from(d.zone).checked_rem(SIXTY).unwrap_or(0), 2),
    }
}

/// [GR5f] The hour `h` on a 12-hour clock: 12 for 0, `h` less 12 past 12.
fn twelve(h: i64) -> i64 {
    if h == 0 {
        HALF_DAY_HOURS
    } else if h > HALF_DAY_HOURS {
        h.saturating_sub(HALF_DAY_HOURS)
    } else {
        h
    }
}

/// [GR4, GR5k] The first `width` digits of the fraction of a second that
/// `micros` microseconds are, the rest dropped; zeros past the sixth.
fn fraction(micros: i64, width: usize) -> String {
    let six = digits(micros, usize::try_from(MAX_TIME_PRECISION).unwrap_or(MOST_FRACTION_DIGITS));
    format!("{six:0<MOST_FRACTION_DIGITS$}").chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE, Moment};

    /// 2024-03-05 14:07:09.123456 local, in a displacement of `zone`
    /// minutes.
    fn moment(zone: i16) -> Datetime {
        let days = days_from_civil(2024, 3, 5);
        let time = (14 * 60 + 7) * MICROS_PER_MINUTE + 9 * MICROS_PER_SECOND + 123_456;
        Datetime::of_local(Moment::Timestamp, days.saturating_mul(MICROS_PER_DAY).saturating_add(time), zone, true, 6)
    }

    fn format(template: &str, d: &Datetime) -> String {
        Template::read(template, Moment::Timestamp, true).unwrap().format(d)
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.50 GR4, GR5] each field
    /// written as its General Rule says: a year's last digits, a 12-hour
    /// hour and A.M. or P.M., the day of the year, the second of the day,
    /// the fraction truncated to the template's precision, and the time
    /// zone's sign negative if either of its fields is.
    #[test]
    fn fields_are_formatted_9_50() {
        let d = moment(330);
        assert_eq!(format("YYYY-MM-DD HH24:MI:SS.FF3", &d), "2024-03-05 14:07:09.123");
        assert_eq!(format("YYY DDD", &d), "024 065");
        assert_eq!((format("YY", &d), format("Y", &d)), ("24".to_owned(), "4".to_owned()));
        assert_eq!(format("HH12 P.M.", &d), "02 P.M.");
        assert_eq!(format("SSSSS.FF9", &d), "50829.123456000");
        assert_eq!(format("FF1", &d), "1");
        assert_eq!(format("TZH:TZM", &d), "+05:30");
        assert_eq!(format("TZH:TZM", &moment(-30)), "-00:30");
        assert_eq!(format("TZH:TZM", &moment(-750)), "-12:30");
        let midnight = Datetime::of_local(Moment::Timestamp, days_from_civil(2024, 1, 1) * MICROS_PER_DAY, 0, true, 0);
        assert_eq!(format("HH12 A.M.", &midnight), "12 A.M.");
        assert_eq!(format("HH24 DDD", &midnight), "00 001");
    }
}
