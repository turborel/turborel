//! The mapping of SQL data types to XML Schema data types [ISO/IEC
//! 9075-14:2006 draft §9.5]: the type, restricted by facets, that XMLCAST
//! casts an XML value as before it is an SQL value [§6.6 GR4e, GR4j].
//!
//! \[DESIGN\] RULES.md D106: the implementation-defined choices of §9.5
//! are those `functions/xml/cast.rs` states; a type's annotations
//! [§9.5 GR8a vii and the like] are not made, nor written in XMLMAP's
//! XML Schema documents. Two choices are new. \[GR8a ii\] The mapping of a character
//! set to Unicode, CSM, is the identity, as turborel's strings are
//! Unicode, so it is homomorphic and MAXCSL is N: CHARACTER(n) has the
//! length n; of a length in OCTETS a character string has the maxLength n,
//! as its characters are fewer than its octets. And the pattern of a time
//! zone [GR8i ii, GR8k iii], printed `(+|-)`, is no regular expression,
//! `+` being a metacharacter [Schema2 Appendix F \[10\]], so it is read as
//! `(\+|-)`, a sign, which it names.
//!
//! The facets are derived once, by `xml_schema_facets`, for XMLCAST's
//! type and for the text of the definitions an XML Schema document of
//! XMLMAP holds (`functions/xml/map/types.rs`).
//!
//! Two readings make an interval's pattern \[GR8l\] describe what §9.8
//! GR9h writes. Of a fractional seconds precision of 0, the seconds are
//! written with no fraction \[GR9h vi, vii\], so SECS is `\p{Nd}{2}S`, as
//! of a type of no seconds \[GR8l ii 2\], not `\p{Nd}{2}.\p{Nd}{0}S`. Of
//! INTERVAL SECOND, the seconds are the leading field, of L digits \[GR9h
//! vii 13\], so they are `\p{Nd}{PLIT}`, as each other leading field is,
//! not `\p{Nd}{2}`.

use std::fmt::Write;
use std::sync::Arc;

use super::mapping::BinaryEncoding;
use crate::error::Result;
use crate::value::datetime::Field;
use crate::value::interval::Qualifier;
use crate::value::lob::LobKind;
use crate::value::{CharUnits, DataType, Spelled};
use crate::xml::{AtomicType, Facet, FacetSpec, SimpleType};

/// [§9.5 GR8g ii] The pattern of a date: `\p{Nd}{4}-\p{Nd}{2}-\p{Nd}{2}`.
const DATE: &str = "\\p{Nd}{4}-\\p{Nd}{2}-\\p{Nd}{2}";
/// [§9.5 GR8h ii, GR8i iii] The pattern of a time of day.
const TIME: &str = "\\p{Nd}{2}:\\p{Nd}{2}:\\p{Nd}{2}";
/// [§9.5 GR8i ii, GR8k iii] TZ, the pattern of a time zone, its sign
/// escaped as above.
const TZ: &str = "(\\+|-)\\p{Nd}{2}:\\p{Nd}{2}";
/// [§9.5 GR8l iii] The optional sign and P that begin an interval's
/// pattern.
const SIGNED: &str = "-?P";
/// [§9.5 GR8l iii] The digits of each field of an interval's pattern but
/// its leading one.
const FIELD_DIGITS: u8 = 2;
/// [§9.5 GR8d i, ii] The digit each place of the greatest value of an
/// exact numeric type of a precision is.
const NINE: char = '9';

/// [§9.5 GR8] The XML Schema type the SQL type `t` maps to, restricted by
/// the facets §9.5 gives it: a character string's `xs:string` of its
/// length or maxLength \[GR8a\]; a binary string's `xs:hexBinary`, or of
/// BASE64 `encoding` `xs:base64Binary`, of its maxLength \[GR8b\]; NUMERIC's and
/// DECIMAL's `xs:decimal` of its totalDigits and fractionDigits \[GR8c\];
/// INTEGER's, SMALLINT's and BIGINT's `xs:integer` of its bounds \[GR8d\];
/// an approximate number's `xs:float` or `xs:double` \[GR8e\]; BOOLEAN's
/// `xs:boolean` \[GR8f\]; a datetime's `xs:date`, `xs:time` or
/// `xs:dateTime` of its pattern [GR8g to GR8k]; an interval's duration of
/// its pattern \[GR8l\]; none of another type.
///
/// # Errors
///
/// None of the types it maps, whose facets are valid; as `restrict` does
/// otherwise.
pub fn xml_schema_restriction(t: &DataType, encoding: BinaryEncoding) -> Result<Option<SimpleType>> {
    let Some((base, facets)) = xml_schema_facets(t, encoding) else { return Ok(None) };
    let specified: Vec<FacetSpec> = facets.into_iter().map(FacetSpec::new).collect();
    Ok(Some(SimpleType::restrict(&Arc::new(SimpleType::built_in(base)), &specified)?.named(&t.to_string())))
}

