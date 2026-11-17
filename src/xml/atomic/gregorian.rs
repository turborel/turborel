//! The Gregorian types [Schema2 §3.2.10 to §3.2.14]: `xs:gYearMonth`,
//! `xs:gYear`, `xs:gMonthDay`, `xs:gDay` and `xs:gMonth`, each a period
//! held as its starting instant, the date its fields and a template give
//! the fields it has not [XQueryFO §10.4.17 to §10.4.21].
//!
//! \[DESIGN\] RULES.md D86: the template year of a recurring value is
//! 1972, a leap year, so --02-29 is a value, and its template month
//! December, of 31 days, so ---31 is; XQueryFO's templates for equality
//! name them.

use super::moment::{Moment, Shape};
use crate::value::datetime::civil_from_days;

/// [XQueryFO §10.4.19 to §10.4.21] The year of the template of the
/// recurring types.
const TEMPLATE_YEAR: &str = "1972";
/// [XQueryFO §10.4.21] The month of the template of `xs:gDay`.
const TEMPLATE_MONTH: &str = "12";
/// The first day of a month, and the first month of a year.
const FIRST: &str = "01";
/// The digits of a month's or a day's field.
const FIELD: usize = 2;

/// The date, yyyy-mm-dd, of the starting instant of `body`, a literal of
/// the Gregorian type of `shape` without its time zone [Schema2
/// §3.2.10.1 to §3.2.14.1]: CCYY-MM, CCYY, --MM-DD, ---DD or --MM--; none
/// if it is of none of their forms. The date is read as a date's is, so
/// its year, month and day are checked as a date's are.
pub(super) fn as_date(body: &str, shape: Shape) -> Option<String> {
    let two = |s: &str| (s.len() == FIELD && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.to_owned());
    Some(match shape {
        Shape::GYearMonth => {
            let (year, month) = body.rsplit_once('-')?;
            format!("{year}-{}-{FIRST}", two(month)?)
        }
        Shape::GYear => format!("{body}-{FIRST}-{FIRST}"),
        Shape::GMonthDay => {
            let (month, day) = body.strip_prefix("--")?.split_once('-')?;
            format!("{TEMPLATE_YEAR}-{}-{}", two(month)?, two(day)?)
        }
        Shape::GDay => format!("{TEMPLATE_YEAR}-{TEMPLATE_MONTH}-{}", two(body.strip_prefix("---")?)?),
        Shape::GMonth => format!("{TEMPLATE_YEAR}-{}-{FIRST}", two(body.strip_prefix("--")?.strip_suffix("--")?)?),
        Shape::Date | Shape::Time | Shape::DateTime => return None,
    })
}

impl Moment {
    /// The fields of its Gregorian type, as its literals write them
    /// [Schema2 §3.2.10.1 to §3.2.14.1], without its time zone.
    pub(super) fn gregorian_text(&self) -> String {
        let (year, month, day) = civil_from_days(self.days);
        match self.shape {
            Shape::GYearMonth => format!("{year:04}-{month:02}"),
            Shape::GYear => format!("{year:04}"),
            Shape::GMonthDay => format!("--{month:02}-{day:02}"),
            Shape::GDay => format!("---{day:02}"),
            Shape::GMonth => format!("--{month:02}--"),
            Shape::Date | Shape::Time | Shape::DateTime => String::new(),
        }
    }

    /// The value of the Gregorian type of `shape` of its date's fields and
    /// its time zone [XQueryFO §17.1.5].
    pub(super) fn to_gregorian(self, shape: Shape) -> Moment {
        let (year, month, day) = civil_from_days(self.days);
        let date = match shape {
            Shape::GYearMonth => format!("{year:04}-{month:02}-{FIRST}"),
            Shape::GYear => format!("{year:04}-{FIRST}-{FIRST}"),
            Shape::GMonthDay => format!("{TEMPLATE_YEAR}-{month:02}-{day:02}"),
            Shape::GDay => format!("{TEMPLATE_YEAR}-{TEMPLATE_MONTH}-{day:02}"),
            Shape::GMonth => format!("{TEMPLATE_YEAR}-{month:02}-{FIRST}"),
            Shape::Date | Shape::Time | Shape::DateTime => return self,
        };
        let days = Moment::parse(&date, Shape::Date).ok().flatten().map_or(self.days, |m| m.days);
        Moment { shape, days, micros: 0, zone: self.zone }
    }
}
