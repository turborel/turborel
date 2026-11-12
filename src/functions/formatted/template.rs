//! [ISO/IEC 9075-2:2023 §9.52] A `<datetime template>`: its parts read,
//! held to the Syntax Rules for the datetime type it formats or reads,
//! and the lengths and precision those rules define.

use std::mem::discriminant;

use crate::error::{Code, Result, SqlError};
use crate::value::datetime::Moment;

/// A `<datetime template field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    /// YYYY, YYY, YY or Y: so many of the year's last digits.
    Year(usize),
    /// RRRR or RR.
    RoundedYear(usize),
    /// MM.
    Month,
    /// DD.
    DayOfMonth,
    /// DDD.
    DayOfYear,
    /// HH or HH12.
    Hour12,
    /// HH24.
    Hour24,
    /// MI.
    Minute,
    /// SS.
    Second,
    /// SSSSS.
    SecondOfDay,
    /// FF1 to FF9: so many digits of the second's fraction.
    Fraction(usize),
    /// A.M. or P.M.
    AmPm,
    /// TZH.
    ZoneHour,
    /// TZM.
    ZoneMinute,
}

/// What a field is of: the date, the time of day, or the time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Of {
    Date,
    Time,
    Zone,
}

impl Field {
    fn of(self) -> Of {
        match self {
            Field::Year(_) | Field::RoundedYear(_) | Field::Month | Field::DayOfMonth | Field::DayOfYear => Of::Date,
            Field::Hour12 | Field::Hour24 | Field::Minute | Field::Second | Field::SecondOfDay | Field::Fraction(_) | Field::AmPm => {
                Of::Time
            }
            Field::ZoneHour | Field::ZoneMinute => Of::Zone,
        }
    }
}

/// A `<datetime template part>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    /// A field.
    Field(Field),
    /// A `<datetime template delimiter>`.
    Delimiter(char),
}

/// Each field as written.
const WORDS: [(&str, Field); 28] = [
    ("YYYY", Field::Year(4)),
    ("YYY", Field::Year(3)),
    ("YY", Field::Year(2)),
    ("Y", Field::Year(1)),
    ("RRRR", Field::RoundedYear(4)),
    ("RR", Field::RoundedYear(2)),
    ("MM", Field::Month),
    ("DDD", Field::DayOfYear),
    ("DD", Field::DayOfMonth),
    ("HH24", Field::Hour24),
    ("HH12", Field::Hour12),
    ("HH", Field::Hour12),
    ("MI", Field::Minute),
    ("SSSSS", Field::SecondOfDay),
    ("SS", Field::Second),
    ("FF1", Field::Fraction(1)),
    ("FF2", Field::Fraction(2)),
    ("FF3", Field::Fraction(3)),
    ("FF4", Field::Fraction(4)),
    ("FF5", Field::Fraction(5)),
    ("FF6", Field::Fraction(6)),
    ("FF7", Field::Fraction(7)),
    ("FF8", Field::Fraction(8)),
    ("FF9", Field::Fraction(9)),
    ("A.M.", Field::AmPm),
    ("P.M.", Field::AmPm),
    ("TZH", Field::ZoneHour),
    ("TZM", Field::ZoneMinute),
];

/// The `<datetime template delimiter>`s.
const DELIMITERS: [char; 8] = ['-', '.', '/', ',', '\'', ';', ':', ' '];

/// The most digits of a time zone hour, its sign aside [§9.51 SR4b i].
pub(super) const ZONE_HOUR_DIGITS: usize = 2;

/// The error of a template the Syntax Rule `rule` refuses, `why`.
fn refused(rule: &str, why: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §9.52 {rule}] the datetime template {why}"))
}

/// A `<datetime template>`, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Template {
    parts: Vec<Part>,
}

impl Template {
    /// `text` read as a template of a datetime of `moment`, WITH TIME ZONE
    /// if `zoned`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it breaks a Syntax Rule of §9.52.
    pub(super) fn read(text: &str, moment: Moment, zoned: bool) -> Result<Template> {
        let template = Template { parts: lexed(text)? };
        template.combined()?;
        template.of_type(moment, zoned)?;
        Ok(template)
    }

    /// Its parts, in order.
    pub(super) fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Whether a field of it is one `pick` picks.
    pub(super) fn has(&self, pick: impl Fn(Field) -> bool) -> bool {
        self.fields().any(pick)
    }

    fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        self.parts.iter().filter_map(|p| if let Part::Field(f) = p { Some(*f) } else { None })
    }

    /// \[SR3\]-\[SR11\] No delimiter after another, no field twice, and
    /// the fields together as they may be.
    fn combined(&self) -> Result<()> {
        if self.parts.windows(2).any(|w| matches!(w, [Part::Delimiter(_), Part::Delimiter(_)])) {
            return Err(refused("SR3", "has two delimiters in a row"));
        }
        let fields: Vec<Field> = self.fields().collect();
        for (i, f) in fields.iter().enumerate() {
            if fields.iter().skip(i.saturating_add(1)).any(|g| discriminant(g) == discriminant(f)) {
                return Err(refused("SR4", "has a field twice"));
            }
        }
        let has = |pick: fn(&Field) -> bool| fields.iter().any(pick);
        let year = has(|f| matches!(f, Field::Year(_)));
        let rounded = has(|f| matches!(f, Field::RoundedYear(_)));
        let (month_or_day, day_of_year) = (has(|f| matches!(f, Field::Month | Field::DayOfMonth)), has(|f| *f == Field::DayOfYear));
        let (hour12, hour24, am_pm) = (has(|f| *f == Field::Hour12), has(|f| *f == Field::Hour24), has(|f| *f == Field::AmPm));
        let clock = has(|f| matches!(f, Field::Hour12 | Field::Hour24 | Field::Minute | Field::Second | Field::AmPm));
        let checks = [
            (year && rounded, "SR5", "has both a year and a rounded year"),
            (day_of_year && month_or_day, "SR6", "has a day of the year beside a month or a day of the month"),
            (hour24 && (hour12 || am_pm), "SR7", "has HH24 beside a 12-hour field or A.M. or P.M."),
            (hour12 != am_pm, "SR8, SR9", "has a 12-hour field without A.M. or P.M., or one without the other"),
            (has(|f| *f == Field::SecondOfDay) && clock, "SR10", "has SSSSS beside an hour, a minute or a second"),
            (has(|f| *f == Field::ZoneMinute) && !has(|f| *f == Field::ZoneHour), "SR11", "has TZM without TZH"),
        ];
        match checks.into_iter().find(|(broken, ..)| *broken) {
            Some((_, rule, why)) => Err(refused(rule, why)),
            None => Ok(()),
        }
    }

    /// \[SR12\] Only fields the datetime type has.
    fn of_type(&self, moment: Moment, zoned: bool) -> Result<()> {
        let refuses: &[Of] = match (moment, zoned) {
            (Moment::Date, _) => &[Of::Time, Of::Zone],
            (Moment::Time, false) => &[Of::Date, Of::Zone],
            (Moment::Time, true) => &[Of::Date],
            (Moment::Timestamp, false) => &[Of::Zone],
            (Moment::Timestamp, true) => &[],
        };
        if self.has(|f| refuses.contains(&f.of())) {
            let kind = if zoned { format!("{moment:?} WITH TIME ZONE") } else { format!("{moment:?}") };
            return Err(refused("SR12", &format!("has a field {} has not", kind.to_uppercase())));
        }
        Ok(())
    }

    /// \[SR13\]-\[SR15\] Whether part `i` is delimited: first, or A.M. or
    /// P.M. or TZH, or after a delimiter or A.M. or P.M.; and the last
    /// field, or A.M. or P.M., or before a delimiter, A.M. or P.M., or
    /// TZH.
    pub(super) fn delimited(&self, i: usize) -> bool {
        let part = self.parts.get(i);
        let am_pm = matches!(part, Some(Part::Field(Field::AmPm)));
        let before = i.checked_sub(1).and_then(|b| self.parts.get(b));
        let left = before.is_none()
            || am_pm
            || matches!(part, Some(Part::Field(Field::ZoneHour)))
            || matches!(before, Some(Part::Delimiter(_) | Part::Field(Field::AmPm)));
        let last_field = !self.parts.iter().skip(i.saturating_add(1)).any(|p| matches!(p, Part::Field(_)));
        let after = self.parts.get(i.saturating_add(1));
        let right = last_field || am_pm || matches!(after, Some(Part::Delimiter(_) | Part::Field(Field::AmPm | Field::ZoneHour)));
        left && right
    }

    /// \[SR16\] The maximum field length of `part`.
    pub(super) fn max_length(part: Part) -> usize {
        match part {
            Part::Delimiter(_) => 1,
            Part::Field(f) => match f {
                Field::Hour12 | Field::Hour24 | Field::ZoneMinute | Field::Month | Field::DayOfMonth | Field::Minute | Field::Second => 2,
                Field::Year(n) | Field::RoundedYear(n) | Field::Fraction(n) => n,
                Field::DayOfYear | Field::ZoneHour => 3,
                Field::AmPm => 4,
                Field::SecondOfDay => 5,
            },
        }
    }

    /// \[SR18\], \[SR19\] The minimum template length: of each part
    /// delimited 1, of TZH 2, else its maximum field length.
    pub(super) fn min_length(&self) -> usize {
        let least = |(i, part): (usize, &Part)| match part {
            Part::Field(f) => {
                if !self.delimited(i) {
                    Template::max_length(*part)
                } else if *f == Field::ZoneHour {
                    ZONE_HOUR_DIGITS
                } else {
                    1
                }
            }
            Part::Delimiter(_) => 1,
        };
        self.parts.iter().enumerate().map(least).sum()
    }

    /// \[SR20\] Its fractional precision: of FFn n, else 0.
    pub(super) fn precision(&self) -> usize {
        self.fields().find_map(|f| if let Field::Fraction(n) = f { Some(n) } else { None }).unwrap_or(0)
    }
}