/// [§9.5 GR8a to GR8l] The base and the facets of the restriction §9.5
/// maps `t` to, as `xml_schema_restriction` says, of binary strings in
/// `encoding` \[GR8b\]. None of a type §9.5 maps to no restriction of a
/// built-in type.
#[must_use]
pub fn xml_schema_facets(t: &DataType, encoding: BinaryEncoding) -> Option<(AtomicType, Vec<Facet>)> {
    Some(match t.represented() {
        DataType::Character { length, units: CharUnits::Characters } => (AtomicType::String, vec![Facet::Length(u64::from(*length))]),
        DataType::Character { length, units: CharUnits::Octets } | DataType::Varying { length, .. } => {
            (AtomicType::String, vec![Facet::MaxLength(u64::from(*length))])
        }
        DataType::Lob { kind: LobKind::Character, length, .. } => (AtomicType::String, vec![Facet::MaxLength(*length)]),
        DataType::Binary { length } | DataType::VarBinary { length } => {
            (super::cast::binary_type(encoding), vec![Facet::MaxLength(u64::from(*length))])
        }
        DataType::Lob { kind: LobKind::Binary, length, .. } => (super::cast::binary_type(encoding), vec![Facet::MaxLength(*length)]),
        DataType::Exact { precision, spelled: Spelled::Integer | Spelled::Smallint | Spelled::Bigint, .. } => {
            let most: String = std::iter::repeat_n(NINE, usize::try_from(*precision).unwrap_or(0)).collect();
            (AtomicType::Integer, vec![Facet::MaxInclusive(most.as_str().into()), Facet::MinInclusive(format!("-{most}").into())])
        }
        DataType::Exact { precision, scale, .. } => {
            (AtomicType::Decimal, vec![Facet::TotalDigits(u64::from(*precision)), Facet::FractionDigits(u64::from(*scale))])
        }
        DataType::Approximate { .. } | DataType::Boolean => (super::xml_schema_type(t, encoding)?, Vec::new()),
        DataType::Date => (AtomicType::Date, vec![Facet::Pattern(DATE.into())]),
        DataType::Time { precision, zoned } => (AtomicType::Time, vec![Facet::Pattern(clock(TIME, *precision, *zoned).into())]),
        DataType::Timestamp { precision, zoned } => {
            (AtomicType::DateTime, vec![Facet::Pattern(clock(&format!("{DATE}T{TIME}"), *precision, *zoned).into())])
        }
        DataType::Interval(q) => (super::xml_schema_type(t, encoding)?, vec![Facet::Pattern(interval(*q).into())]),
        DataType::DecFloat { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => return None,
    })
}

/// [§9.5 GR8h to GR8k] The pattern of a time or timestamp whose time of
/// day ends `clock`: then `.` and S digits if S, its fractional seconds
/// precision, is more than 0, and TZ if `zoned`.
fn clock(clock: &str, precision: u32, zoned: bool) -> String {
    let fraction = if precision > 0 { format!(".\\p{{Nd}}{{{precision}}}") } else { String::new() };
    let zone = if zoned { TZ } else { "" };
    format!("{clock}{fraction}{zone}")
}

/// [§9.5 GR8l ii, iii] The pattern of an interval of the qualifier `q`,
/// read as the module says: `-?P`, then each field of `q` with its
/// designator, T before the first of hours, minutes and seconds; the
/// leading field of PLIT digits, its leading precision, the others of two;
/// the seconds with a fraction of SLIT digits after `.` if the type has
/// one.
fn interval(q: Qualifier) -> String {
    let mut out = String::from(SIGNED);
    for field in q.fields() {
        let timed = matches!(field, Field::Hour | Field::Minute | Field::Second);
        if timed && !out.contains('T') {
            out.push('T');
        }
        let digits = if field == q.start { q.leading } else { FIELD_DIGITS };
        // Writing to a String cannot fail.
        let _ = write!(out, "\\p{{Nd}}{{{digits}}}");
        if field == Field::Second && q.fraction > 0 {
            let _ = write!(out, ".\\p{{Nd}}{{{}}}", q.fraction);
        }
        out.push(designator(field));
    }
    out
}

/// The designator of `field` in a duration [Schema2 §3.2.6.1].
fn designator(field: Field) -> char {
    match field {
        Field::Year => 'Y',
        Field::Month | Field::Minute => 'M',
        Field::Day => 'D',
        Field::Hour => 'H',
        Field::Second => 'S',
    }
}

#[cfg(test)]
mod tests;
