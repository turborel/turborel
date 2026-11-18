//! The mapping of SQL values to XML [ISO/IEC 9075-14:2006 draft §9.8]:
//! the lexical form of an SQL value of a predefined type, as XMLCAST
//! [§6.6 GR3a] and the publishing functions write it (`xml_lexical`), and
//! the XML text of a value of a distinct, collection, row or XML type
//! (`xml_mapped`).

use std::fmt::Write;

use super::{Input, input};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::datetime::Now;
use crate::value::datetime::{Field, MICROS_PER_SECOND};
use crate::value::interval::{Interval, Qualifier};
use crate::value::types::datetime::shown;
use crate::value::types::{Field as RowField, MAX_TYPE_NESTING};
use crate::value::{CharUnits, DataType, MAX_CHARACTER_LENGTH, Value};
use crate::xml::{Escape, Version, XSI_NAMESPACE, XSI_PREFIX, base64_text, hex_text, is_char, name_of};

/// [§9.5 GR3, §9.8 GR3] How binary strings are encoded: in base64 or in
/// hex [Schema2 §3.2.16, §3.2.15].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryEncoding {
    /// BASE64: `xs:base64Binary`.
    Base64,
    /// HEX: `xs:hexBinary`.
    Hex,
}

impl BinaryEncoding {
    /// `octets` in this encoding [§9.8 GR9b]: hex, upper case [Schema2
    /// §3.2.15.2]; or base64, of no line breaks, padded [Schema2 §3.2.16].
    #[must_use]
    pub fn encode(self, octets: &[u8]) -> String {
        match self {
            BinaryEncoding::Hex => hex_text(octets),
            BinaryEncoding::Base64 => base64_text(octets),
        }
    }
}

/// [§9.8 GR9] The lexical form of `v`, not null, of an SQL type, as XML
/// text: a character string's characters, `&`, `<`, `>` and a carriage
/// return as references if `charmapping` \[GR4, GR9a iii\]; a binary
/// string in hex or base64 as `encoding` says \[GR3, GR9b\]; a number as CAST to a character string writes it \[GR7,
/// GR9c\]; a truth value in lower case \[GR9d\]; a date, time or timestamp
/// as its literal's string, T between date and time \[GR9e to GR9g\]; an
/// interval as a duration \[GR9h\].
///
/// # Errors
///
/// SQLSTATE 0N002 of a character no XML character [GR9a ii]; the errors
/// of reading a large object.
pub fn xml_lexical(v: &Value, encoding: BinaryEncoding, charmapping: bool) -> Result<String> {
    match v {
        Value::Char(s) => characters(s, charmapping),
        Value::Binary(b) => Ok(encoding.encode(b.bytes())),
        Value::Lob(_) => match input(v)? {
            Some(Input::Text(s)) => characters(&s, charmapping),
            Some(Input::Octets(o)) => Ok(encoding.encode(&o)),
            None => Ok(String::new()),
        },
        Value::Exact(_) | Value::Approx(_) => as_string(v),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Datetime(d) => Ok(d.to_string().replacen(' ', "T", 1)),
        Value::Interval(i) => Ok(duration_text(i)),
        Value::Null
        | Value::DecFloat(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => Err(SqlError::new(Code::Api, format!("[DESIGN] the value {v} is of no type §9.8 maps to XML"))),
    }
}

/// [§9.8 GR7] `v` CAST to CHARACTER VARYING of the most characters.
fn as_string(v: &Value) -> Result<String> {
    let varying = DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::default() };
    let cast = crate::value::cast(v.clone(), &varying)?;
    if let Value::Char(s) = cast { Ok(s) } else { Ok(cast.to_string()) }
}

