//! [ISO/IEC 9075-14:2006 draft §4.2, §6.1] The XML types, whose values
//! are `Value::Xml` (RULES.md D86).
//!
//! XML values are not comparable [§4.2.4]: no equality, grouping or
//! ordering operation takes one [§10.8 to §10.11], so none compares,
//! hashes or is in a key. Types of XML are comparable with each other
//! alone, as a union or a CASE combines them [§10.3]. In a record and the
//! log a value is its stored form as a binary large object is: its
//! octets, kept in the row, or the id of the large object its table keeps
//! them in (`lob.rs`, `engine/rows/lobs.rs`).

use std::cmp::Ordering;

use super::lob::{put_lob, read_lob};
use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::lob::LobKind;
use crate::value::xml::{Typing, XmlType, XmlValue};
use crate::value::{Bucket, DataType, Value, assignment_rule, unassignable};

/// [§6.1] XML, the name of the XML types.
pub const XML_NAME: &str = "XML";

/// The kind of the XML types.
#[derive(Debug)]
pub struct XmlKind;

/// The XML types.
pub static XML: XmlKind = XmlKind;

// The bytes of a type's parameters (`put_params`).
/// XML(DOCUMENT(...)).
const DOCUMENT: u8 = 0;
/// XML(CONTENT(...)).
const CONTENT: u8 = 1;
/// XML(SEQUENCE).
const SEQUENCE: u8 = 2;
/// UNTYPED, or no secondary modifier, of SEQUENCE.
const UNTYPED: u8 = 0;
/// ANY.
const ANY: u8 = 1;

/// The XML type `t` is, XML(SEQUENCE) if it is none.
fn xml_type(t: &DataType) -> XmlType {
    if let DataType::Xml(x) = t { *x } else { XmlType::Sequence }
}

/// `v`, a value assigned or cast to a site of `t`, checked to be of its
/// primary type: one XQuery document node, of one element and comments
/// and processing instructions if a DOCUMENT type [ISO/IEC 9075-14:2006
/// draft §10.2 GR1a, §6.5 GR1a], by the rule `rule`.
fn of_primary(v: XmlValue, t: XmlType, rule: &str) -> Result<Value> {
    let fits = match t {
        XmlType::Document(_) => v.sequence()?.is_document(),
        XmlType::Content(_) => v.sequence()?.is_content(),
        XmlType::Sequence => true,
    };
    if fits {
        return Ok(Value::Xml(v));
    }
    let (what, state) = match t {
        XmlType::Document(_) => ("not an XML document", SqlState::NOT_AN_XML_DOCUMENT),
        XmlType::Content(_) | XmlType::Sequence => ("not an XQuery document node", SqlState::NOT_AN_XQUERY_DOCUMENT_NODE),
    };
    Err(SqlError::new(Code::Data, format!("[{rule}] the value of {} is {what}", t.written())).with_state(state))
}

