//! Casting atomic values [XQueryFO §17.1], as XMLCAST casts them
//! [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j]. A value of each
//! `AtomicType` is held as its canonical lexical form, the string casting
//! it to `xs:string` gives [XQueryFO §17.1.2]; a string or an untyped
//! atomic value is cast by the lexical forms of XML Schema Part 2,
//! whitespace collapsed first [XQueryFO §17.1.1] [Schema2 §4.3.6].
//!
//! \[DESIGN\] RULES.md D86: implementation-defined limits [XQueryFO
//! §10.1.1, §17.1.1]: a date's year is from 0001 to 9999, as SQL's are,
//! a year past them failing with err:FODT0001; seconds, and the seconds of
//! a `xdt:dayTimeDuration`, are kept to the microsecond, more digits of
//! their fraction dropped; a duration is at most what 128 bits of months
//! or microseconds hold, else err:FODT0002. An `xs:decimal` or
//! `xs:integer` has any number of digits. A zero time zone is written Z,
//! as its canonical form in [Schema2 §3.2.7.2] is.

mod duration;
mod moment;
mod number;

#[cfg(test)]
mod tests;

pub use moment::{LAST_YEAR, Moment, Shape};

use std::fmt::Write;

use super::chars::is_space;
use super::sequence::{Atomic, AtomicType};
use crate::error::{Code, Result, SqlError, SqlState};

/// The error of an XQuery evaluation of XMLCAST, `why`, by the rule and
/// error code `code` [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j]:
/// SQLSTATE 10000, XQuery error.
pub fn xquery_error(why: &str, code: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j] XQuery error: {why} {code}"))
        .with_state(SqlState::XQUERY_ERROR)
}

impl AtomicType {
    /// Its name, a QName of the namespaces `xs` and `xdt` [XQueryDM §2.6].
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            AtomicType::String => "xs:string",
            AtomicType::UntypedAtomic => "xdt:untypedAtomic",
            AtomicType::Boolean => "xs:boolean",
            AtomicType::Decimal => "xs:decimal",
            AtomicType::Integer => "xs:integer",
            AtomicType::Float => "xs:float",
            AtomicType::Double => "xs:double",
            AtomicType::Date => "xs:date",
            AtomicType::Time => "xs:time",
            AtomicType::DateTime => "xs:dateTime",
            AtomicType::YearMonthDuration => "xdt:yearMonthDuration",
            AtomicType::DayTimeDuration => "xdt:dayTimeDuration",
            AtomicType::HexBinary => "xs:hexBinary",
        }
    }

    /// Whether it is `xs:string` or `xdt:untypedAtomic`, which every value
    /// casts to and is cast from by its lexical form [XQueryFO §17.1.1,
    /// §17.1.2].
    fn textual(self) -> bool {
        matches!(self, AtomicType::String | AtomicType::UntypedAtomic)
    }
}

/// The units of the canonical duration `text`: months if `year_month`,
/// else microseconds [XQueryFO §10.3].
///
/// # Errors
///
/// SQLSTATE 10000 if it is no lexical form of the duration, err:FORG0001,
/// or too large to hold, err:FODT0002.
pub fn duration_units(text: &str, year_month: bool) -> Result<i128> {
    let units = if year_month { duration::year_month(text)? } else { duration::day_time(text)? };
    units.ok_or_else(|| xquery_error(&format!("'{text}' is no duration"), "[XQueryFO §17.1.1] err:FORG0001"))
}

