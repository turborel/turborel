//! The functions of SQL/XML's publishing operators [ISO/IEC 9075-14:2006
//! draft §6]: XMLCOMMENT [§6.11], XMLCONCAT [§6.12], XMLPI [§6.17] and
//! XMLTEXT [§6.19], as the binder calls them (`binder/xml/publish.rs`),
//! each RETURNING SEQUENCE; RETURNING CONTENT is XMLDOCUMENT's of it.

use super::super::{BinaryEncoding, ScalarFunction, xml_lexical};
use super::{Input, input};
use crate::error::{Code, Result, SqlError};
use crate::value::xml::XmlValue;
use crate::value::{DataType, Value};
use crate::xml::{Sequence, comment, concatenate, processing_instruction, text};

/// The XML value of `sequence`, RETURNING SEQUENCE.
fn xml(sequence: Sequence) -> Value {
    Value::Xml(XmlValue::of(sequence))
}

/// The characters of `v`, a character string, as a comment's content
/// holds them [§6.11 GR4a]; none of the null value.
fn characters(v: &Value) -> Result<Option<String>> {
    match input(v)? {
        None => Ok(None),
        Some(Input::Text(s)) => Ok(Some(s)),
        Some(Input::Octets(_)) => Err(SqlError::new(Code::Api, "[DESIGN] XMLCOMMENT was given a binary string")),
    }
}

/// [§6.11] XMLCOMMENT, RETURNING SEQUENCE.
#[derive(Debug)]
pub struct XmlComment;

impl ScalarFunction for XmlComment {
    fn name(&self) -> &'static str {
        "XMLCOMMENT"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(crate::value::xml::XmlType::Sequence))
    }

    /// [§6.11 GR2] The null value of the null value; else the comment of
    /// the string \[GR3 to GR5\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(content) = characters(args.first().unwrap_or(&Value::Null))? else { return Ok(Value::Null) };
        Ok(xml(comment(&content)?))
    }
}

/// [§6.19] XMLTEXT, RETURNING SEQUENCE, of binary strings in the
/// `<XML binary encoding>` with innermost scope that contains it \[GR3,
/// GR4\].
#[derive(Debug)]
pub struct XmlText(pub BinaryEncoding);

impl ScalarFunction for XmlText {
    fn name(&self) -> &'static str {
        "XMLTEXT"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(crate::value::xml::XmlType::Sequence))
    }

    /// [§6.19 GR2] The null value of the null value; else the text node
    /// of the string mapped to XML, its characters as they are [§9.8]
    /// \[GR5 to GR7\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let v = args.first().unwrap_or(&Value::Null);
        if v.is_null() {
            return Ok(Value::Null);
        }
        Ok(xml(text(&xml_lexical(v, self.0, false)?)?))
    }
}

/// [§6.17] XMLPI of its target, RETURNING SEQUENCE.
#[derive(Debug)]
pub struct XmlPi(pub Box<str>);

impl ScalarFunction for XmlPi {
    fn name(&self) -> &'static str {
        "XMLPI"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(crate::value::xml::XmlType::Sequence))
    }

    /// [§6.17 GR1] The null value of the null value; else the processing
    /// instruction of the target and the string mapped to XML, its
    /// characters as they are [§9.8], or of '' if none is written \[GR2
    /// to GR10\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(v) = args.first() else { return Ok(xml(processing_instruction(&self.0, "")?)) };
        if v.is_null() {
            return Ok(Value::Null);
        }
        Ok(xml(processing_instruction(&self.0, &xml_lexical(v, BinaryEncoding::Hex, false)?)?))
    }
}

/// [§6.12] XMLCONCAT, RETURNING SEQUENCE.
#[derive(Debug)]
pub struct XmlConcat;

impl ScalarFunction for XmlConcat {
    fn name(&self) -> &'static str {
        "XMLCONCAT"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(crate::value::xml::XmlType::Sequence))
    }

    /// [§6.12 GR1 to GR4] Each value concatenated with the last [§10.14]:
    /// every item of each that is not null, in order; the null value if
    /// every one is.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let mut held = Vec::with_capacity(args.len());
        for v in args {
            if let Value::Xml(x) = v {
                held.push(x.sequence()?);
            }
        }
        Ok(concatenate(held.iter().map(|s| Some(&**s))).map_or(Value::Null, xml))
    }
}
