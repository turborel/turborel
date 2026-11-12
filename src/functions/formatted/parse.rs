//! [ISO/IEC 9075-2:2023 §9.51] A character string as a template reads it:
//! the template's regular expression matched against it, and each field's
//! digits taken from the group of its regular expression position.

use super::template::{Field, Part, Template, ZONE_HOUR_DIGITS};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::regex::{Captures, Regex};
use crate::value::datetime::{
    Datetime, Field as Of, MAX_DISPLACEMENT, MAX_TIME_PRECISION, MICROS_PER_DAY, MICROS_PER_SECOND, MIN_DISPLACEMENT, Moment, Now,
    civil_from_days, days_from_civil, days_in,
};

/// Seconds in a minute, and minutes in an hour.
const SIXTY: i64 = 60;
/// Hours in a day.
const DAY_HOURS: i64 = 24;
/// Hours in half a day.
const HALF_DAY_HOURS: i64 = 12;
/// The digits of a year written whole [GR6a iv].
const YEAR_DIGITS: usize = 4;
/// The greatest year of a datetime [ISO/IEC 9075-2:2023 §6.1 Table 10].
const LAST_YEAR: i64 = 9999;
/// The months of a year.
const MONTHS: i64 = 12;

/// The error of a string the template does not read, `why`.
fn invalid(rule: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.51 {rule}] {why}")).with_state(SqlState::INVALID_DATETIME_FORMAT)
}

/// The regular expression a template defines, compiled, and the regular
/// expression position of each of its fields.
pub(super) struct Pattern {
    regex: Regex,
    positions: Vec<(Field, usize)>,
}

/// What a datetime type is: which fields, its fractional precision, and
/// whether WITH TIME ZONE.
pub(super) type Target = (Moment, u32, bool);

impl Template {
    /// \[SR4\]-\[SR6\] Its regular expression, `'^' || RE1 || ... || REN ||
    /// '$'`, and of each field the number of left parens before its REi,
    /// plus one.
    ///
    /// # Errors
    ///
    /// None but of a design fault: the expression is always one.
    pub(super) fn pattern(&self) -> Result<Pattern> {
        let mut rx = String::from("^");
        let mut positions = Vec::new();
        for (i, part) in self.parts().iter().enumerate() {
            match part {
                Part::Delimiter(c) => rx.push_str(delimiter(*c)),
                Part::Field(f) => {
                    positions.push((*f, rx.matches('(').count().saturating_add(1)));
                    rx.push_str(&field_expression(*f, &self.quantifier(i)));
                }
            }
        }
        rx.push('$');
        Ok(Pattern { regex: Regex::new(&rx, "")?, positions })
    }

    /// \[SR4b, SR4c\] The quantifier of part `i`: `{1,Li}` if delimited,
    /// else `{Li}`; Li 2 of TZH, else its maximum field length.
    fn quantifier(&self, i: usize) -> String {
        let Some(part) = self.parts().get(i) else { return String::new() };
        let most = if *part == Part::Field(Field::ZoneHour) { ZONE_HOUR_DIGITS } else { Template::max_length(*part) };
        if self.delimited(i) { format!("{{1,{most}}}") } else { format!("{{{most}}}") }
    }

    /// Whether reading a string as a value of `moment` takes the current
    /// year or month [GR5]: of a date, without a year of four digits not
    /// delimited, or without a month or a day of the year.
    pub(super) fn needs_now(&self, moment: Moment) -> bool {
        if moment == Moment::Time {
            return false;
        }
        let whole_year = self
            .parts()
            .iter()
            .enumerate()
            .any(|(i, p)| matches!(p, Part::Field(Field::Year(YEAR_DIGITS) | Field::RoundedYear(YEAR_DIGITS))) && !self.delimited(i));
        !whole_year || !self.has(|f| matches!(f, Field::Month | Field::DayOfYear))
    }
}

/// \[SR4d i\] The expression of a delimiter `c`.
fn delimiter(c: char) -> &'static str {
    match c {
        '-' => "\\-",
        '.' => "\\.",
        '/' => "/",
        ',' => ",",
        '\'' => "'",
        ';' => ";",
        ':' => ":",
        _ => " ",
    }
}

