//! Atomic values [XQueryDM §2.6.4] of the built-in datatypes of XML
//! Schema [Schema2 §3] and of XQuery [XQueryDM §2.6.2] (`atomic/types.rs`),
//! their value spaces (`atomic/value.rs`), and casting among them
//! [XQueryFO §17], as XMLCAST casts them [ISO/IEC 9075-14:2006 draft §6.6
//! GR3d vi, GR4j]. A value of each `AtomicType` is held as the string
//! casting it to `xs:string` gives [XQueryFO §17.1.2], one for each value;
//! a string or an untyped atomic value is cast by the lexical forms of
//! XML Schema Part 2, its white space normalized first as the target
//! type's whiteSpace facet says [XQueryFO §17.1.1] [Schema2 §4.3.6], then
//! checked against the facets that type fixes [XQueryFO §17.2].
//!
//! \[DESIGN\] RULES.md D106: implementation-defined limits [XQueryFO
//! §10.1.1, §17.1.1]: a date's year is from 0001 to 9999, as SQL's are,
//! a year past them failing with err:FODT0001; seconds, and the seconds of
//! a duration, are kept to the microsecond, more digits of their fraction
//! dropped; a duration is at most what 128 bits of months or microseconds
//! hold, else err:FODT0002. An `xs:decimal` or `xs:integer` has any
//! number of digits. A zero time zone is written Z, as its canonical form
//! in [Schema2 §3.2.7.2] is. Every string is a lexical form of
//! `xs:anyURI`, as the extent to which one is checked is
//! implementation-dependent [XQueryFO §17.1.1].

mod binary;
mod duration;
mod exact;
mod gregorian;
mod moment;
mod number;
mod text;
mod types;
mod value;

#[cfg(test)]
mod tests;

pub use binary::{base64, base64_text, hex, hex_text};
pub use duration::{day_time_text, units as duration_value, year_month_text};
pub use exact::Exact;
pub use moment::{LAST_YEAR, Moment, Shape};
pub use number::{double, real_text};
pub use types::{Ordered, Primitive, Production, WhiteSpace};
pub use value::{EqualityKey, canonical, compare, digits, equal, equality_key, length};

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

/// The error of a value that is not of `to`, `why`: err:FORG0001
/// [XQueryFO §17.1.1, §17.4].
fn not_of(why: &str) -> SqlError {
    xquery_error(why, "[XQueryFO §17.1.1, §17.4] err:FORG0001")
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

/// `value` cast as `to` [XQueryFO §17]: itself if of `to`; as a string or
/// untyped, its string; from a string, a type derived from one, or an
/// untyped value, the value of `to` its normalized text is a lexical form
/// of; else, cast as the type a cast treats `to`'s as primitive as the
/// types' casting rules say, then checked against the facets `to` fixes
/// [XQueryFO §17.4, §17.5].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error: of text that is no lexical form of `to`,
/// or of a value its facets refuse, err:FORG0001; of a type not cast to
/// `to`, err:XPTY0004; to `xs:NOTATION`, which is abstract, err:XPST0080;
/// to `xs:QName` from a string, which a cast takes only as a literal,
/// err:XPST0083; of a value too large for the limits above, err:FODT0001
/// or err:FODT0002; of a NaN or infinite number cast to a decimal or an
/// integer, err:FOCA0002.
pub fn cast(value: &Atomic, to: AtomicType) -> Result<Atomic> {
    if value.kind == to {
        return Ok(value.clone());
    }
    if to == AtomicType::String || to == AtomicType::UntypedAtomic {
        return Ok(Atomic::new(to, value.lexical.clone()));
    }
    refuse_names(value, to)?;
    let lexical = if value.kind.primitive().textual() {
        of_text(&value.lexical, to)?.ok_or_else(|| not_of(&format!("'{}' is no lexical form of {}", value.lexical, to.name())))?
    } else {
        let up = between(value, to.primitive())?
            .ok_or_else(|| xquery_error(&format!("{} is not cast as {}", value.kind.name(), to.name()), "[XQueryFO §17.1] err:XPTY0004"))?;
        derived(up, to).ok_or_else(|| not_of(&format!("{} is no value of {}", value.lexical, to.name())))?
    };
    Ok(Atomic::new(to, lexical))
}

/// [XQueryFO §17.1, §17.1.1] The casts to `xs:NOTATION` and `xs:QName`
/// refused: to `xs:NOTATION`, abstract, err:XPST0080; to `xs:QName`, of a
/// string err:XPST0083, as XMLCAST casts no literal, else err:XPTY0004.
fn refuse_names(value: &Atomic, to: AtomicType) -> Result<()> {
    let why = format!("{} is not cast as {}", value.kind.name(), to.name());
    let target = to.primitive();
    if target == Primitive::Notation {
        return Err(xquery_error(&why, "[XQueryFO §17.1] err:XPST0080"));
    }
    if target != Primitive::QName {
        return Ok(());
    }
    let code =
        if value.kind.primitive() == Primitive::String { "[XQueryFO §17.1.1] err:XPST0083" } else { "[XQueryFO §17.1] err:XPTY0004" };
    Err(xquery_error(&why, code))
}

/// `text` normalized as `white_space` says [Schema2 §4.3.6]: of replace,
/// each tab, line feed and carriage return a space; of collapse, then
/// runs of spaces one space, none at either end.
#[must_use]
pub fn normalize(text: &str, white_space: WhiteSpace) -> String {
    match white_space {
        WhiteSpace::Preserve => text.to_owned(),
        WhiteSpace::Replace => text.chars().map(|c| if is_space(c) { ' ' } else { c }).collect(),
        WhiteSpace::Collapse => text.split(is_space).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" "),
    }
}