/// `value` cast as `to` [XQueryFO §17.1]: itself if of `to`; as text, its
/// lexical form; from text, the value its whitespace-collapsed text is a
/// lexical form of; else as the types' casting rules say.
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error: of text that is no lexical form of `to`,
/// err:FORG0001; of a type not cast to `to`, err:XPTY0004; of a value too
/// large for the limits above, err:FODT0001 or err:FODT0002; of a NaN or
/// infinite number cast to a decimal or an integer, err:FOCA0002.
pub fn cast(value: &Atomic, to: AtomicType) -> Result<Atomic> {
    if value.kind == to {
        return Ok(value.clone());
    }
    if to.textual() {
        return Ok(Atomic { kind: to, lexical: value.lexical.clone() });
    }
    let lexical = if value.kind.textual() {
        of_text(&value.lexical, to)?.ok_or_else(|| {
            xquery_error(&format!("'{}' is no lexical form of {}", value.lexical, to.name()), "[XQueryFO §17.1.1] err:FORG0001")
        })?
    } else {
        between(value, to)?
            .ok_or_else(|| xquery_error(&format!("{} is not cast as {}", value.kind.name(), to.name()), "[XQueryFO §17.1] err:XPTY0004"))?
    };
    Ok(Atomic { kind: to, lexical: lexical.into() })
}

/// `text` collapsed [Schema2 §4.3.6]: runs of white space one space, none
/// at either end.
fn collapse(text: &str) -> String {
    text.split(is_space).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The canonical lexical form of the value of `to` that `text`, collapsed,
/// is a lexical form of; none if it is none [XQueryFO §17.1.1].
fn of_text(text: &str, to: AtomicType) -> Result<Option<String>> {
    let collapsed = collapse(text);
    let c = collapsed.as_str();
    Ok(match to {
        AtomicType::String | AtomicType::UntypedAtomic => Some(text.to_owned()),
        AtomicType::Boolean => number::boolean(c).map(number::boolean_text),
        AtomicType::Decimal => number::decimal(c),
        AtomicType::Integer => number::integer(c),
        AtomicType::Float => number::float(c).map(|x| number::real_text(x, true)),
        AtomicType::Double => number::double(c).map(|x| number::real_text(x, false)),
        AtomicType::Date => Moment::parse(c, Shape::Date)?.map(|m| m.text()),
        AtomicType::Time => Moment::parse(c, Shape::Time)?.map(|m| m.text()),
        AtomicType::DateTime => Moment::parse(c, Shape::DateTime)?.map(|m| m.text()),
        AtomicType::YearMonthDuration => duration::year_month(c)?.map(duration::year_month_text),
        AtomicType::DayTimeDuration => duration::day_time(c)?.map(duration::day_time_text),
        AtomicType::HexBinary => hex(c).map(|octets| hex_text(&octets)),
    })
}

/// `value`, of no textual type, cast as `to`, of none and not its own;
/// none if no value of its type is [XQueryFO §17.1].
fn between(value: &Atomic, to: AtomicType) -> Result<Option<String>> {
    match to {
        AtomicType::Boolean | AtomicType::Decimal | AtomicType::Integer | AtomicType::Float | AtomicType::Double => {
            number::convert(value, to)
        }
        AtomicType::Date | AtomicType::Time | AtomicType::DateTime => Ok(Moment::convert(value, to)),
        AtomicType::YearMonthDuration | AtomicType::DayTimeDuration => Ok(duration::convert(value, to)),
        AtomicType::String | AtomicType::UntypedAtomic | AtomicType::HexBinary => Ok(None),
    }
}

/// The octets `text` encodes, if it is a lexical form of `xs:hexBinary`:
/// two hexadecimal digits each [Schema2 §3.2.15].
#[must_use]
pub fn hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair).ok().filter(|p| p.bytes().all(|b| b.is_ascii_hexdigit())).and_then(|p| u8::from_str_radix(p, 16).ok())
        })
        .collect()
}

/// `octets` as the canonical lexical form of `xs:hexBinary`: two upper case
/// hexadecimal digits each [Schema2 §3.2.15.2].
#[must_use]
pub fn hex_text(octets: &[u8]) -> String {
    octets.iter().fold(String::with_capacity(octets.len().saturating_mul(2)), |mut out, o| {
        let _ = write!(out, "{o:02X}");
        out
    })
}