/// \[SR4d ii\]-\[SR4d iv\] The expression of the field `f`, its digits
/// quantified by `q`.
fn field_expression(f: Field, q: &str) -> String {
    match f {
        Field::AmPm => "((A|P)\\.M\\.)".to_owned(),
        Field::ZoneHour => format!("((\\+|\\-| )[\\p{{Nd}}]{q})"),
        Field::Year(_)
        | Field::RoundedYear(_)
        | Field::Month
        | Field::DayOfMonth
        | Field::DayOfYear
        | Field::Hour12
        | Field::Hour24
        | Field::Minute
        | Field::Second
        | Field::SecondOfDay
        | Field::Fraction(_)
        | Field::ZoneMinute => format!("([\\p{{Nd}}]{q})"),
    }
}

/// What each field of a template matched in a string.
struct Matched {
    texts: Vec<(Field, String)>,
}

impl Matched {
    /// What the field `pick` picks matched, if the template has it.
    fn text(&self, pick: impl Fn(Field) -> bool) -> Option<&str> {
        self.texts.iter().find(|(f, _)| pick(*f)).map(|(_, t)| t.as_str())
    }

    /// The field `pick` picks as an `<unsigned integer>`, if the template
    /// has it.
    fn number(&self, pick: impl Fn(Field) -> bool) -> Result<Option<i64>> {
        self.text(pick).map(unsigned).transpose()
    }
}

/// `s`, digits, as an `<unsigned integer>`.
///
/// # Errors
///
/// SQLSTATE 22007 of a decimal digit not a `<digit>`, of no `<unsigned
/// integer>`.
fn unsigned(s: &str) -> Result<i64> {
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid("GR6-GR11", &format!("'{s}' has a decimal digit that is not 0 to 9")));
    }
    s.parse().map_err(|_| invalid("GR6-GR11", &format!("'{s}' is no unsigned integer")))
}

impl Pattern {
    /// [§9.51 GR2-GR12] `fcs` as a value of `target`, read by `template`,
    /// in the SQL-session `now` gives: its instant's year and month those
    /// not written.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22007 if the template's expression does not match it, or
    /// the fields it writes are no datetime.
    pub(super) fn parse(&self, fcs: &str, target: Target, now: Now) -> Result<Datetime> {
        let chars: Vec<char> = fcs.chars().collect();
        let Some(found) = self.regex.find_at(&chars, 0)? else {
            return Err(invalid("GR4", &format!("'{fcs}' is not of the datetime template")));
        };
        let matched = self.matched(&chars, &found);
        let (moment, precision, zoned) = target;
        let days = if moment == Moment::Time { 0 } else { date(&matched, now)? };
        let time = if moment == Moment::Date { 0 } else { time(&matched)? };
        let zone = if zoned { displacement(&matched)? } else { now.zone };
        let local = days.saturating_mul(MICROS_PER_DAY).saturating_add(time);
        let d = Datetime::of_local(moment, local, zone, zoned, precision).truncated(precision);
        if !d.in_range() {
            return Err(invalid("GR12", &format!("'{fcs}' is of a year before 0001 or after 9999 in UTC")));
        }
        Ok(d)
    }

    /// Each field's text: its group's in `found` [§9.28 GR10].
    fn matched(&self, chars: &[char], found: &Captures) -> Matched {
        let texts = self
            .positions
            .iter()
            .map(|(f, group)| {
                let (start, end) = found.get(*group).unwrap_or_default();
                (*f, chars.get(start..end).unwrap_or_default().iter().collect())
            })
            .collect();
        Matched { texts }
    }
}