/// [§9.8 GR9a] The string `s` as XML text: each character an XML 1.1
/// character, else SQLSTATE 0N002 \[GR9a ii 1\], and, if `charmapping`,
/// `&`, `<`, `>` and a carriage return written as references \[GR9a iii
/// 1\].
fn characters(s: &str, charmapping: bool) -> Result<String> {
    if let Some(bad) = s.chars().find(|&c| !is_char(Version::V11, c)) {
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-14:2006 draft §9.8 GR9a ii] U+{:04X} is no XML 1.1 character", u32::from(bad)),
        )
        .with_state(SqlState::INVALID_XML_CHARACTER));
    }
    if !charmapping {
        return Ok(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#x0d;"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// [§9.8 GR9h] The interval `i` as a duration: its sign, P, then each of
/// its fields and its designator, the leading one of the digits of its
/// precision and the others of two, T before the first of hours, minutes
/// and seconds, the seconds' fraction of the digits of theirs.
fn duration_text(i: &Interval) -> String {
    let q: Qualifier = i.qualifier;
    let second: u128 = MICROS_PER_SECOND.unsigned_abs().into();
    let mut out = String::from(if i.units < 0 { "-P" } else { "P" });
    for field in q.fields() {
        let n = i.field(field).unwrap_or(0).unsigned_abs();
        let width = if field == q.start { usize::from(q.leading) } else { 2 };
        let (designator, whole) = match field {
            Field::Year => ('Y', n),
            Field::Month | Field::Minute => ('M', n),
            Field::Day => ('D', n),
            Field::Hour => ('H', n),
            Field::Second => ('S', n.checked_div(second).unwrap_or(0)),
        };
        if matches!(field, Field::Hour | Field::Minute | Field::Second) && !out.contains('T') {
            out.push('T');
        }
        let _ = write!(out, "{whole:0width$}");
        if field == Field::Second && q.fraction > 0 {
            let fraction = format!("{:06}", n.checked_rem(second).unwrap_or(0));
            out.push('.');
            out.push_str(fraction.get(..usize::from(q.fraction)).unwrap_or_default());
        }
        out.push(designator);
    }
    out
}

/// [§9.8 GR9j iii 2] The name of the element each element of a collection
/// is mapped to.
const ELEMENT: &str = "element";

/// \[DESIGN\] R-CF-6: how many values made of values may enclose a value
/// mapped: as many as types nest (`MAX_TYPE_NESTING`), so every value of
/// a declared type is mapped whole.
const MAX_MAPPED_DEPTH: usize = MAX_TYPE_NESTING;

/// [§9.8 GR3, GR4, GR7] The choices a value is mapped with: how binary
/// strings are encoded \[GR3\]; whether `&`, `<`, `>` and a carriage
/// return are written as references \[GR4\]; and the SQL-session's time
/// zone displacement, in which a datetime without time zone is written,
/// as CAST to a character string writes it \[GR7\]; and NULLS \[GR2\],
/// absent wherever SQL/XML's operators map a value [§6.6 GR3a, §10.13
/// GR4c i 1 B], so a null field is no element \[GR9i ii 2 A\], and nil
/// where a mapping of a table says so [§9.10 GR6b v 3 A], a null field
/// then an element nilled \[GR9i ii 2 B\].
#[derive(Debug, Clone, Copy)]
pub struct Mapping {
    /// How binary strings are encoded.
    pub encoding: BinaryEncoding,
    /// Whether characters are mapped to references.
    pub charmapping: bool,
    /// The SQL-session's time zone displacement, in minutes.
    pub zone: i16,
    /// Whether NULLS is nil, rather than absent.
    pub nil: bool,
}

/// [§9.8] The XML text of `v`, not null, of the declared type `t`: of a
/// distinct type, its source's \[GR1a\]; of a row type, an element of
/// each field not null, named by its name mapped to an XML Name fully
/// escaped [§9.1], NULLS absent \[GR9i\]; of an array or a multiset,
/// an element `element` of each element, of a null one an element marked
/// `xsi:nil="true"` \[GR9j\]; of an XML type, its serialization as
/// CONTENT \[GR9k\]; of another, its lexical form (`xml_lexical`).
///
/// \[DESIGN\] The element of a null element declares the namespace of
/// its prefix `xsi` [§4.9 Table 2], so its text, parsed as CONTENT
/// [§6.6 GR3b, §10.13 GR4c i 1 B], is namespace-well-formed.
///
/// # Errors
///
/// SQLCODE -900 of values nested deeper than `MAX_MAPPED_DEPTH`;
/// SQLSTATE 2200W of an XML value of no serialization; the errors of
/// `xml_lexical`.
pub fn xml_mapped(v: &Value, t: &DataType, m: Mapping) -> Result<String> {
    let mut out = String::new();
    mapped_into(v, t, m, 0, &mut out)?;
    Ok(out)
}

/// The elements of `v`, if it is an array or a multiset.
fn items_of(v: &Value) -> Option<&[Value]> {
    match v {
        Value::Array(items) | Value::Multiset(items) => Some(items),
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => None,
    }
}

/// The element type of `t`, if it is a collection type.
fn element_of(t: &DataType) -> Option<&DataType> {
    if let DataType::Array(a) = t {
        return Some(&a.element);
    }
    t.multiset_type().map(|m| &m.element)
}

/// Appends the XML text of `v`, inside `depth` values, as `xml_mapped`.
fn mapped_into(v: &Value, t: &DataType, m: Mapping, depth: usize, out: &mut String) -> Result<()> {
    if depth > MAX_MAPPED_DEPTH {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] a value inside more than {MAX_MAPPED_DEPTH} others is beyond turborel's limits for §9.8's mapping"),
        ));
    }
    // [GR1a] CAST (DV AS SQLT): a distinct type's value is its source's
    // (RULES.md D69).
    let t = t.represented();
    if let Value::Xml(x) = v {
        out.push_str(&x.text()?);
        return Ok(());
    }
    if let (Some(items), Some(element)) = (items_of(v), element_of(t)) {
        return elements_into(items, element, (m, depth), out);
    }
    if let (Value::Row(values), DataType::Row(r)) = (v, t) {
        return fields_into(values, &r.fields, (m, depth), out);
    }
    // [GR7] A datetime without time zone in the session's displacement,
    // which `shown` reads alone.
    if let Value::Datetime(d) = v {
        let local = Value::Datetime(shown(d, Now { zone: m.zone, ..Now::default() }));
        out.push_str(&xml_lexical(&local, m.encoding, m.charmapping)?);
        return Ok(());
    }
    out.push_str(&xml_lexical(v, m.encoding, m.charmapping)?);
    Ok(())
}