/// The value of `to` whose lexical form `text` is, once normalized as
/// `to`'s whiteSpace facet says [Schema2 §4.1.4 Datatype Valid] [Schema1
/// §3.1.4, §3.14.4]: a literal read as validation reads it, as a cast from a
/// string reads it [XQueryFO §17.1.1], and as a literal of `xs:QName` or
/// `xs:NOTATION` too; none if it is no lexical form of `to`.
///
/// # Errors
///
/// SQLSTATE 10000 of a date or duration too large to hold, err:FODT0001
/// or err:FODT0002.
pub fn from_lexical(text: &str, to: AtomicType) -> Result<Option<Atomic>> {
    Ok(of_text(text, to)?.map(|lexical| Atomic::new(to, lexical)))
}

/// The string of the value of `to` that `text`, normalized as `to`'s
/// whiteSpace facet says, is a lexical form of; none if it is none
/// [XQueryFO §17.1.1].
fn of_text(text: &str, to: AtomicType) -> Result<Option<String>> {
    let normalized = normalize(text, to.white_space());
    Ok(lexical(&normalized, to.primitive())?.and_then(|found| derived(found, to)))
}

/// The string of the value of the type a cast treats as primitive `p`
/// that `c` is a lexical form of; none if it is none [Schema2 §3.2,
/// §3.3.13.1] [XQueryFO §10.3.1.1, §10.3.2.1].
fn lexical(c: &str, p: Primitive) -> Result<Option<String>> {
    if let Some(shape) = Shape::of(p) {
        return Ok(Moment::parse(c, shape)?.map(|m| m.text()));
    }
    Ok(match p {
        Primitive::Untyped | Primitive::String | Primitive::AnyUri => Some(c.to_owned()),
        Primitive::Boolean => number::boolean(c).map(number::boolean_text),
        Primitive::Decimal => number::decimal(c),
        Primitive::Integer => number::integer(c),
        Primitive::Float => number::float(c).map(|x| real_text(x, true)),
        Primitive::Double => double(c).map(|x| real_text(x, false)),
        Primitive::Duration => duration::general(c)?.map(|(months, micros)| duration::general_text(months, micros)),
        Primitive::YearMonthDuration => duration::year_month(c)?.map(year_month_text),
        Primitive::DayTimeDuration => duration::day_time(c)?.map(day_time_text),
        Primitive::HexBinary => hex(c).map(|octets| hex_text(&octets)),
        Primitive::Base64Binary => base64(c).map(|octets| base64_text(&octets)),
        Primitive::QName | Primitive::Notation => text::is_qname(c).then(|| c.to_owned()),
        Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth => None,
    })
}

/// `found`, the string of a value of the type `to` is treated as, if it
/// is a value of `to` [XQueryFO §17.4]: within the bounds an integer type
/// fixes, and of the production a type derived from `xs:token` matches
/// [Schema2 §3.3, Appendix A].
fn derived(found: String, to: AtomicType) -> Option<String> {
    let (least, most) = to.bounds();
    if least.is_some() || most.is_some() {
        let value = Exact::parse(&found)?;
        let within = |bound: Option<&str>, beyond| bound.and_then(Exact::parse).is_none_or(|b| value.compare(&b) != beyond);
        if !within(least, std::cmp::Ordering::Less) || !within(most, std::cmp::Ordering::Greater) {
            return None;
        }
    }
    text::matches(&found, to.production()).then_some(found)
}

/// `value`, of no textual type, cast as the type `to`, of a primitive
/// type a cast treats as one; none if no value of its type is [XQueryFO
/// §17.1].
fn between(value: &Atomic, to: Primitive) -> Result<Option<String>> {
    if Shape::of(to).is_some() {
        return Ok(Moment::convert(value, to));
    }
    match to {
        Primitive::Boolean | Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double => number::convert(value, to),
        Primitive::Duration | Primitive::YearMonthDuration | Primitive::DayTimeDuration => Ok(duration::convert(value, to)),
        Primitive::HexBinary | Primitive::Base64Binary => Ok(binary_convert(value, to)),
        // [XQueryFO §17.1.2] every value is cast to a string as its string.
        Primitive::String => Ok(Some(value.lexical.to_string())),
        Primitive::Untyped
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth => Ok(None),
    }
}

/// [XQueryFO §17.1.7] `value`, hex or base64, cast as the other: the same
/// octets; none of another type.
fn binary_convert(value: &Atomic, to: Primitive) -> Option<String> {
    let octets = match value.kind.primitive() {
        Primitive::HexBinary => hex(&value.lexical)?,
        Primitive::Base64Binary => base64(&value.lexical)?,
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
        | Primitive::DateTime
        | Primitive::Time
        | Primitive::Date
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => return None,
    };
    Some(if to == Primitive::HexBinary { hex_text(&octets) } else { base64_text(&octets) })
}