impl SqlType for XmlKind {
    fn name(&self) -> &'static str {
        XML_NAME
    }

    fn written(&self, t: &DataType) -> String {
        xml_type(t).written()
    }

    /// [§10.3] Of the XML types alone, which a union or a CASE combines.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Xml(_))
    }

    /// None: XML values are not comparable [§4.2.4].
    fn compare(&self, _: &Value, _: &Value) -> Option<Ordering> {
        None
    }

    /// None: no XML value is grouped [§10.9 SR1], so none is in a bucket.
    fn bucket(&self, _: &Value) -> Option<Bucket> {
        None
    }

    /// [§10.2 GR1a] An XML value, of `t`'s primary type.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        if let Value::Xml(x) = v {
            return of_primary(x, xml_type(t), &assignment_rule(rule, "ISO/IEC 9075-14:2006 draft §10.2 GR1a"));
        }
        if v.is_null() { Ok(Value::Null) } else { unassignable(&v, t, rule) }
    }

    /// [§6.5 SR3, GR1a] An XML value, of `t`'s primary type; a value of
    /// no other type. \[SR2\] implementation-defined: BY REF is implicit,
    /// so the value is its nodes, not a copy of them.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if let Value::Xml(x) = v {
            return of_primary(x, xml_type(t), "ISO/IEC 9075-14:2006 draft §6.5 GR1a");
        }
        if v.is_null() { Ok(Value::Null) } else { crate::value::cast::uncastable(&v, t) }
    }

    /// \[EXTENSION\] What XMLPARSE of its serialization as CONTENT makes:
    /// SQL has no XML literal.
    fn literal(&self, v: &Value) -> String {
        let Value::Xml(x) = v else { return v.to_string() };
        let text = x.text().unwrap_or_default();
        format!("XMLPARSE(CONTENT '{}' PRESERVE WHITESPACE)", text.replace('\'', "''"))
    }

    /// \[EXTENSION\] Its serialization as CONTENT, as it is shown (RULES.md
    /// D86); if it has none, a word saying so.
    fn text(&self, v: &Value) -> String {
        let Value::Xml(x) = v else { return v.to_string() };
        x.text().unwrap_or_else(|_| "(an XML value with no serialization)".to_owned())
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075:1992 §21.3.5] XML, its other columns null.
    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(XML_NAME)
    }

    /// Its stored form as a binary large object's (`lob::put_lob`): its
    /// octets, or the id its table keeps them by.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let lob = if let Value::Xml(x) = v { x.lob().ok() } else { None };
        put_lob(&lob.map_or(Value::Null, Value::Lob), out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        if let Value::Lob(lob) = read_lob(r, LobKind::Binary)? {
            return Ok(Value::Xml(XmlValue::stored(lob)));
        }
        Err(r.corrupt("an XML value is no binary large object"))
    }

    /// Its primary modifier, then its secondary one.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let typing = |s: Typing| match s {
            Typing::Untyped => UNTYPED,
            Typing::Any => ANY,
        };
        let (primary, secondary) = match xml_type(t) {
            XmlType::Document(s) => (DOCUMENT, typing(s)),
            XmlType::Content(s) => (CONTENT, typing(s)),
            XmlType::Sequence => (SEQUENCE, UNTYPED),
        };
        out.put_u8(primary);
        out.put_u8(secondary);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let (primary, secondary) = (r.u8()?, r.u8()?);
        let typing = match secondary {
            UNTYPED => Typing::Untyped,
            ANY => Typing::Any,
            other => return Err(r.corrupt(&format!("an XML type's secondary modifier is {other}"))),
        };
        Ok(DataType::Xml(match (primary, typing) {
            (DOCUMENT, _) => XmlType::Document(typing),
            (CONTENT, _) => XmlType::Content(typing),
            (SEQUENCE, Typing::Untyped) => XmlType::Sequence,
            (other, _) => return Err(r.corrupt(&format!("an XML type's primary modifier is {other}"))),
        }))
    }

    /// Of variable length.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    /// As `put_value`: a table has made it held inline, or a reference to
    /// the large object it keeps.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Value::Xml(x) = v else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}")));
        };
        let mut out = Vec::new();
        put_lob(&Value::Lob(x.lob()?), &mut out);
        Ok(out)
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        if let Value::Lob(lob) = read_lob(&mut Reader::new(field, "an XML value"), LobKind::Binary).ok()? {
            return Some(Value::Xml(XmlValue::stored(lob)));
        }
        None
    }

    /// None: no XML value is in a key [§10.9 SR1, §10.11 SR1].
    fn encode_key(&self, v: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-14:2006 draft §10.11 SR1] {v} of {t} is in no key, as no XML value is")))
    }

    fn decode_key(&self, _: &mut &[u8], t: &DataType) -> Result<Value> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-14:2006 draft §10.11 SR1] no key is of {t}, as no XML value is")))
    }

    fn anchor(&self, _: &Value, _: &DataType) -> Option<(Value, bool)> {
        None
    }
}

#[cfg(test)]
mod tests;