/// Appends an element `name` nilled, `xsi:nil="true"`, declaring the
/// namespace of its prefix `xsi`.
fn nilled(out: &mut String, name: &str) {
    for piece in ["<", name, " xmlns:", XSI_PREFIX, "=\"", XSI_NAMESPACE, "\" ", XSI_PREFIX, ":nil=\"true\"/>"] {
        out.push_str(piece);
    }
}

/// Appends the start tag of an element `name`, or its end tag if `end`.
fn tag(out: &mut String, name: &str, end: bool) {
    out.push_str(if end { "</" } else { "<" });
    out.push_str(name);
    out.push('>');
}

/// [§9.8 GR9j] Appends each of `items`, of the element type `element`:
/// `<element>X</element>`, X its mapping, or `<element xsi:nil="true"/>`
/// of a null one.
fn elements_into(items: &[Value], element: &DataType, (m, depth): (Mapping, usize), out: &mut String) -> Result<()> {
    for item in items {
        if item.is_null() {
            nilled(out, ELEMENT);
            continue;
        }
        tag(out, ELEMENT, false);
        mapped_into(item, element, m, depth.saturating_add(1), out)?;
        tag(out, ELEMENT, true);
    }
    Ok(())
}

/// [§9.8 GR9i] Appends each of `values`, the fields `fields` declare: of a
/// null one none, NULLS absent \[GR9i ii 2 A\], or `<FN xsi:nil="true"/>`,
/// NULLS nil \[GR9i ii 2 B\]; else `<FN>X</FN>`, FN its
/// name mapped by §9.1 fully escaped \[GR9i ii 1\], X its mapping \[GR9i
/// ii 2 C\].
fn fields_into(values: &[Value], fields: &[RowField], (m, depth): (Mapping, usize), out: &mut String) -> Result<()> {
    for (v, f) in values.iter().zip(fields) {
        let name = name_of(&f.name, Escape::Full);
        if v.is_null() {
            if m.nil {
                nilled(out, &name);
            }
            continue;
        }
        tag(out, &name, false);
        mapped_into(v, &f.data_type, m, depth.saturating_add(1), out)?;
        tag(out, &name, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