/// \[GR5\]-\[GR7\], \[GR12a\] The date `m` writes, as days from 0001-01-01:
/// its year, else the current; its day of the year, else its month, else
/// the current, and day of the month, else 1.
fn date(m: &Matched, now: Now) -> Result<i64> {
    let current = Datetime { moment: Moment::Timestamp, utc: now.utc, zone: now.zone, zoned: true, precision: 0 };
    let cy = current.field(Of::Year).unwrap_or(1);
    let year = match m.text(|f| matches!(f, Field::Year(_) | Field::RoundedYear(_))) {
        // [GR6a, GR6b] prefixed by the current year's first digits; RY is
        // CY (IV096).
        Some(s) => {
            let cylit = format!("{cy:04}");
            let prefix: String = cylit.chars().take(YEAR_DIGITS.saturating_sub(s.chars().count())).collect();
            unsigned(&format!("{prefix}{s}"))?
        }
        None => cy,
    };
    if !(1..=LAST_YEAR).contains(&year) {
        return Err(invalid("GR12", &format!("the year {year} is not 0001 to 9999")));
    }
    let first = days_from_civil(year, 1, 1);
    // [GR7a] of the year read, as the day of year is shown of it.
    if let Some(n) = m.number(|f| f == Field::DayOfYear)? {
        let last = days_from_civil(year.saturating_add(1), 1, 1).saturating_sub(first);
        if !(1..=last).contains(&n) {
            return Err(invalid("GR7a iv", &format!("{year} has no day of the year {n}")));
        }
        return Ok(first.saturating_add(n).saturating_sub(1));
    }
    let (_, cm, _) = civil_from_days(current.days());
    let month = m.number(|f| f == Field::Month)?.unwrap_or(cm);
    let day = m.number(|f| f == Field::DayOfMonth)?.unwrap_or(1);
    if !(1..=MONTHS).contains(&month) || !(1..=days_in(year, month)).contains(&day) {
        return Err(invalid("GR12a", &format!("{year:04}-{month:02}-{day:02} is no date of the Gregorian calendar")));
    }
    Ok(days_from_civil(year, month, day))
}

/// \[GR8\], \[GR9\], \[GR12b\] The time of day `m` writes, in microseconds:
/// its second of the day, else its hour, minute and second, each 0 if not
/// written; and its fraction, the digits past the sixth dropped (IA074).
fn time(m: &Matched) -> Result<i64> {
    let seconds = match m.number(|f| f == Field::SecondOfDay)? {
        Some(n) => n,
        None => clock(m)?,
    };
    if !(0..DAY_HOURS.saturating_mul(SIXTY).saturating_mul(SIXTY)).contains(&seconds) {
        return Err(invalid("GR8a iv", &format!("{seconds} is no second of a day")));
    }
    let fraction = m.text(|f| matches!(f, Field::Fraction(_))).map(micros).transpose()?.unwrap_or(0);
    Ok(seconds.saturating_mul(MICROS_PER_SECOND).saturating_add(fraction))
}

/// \[GR8b\] The seconds from midnight of the hour, minute and second `m`
/// writes, each 0 if not written.
fn clock(m: &Matched) -> Result<i64> {
    let hour = hour(m)?;
    let minute = m.number(|f| f == Field::Minute)?.unwrap_or(0);
    let second = m.number(|f| f == Field::Second)?.unwrap_or(0);
    if !(0..DAY_HOURS).contains(&hour) || !(0..SIXTY).contains(&minute) || !(0..SIXTY).contains(&second) {
        return Err(invalid("GR12", &format!("{hour:02}:{minute:02}:{second:02} is no time of day")));
    }
    Ok(hour.saturating_mul(SIXTY).saturating_add(minute).saturating_mul(SIXTY).saturating_add(second))
}

/// \[GR9a\] The microseconds of `s`, the digits of a fraction of a
/// second, those past the sixth dropped (IA074).
fn micros(s: &str) -> Result<i64> {
    unsigned(s)?;
    let six: String = s.chars().take(usize::try_from(MAX_TIME_PRECISION).unwrap_or(0)).collect();
    let unit = 10_i64.checked_pow(MAX_TIME_PRECISION.saturating_sub(u32::try_from(six.len()).unwrap_or(0))).unwrap_or(1);
    Ok(unsigned(&six)?.saturating_mul(unit))
}