/// \[SR2\] `text`'s parts, each field the longest written at its place.
fn lexed(text: &str) -> Result<Vec<Part>> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let word = WORDS.iter().filter(|(w, _)| rest.starts_with(w)).max_by_key(|(w, _)| w.len());
        if let Some((w, field)) = word {
            parts.push(Part::Field(*field));
            rest = rest.get(w.len()..).unwrap_or_default();
        } else if DELIMITERS.contains(&c) {
            parts.push(Part::Delimiter(c));
            rest = rest.get(c.len_utf8()..).unwrap_or_default();
        } else {
            return Err(refused("SR2", &format!("'{text}' has '{c}', which begins no field and is no delimiter")));
        }
    }
    if parts.is_empty() {
        return Err(refused("SR2", "is empty"));
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<Template> {
        Template::read(text, Moment::Timestamp, true)
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.52 SR2, SR16-SR20] a
    /// template is read field by field, each the longest written; a field
    /// delimited on both sides may be shorter than its maximum, TZH of two
    /// characters, so the minimum template length counts it so.
    #[test]
    fn templates_are_read_9_52() {
        let t = read("YYYY-MM-DD HH24:MI:SS.FF3 TZH:TZM").unwrap();
        assert_eq!(t.parts().len(), 17);
        assert_eq!(t.min_length(), 18, "each field 1 but TZH 2, and each delimiter 1");
        assert_eq!(t.precision(), 3);
        let packed = read("YYYYMMDD").unwrap();
        assert_eq!(packed.min_length(), 8);
        assert!(!packed.delimited(0) && !packed.delimited(1) && !packed.delimited(2));
        let clock = read("HH12A.M.").unwrap();
        assert_eq!(clock.parts(), [Part::Field(Field::Hour12), Part::Field(Field::AmPm)]);
        assert!(clock.delimited(0));
        assert_eq!(read("SSSSS").unwrap().parts(), [Part::Field(Field::SecondOfDay)]);
        assert!(read("yyyy").is_err() && read("").is_err() && read("YYYY_MM").is_err());
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.52 SR3-SR12] the
    /// template's fields go together as the rules say, and are fields of
    /// the datetime type: each refusal SQLCODE -103.
    #[test]
    fn templates_refused_9_52() {
        let refused = [
            "YYYY--MM",
            "YYYYMMYY",
            "YYYY RR",
            "DDD MM",
            "HH24 HH12 A.M.",
            "HH12",
            "A.M.",
            "SSSSS MI",
            "TZM",
            "HH24 A.M.",
            "YYYYY",
            "SSSS",
        ];
        for t in refused {
            assert_eq!(read(t).unwrap_err().code, Code::SyntaxRule, "{t}");
        }
        assert!(Template::read("YYYY-MM-DD HH24", Moment::Date, false).is_err());
        assert!(Template::read("HH24:MI TZH", Moment::Time, false).is_err());
        assert!(Template::read("HH24:MI TZH", Moment::Time, true).is_ok());
        assert!(Template::read("YYYY HH24", Moment::Time, true).is_err());
        assert!(Template::read("YYYY TZH", Moment::Timestamp, false).is_err());
        assert!(Template::read("YYYY-MM-DD", Moment::Date, false).is_ok());
    }
}