/// \[GR8b i\] The hour `m` writes: of HH24, or of HH12, 12 as 0, and 12
/// more after P.M.; else 0.
fn hour(m: &Matched) -> Result<i64> {
    if let Some(h) = m.number(|f| f == Field::Hour24)? {
        return Ok(h);
    }
    let Some(h12) = m.number(|f| f == Field::Hour12)? else { return Ok(0) };
    let temp = if h12 == HALF_DAY_HOURS { 0 } else { h12 };
    let pm = m.text(|f| f == Field::AmPm).is_some_and(|s| s.starts_with('P'));
    Ok(if pm { temp.saturating_add(HALF_DAY_HOURS) } else { temp })
}

/// \[GR10\], \[GR11\] The time zone displacement `m` writes, in minutes:
/// its hour signed, a space as a plus sign, and its minute of the hour's
/// sign; 0 if it writes none.
fn displacement(m: &Matched) -> Result<i16> {
    let Some(tzh) = m.text(|f| f == Field::ZoneHour) else { return Ok(0) };
    let negative = tzh.starts_with('-');
    let hours = unsigned(tzh.get(1..).unwrap_or_default())?;
    let minutes = m.number(|f| f == Field::ZoneMinute)?.unwrap_or(0);
    let total = hours.saturating_mul(SIXTY).saturating_add(minutes);
    let total = if negative { total.saturating_neg() } else { total };
    match i16::try_from(total).ok().filter(|z| (MIN_DISPLACEMENT..=MAX_DISPLACEMENT).contains(z) && minutes < SIXTY) {
        Some(zone) => Ok(zone),
        None => Err(invalid("GR12c, GR12e", &format!("{tzh}:{minutes:02} is no time zone displacement"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::datetime::MICROS_PER_MINUTE as MINUTE;

    /// 2024-07-15 10:00 UTC, in a displacement of +02:00.
    fn now() -> Now {
        let days = days_from_civil(2024, 7, 15);
        Now { utc: days.saturating_mul(MICROS_PER_DAY).saturating_add(600 * MINUTE), zone: 120 }
    }

    fn parse(template: &str, text: &str, target: Target) -> Result<Datetime> {
        Template::read(template, target.0, target.2)?.pattern()?.parse(text, target, now())
    }

    fn local(template: &str, text: &str, target: Target) -> Result<String> {
        parse(template, text, target).map(|d| d.to_string())
    }

    const STAMP: Target = (Moment::Timestamp, 6, false);
    const ZONED: Target = (Moment::Timestamp, 6, true);
    const DATE: Target = (Moment::Date, 0, false);
    const TIME: Target = (Moment::Time, 0, false);

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.51 SR4-SR6] the
    /// template's regular expression: each delimiter itself, escaped as the
    /// rule writes it; each field's digits quantified, 1 to its length if
    /// delimited, else exactly; and each field's group after the left
    /// parens before it, A.M. or P.M. and TZH two each.
    #[test]
    fn templates_define_expressions_9_51() {
        let p = Template::read("HH12A.M.MI TZH:TZM", Moment::Time, true).unwrap().pattern().unwrap();
        let groups: Vec<usize> = p.positions.iter().map(|(_, g)| *g).collect();
        assert_eq!(groups, [1, 2, 4, 5, 7]);
        assert_eq!(Template::read("YYYYMMDD", Moment::Date, false).unwrap().quantifier(0), "{4}");
        assert_eq!(Template::read("YYYY-MM", Moment::Date, false).unwrap().quantifier(2), "{1,2}");
        assert_eq!(Template::read("HH24 TZH", Moment::Time, true).unwrap().quantifier(2), "{1,2}");
        assert!(!Template::read("YYYYMMDD", Moment::Date, false).unwrap().needs_now(Moment::Date));
        assert!(Template::read("YYYY-MM-DD", Moment::Date, false).unwrap().needs_now(Moment::Date));
        assert!(Template::read("YYYYDD", Moment::Date, false).unwrap().needs_now(Moment::Date));
        assert!(!Template::read("HH24", Moment::Time, false).unwrap().needs_now(Moment::Time));
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.51 GR5-GR12] each
    /// field read as its General Rule says: a short year after the current
    /// year's first digits, the month current and the day 1 unwritten, a
    /// day of the year, a 12-hour clock with 12 as 0, the second of the day,
    /// a fraction of more digits than kept truncated, and a time zone hour
    /// of a space as a plus sign, its minute of the hour's sign.
    #[test]
    fn strings_are_read_9_51() {
        assert_eq!(local("YYYY-MM-DD HH24:MI:SS.FF9", "2023-1-2 3:04:05.123456789", STAMP).unwrap(), "2023-01-02 03:04:05.123456");
        assert_eq!(local("YY-MM-DD", "9-3-4", DATE).unwrap(), "2029-03-04");
        assert_eq!(local("RR-MM-DD", "99-3-4", DATE).unwrap(), "2099-03-04");
        assert_eq!(local("YYYY", "1999", DATE).unwrap(), "1999-07-01");
        assert_eq!(local("YYYY DDD", "2024 060", DATE).unwrap(), "2024-02-29");
        assert_eq!(local("HH12:MI A.M.", "12:30 A.M.", TIME).unwrap(), "00:30:00");
        assert_eq!(local("HH12:MI P.M.", "12:30 P.M.", TIME).unwrap(), "12:30:00");
        assert_eq!(local("HH12:MI P.M.", "1:30 P.M.", TIME).unwrap(), "13:30:00");
        assert_eq!(local("SSSSS", "3661", TIME).unwrap(), "01:01:01");
        let d = parse("YYYYMMDDHH24MITZH:TZM", "202401021530+05:30", ZONED).unwrap();
        assert_eq!((d.zone, d.to_string()), (330, "2024-01-02 15:30:00.000000+05:30".to_owned()));
        assert_eq!(parse("YYYY-MM-DD TZH:TZM", "2024-01-02 -0:30", ZONED).unwrap().zone, -30);
        assert_eq!(parse("YYYY-MM-DD", "2024-01-02", ZONED).unwrap().zone, 0, "no TZH, its displacement 0 [GR10b]");
        assert_eq!(parse("YYYY-MM-DD HH24", "2024-01-02 03", STAMP).unwrap().zone, 120, "the session's");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.51 GR4, GR12] a string
    /// the expression does not match, or whose fields are no datetime, is
    /// refused with SQLSTATE 22007: too many digits, a missing delimiter, a
    /// digit not 0 to 9, a 13th month, February 30, the 25th hour, the 367th
    /// day, SSSSS of a day's seconds, and a displacement past 13:00.
    #[test]
    fn strings_refused_9_51() {
        let refused = [
            ("YYYY-MM-DD", "2024-001-02", DATE),
            ("YYYY-MM-DD", "2024-01 02", DATE),
            ("YYYYMMDD", "2024012", DATE),
            ("YYYY-MM-DD", "2024-01-٠٢", DATE),
            ("YYYY-MM-DD", "2024-13-01", DATE),
            ("YYYY-MM-DD", "2023-02-29", DATE),
            ("YYYY-MM-DD", "0000-01-01", DATE),
            ("HH12 A.M.", "13 P.M.", TIME),
            ("HH24:MI", "24:00", TIME),
            ("HH24:MI:SS", "23:60:00", TIME),
            ("YYYY DDD", "2023 366", DATE),
            ("SSSSS", "86400", TIME),
            ("YYYY TZH:TZM", "2024 +13:01", ZONED),
            ("YYYY TZH:TZM", "2024 +01:60", ZONED),
            ("YYYY-MM-DD", " 2024-01-01", DATE),
        ];
        for (t, s, target) in refused {
            let e = parse(t, s, target).unwrap_err();
            assert_eq!(e.state, Some(SqlState::INVALID_DATETIME_FORMAT), "{t} of '{s}': {e:?}");
        }
        assert!(parse("YYYY TZH:TZM", "2024 +13:00", ZONED).is_ok());
        assert!(parse("YYYY TZH:TZM", "2024 -12:59", ZONED).is_ok());
    }
}
