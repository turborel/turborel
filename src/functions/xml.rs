//! The functions of the SQL/XML operators [ISO/IEC 9075-14:2006 draft
//! §6]: XMLDOCUMENT [§6.13], XMLPARSE [§6.16], XMLSERIALIZE [§6.8] and
//! the publishing operators (`publish`, `element`), and of the document
//! and content predicates [§8.2, §8.3], as the binder calls them
//! (`binder/xml.rs`); XMLCAST's [§6.6] are in `xml/cast.rs`, the types
//! it casts as [§9.5] in `xml/schema_type.rs`.

mod aggregate;
mod cast;
mod element;
pub mod map;
mod mapping;
mod names;
mod publish;
mod query;
mod schema_type;
mod validate;

pub use aggregate::{XMLAGG, XmlAggregate};
pub use cast::{IMPLICIT_BINARY_ENCODING, XmlCastIn, XmlCastOut, xml_schema_type};
pub use element::{XmlElement, XmlForest};
pub use mapping::{BinaryEncoding, Mapping, xml_lexical, xml_mapped};
pub use names::{IDENTIFIER_TO_XMLNAME, IDENTIFIER_TO_XMLNAME_FULLY, XMLNAME_TO_IDENTIFIER, functions as name_functions};
pub use publish::{XmlComment, XmlConcat, XmlPi, XmlText};
pub use query::{Evaluation, RegularColumn, TableColumn, XmlExistsOf, XmlFilter, XmlQueryOf, XmlRowsOf, XmlTableOf};
pub use schema_type::xml_schema_restriction;
pub use validate::{According, Candidate, ValidBy, XmlValid, XmlValidate};

use super::ScalarFunction;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::xml::XmlValue;
use crate::value::{Binary, DataType, Value};
use crate::xml::{Copying, MAX_INPUT, Parameters, Sequence, Syntax, Version, Whitespace, document_as, parse, parse_bytes, serialize};

/// [§6.16] XMLPARSE, of a character string, or a binary string if
/// `binary`.
#[derive(Debug)]
pub struct XmlParse {
    /// DOCUMENT or CONTENT.
    pub syntax: Syntax,
    /// PRESERVE or STRIP WHITESPACE.
    pub whitespace: Whitespace,
    /// Whether it parses a binary string.
    pub binary: bool,
}

/// The error of a string longer than a parse reads (RULES.md D18, D106).
fn too_long() -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[DESIGN] the XML value is beyond turborel's limits: its text is longer than {MAX_INPUT} octets"),
    )
}

/// The text of `v`, a character string, a large object too, as XMLPARSE
/// reads it: at most `MAX_INPUT` octets; none of the null value or of a
/// binary string.
///
/// # Errors
///
/// SQLCODE -900 if it is longer; as a large object is read; SQLCODE -910
/// if `v` is no string.
pub fn xml_text(v: &Value) -> Result<Option<String>> {
    Ok(match input(v)? {
        Some(Input::Text(text)) => Some(text),
        Some(Input::Octets(_)) | None => None,
    })
}

/// The octets of `v`, a binary string, or the text of `v`, a character
/// string, each at most `MAX_INPUT` octets long; none of the null value.
enum Input {
    Text(String),
    Octets(Vec<u8>),
}

/// `v`, a string, as a parse reads it.
fn input(v: &Value) -> Result<Option<Input>> {
    let most = u64::try_from(MAX_INPUT).unwrap_or(u64::MAX);
    let read = if v.is_null() {
        return Ok(None);
    } else if let Value::Char(s) = v {
        Input::Text(s.clone())
    } else if let Value::Binary(b) = v {
        Input::Octets(b.bytes().to_vec())
    } else if let Value::Lob(l) = v {
        let bytes = l.bounded(most)?.ok_or_else(too_long)?;
        if l.kind() == crate::value::lob::LobKind::Character {
            let text =
                String::from_utf8(bytes).map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] a character large object is not UTF-8"))?;
            Input::Text(text)
        } else {
            Input::Octets(bytes)
        }
    } else {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] XMLPARSE was given {v}, not a string")));
    };
    let length = match &read {
        Input::Text(s) => s.len(),
        Input::Octets(b) => b.len(),
    };
    if length > MAX_INPUT {
        return Err(too_long());
    }
    Ok(Some(read))
}

impl ScalarFunction for XmlParse {
    fn name(&self) -> &'static str {
        "XMLPARSE"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        let t = match self.syntax {
            Syntax::Document => crate::value::xml::XmlType::Document(crate::value::xml::Typing::Untyped),
            Syntax::Content => crate::value::xml::XmlType::Content(crate::value::xml::Typing::Untyped),
        };
        Ok(DataType::Xml(t))
    }

    /// [§6.16 GR4] The null value of the null value; else the string
    /// parsed [§10.16].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(read) = input(args.first().unwrap_or(&Value::Null))? else { return Ok(Value::Null) };
        let tree = match read {
            Input::Text(text) => parse(&text, self.syntax, self.whitespace)?,
            Input::Octets(octets) => parse_bytes(&octets, self.syntax, self.whitespace)?,
        };
        Ok(Value::Xml(XmlValue::of(Sequence::document(tree))))
    }
}

/// [§8.2, §8.3] IS CONTENT, or IS DOCUMENT: whether an XML value is one
/// document node, or one whose children are one element and comments and
/// processing instructions.
#[derive(Debug)]
pub struct XmlIs(pub Syntax);

impl ScalarFunction for XmlIs {
    fn name(&self) -> &'static str {
        match self.0 {
            Syntax::Document => "IS DOCUMENT",
            Syntax::Content => "IS CONTENT",
        }
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    /// [§8.2 GR2, §8.3 GR2] Unknown of the null value \[GR2a\]; True of a
    /// sequence of one document node, of one element and comments and
    /// processing instructions for DOCUMENT \[GR2b\]; else False \[GR2c\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Xml(x)) = args.first() else { return Ok(Value::Null) };
        let sequence = x.sequence()?;
        Ok(Value::Bool(match self.0 {
            Syntax::Document => sequence.is_document(),
            Syntax::Content => sequence.is_content(),
        }))
    }
}

/// [§6.13] XMLDOCUMENT: a document node of an XML value's items, made by
/// XQuery's computed document constructor (`xml::document`).
#[derive(Debug)]
pub struct XmlDocument(pub DataType);

impl ScalarFunction for XmlDocument {
    fn name(&self) -> &'static str {
        "XMLDOCUMENT"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }

    /// [§6.13 GR2] The null value of the null value; else `document {
    /// $EXPR }` of the value \[GR8\], construction mode preserve \[GR5\];
    /// of an UNTYPED declared type, unvalidated, its nodes' types dropped
    /// as §10.19 drops them \[GR9a\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Xml(x)) = args.first() else { return Ok(Value::Null) };
        let untyped = if let DataType::Xml(t) = &self.0 { t.is_untyped() } else { false };
        let mode = if untyped { Copying::UNVALIDATED } else { Copying::PRESERVE };
        Ok(Value::Xml(XmlValue::of(document_as(&*x.sequence()?, mode)?)))
    }

    fn xml_filter(&self) -> Option<XmlFilter> {
        Some(XmlFilter::KeepsNull)
    }
}

/// [§10.18] A copy of an XML value: each node of it a node of a tree of its
/// own, with no parent, as its stored form makes it read back
/// (`xml::encode`).
#[derive(Debug)]
pub struct XmlCopy;

impl ScalarFunction for XmlCopy {
    fn name(&self) -> &'static str {
        "XMLCOPY"
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        Ok(args.first().cloned().unwrap_or(DataType::Xml(crate::value::xml::IMPLICIT)))
    }

    /// The null value of the null value; else a copy of the value.
    fn call(&self, args: &[Value]) -> Result<Value> {
        match args.first() {
            Some(Value::Xml(x)) => {
                let copy = crate::xml::decode(&crate::xml::encode(&*x.sequence()?))?;
                Ok(Value::Xml(XmlValue::of(copy)))
            }
            Some(_) | None => Ok(Value::Null),
        }
    }

    fn xml_filter(&self) -> Option<XmlFilter> {
        Some(XmlFilter::Copy)
    }
}

/// [§6.8, §10.15] What XMLSERIALIZE writes.
#[derive(Debug, Clone)]
pub struct Serialization {
    /// DOCUMENT or CONTENT.
    pub syntax: Syntax,
    /// The string type it makes.
    pub target: DataType,
    /// Whether that is a binary string type.
    pub binary: bool,
    /// Whether a binary string is UTF-16, else UTF-8.
    pub utf16: bool,
    /// The version of XML written.
    pub version: Version,
    /// INCLUDING (true) or EXCLUDING (false) XMLDECLARATION, if written.
    pub declaration: Option<bool>,
}

/// [§6.8] XMLSERIALIZE.
#[derive(Debug)]
pub struct XmlSerialize(pub Serialization);

/// [§10.15 GR4a] UTF-16's byte order mark, which a serialization in it
/// begins with.
const BYTE_ORDER_MARK: char = '\u{FEFF}';

impl XmlSerialize {
    /// [§10.15 GR8] Whether an XML declaration is written: if asked for;
    /// if neither asked for nor refused, of a DOCUMENT of XML 1.1 \[GR8c
    /// i\], and \[GR8c ii\] implementation-defined: not of one of XML 1.0 in
    /// UTF-8 or UTF-16 with no standalone declaration, the only other a
    /// DOCUMENT makes; never of CONTENT \[GR8c iii\].
    fn declares(&self) -> bool {
        let s = &self.0;
        s.declaration.unwrap_or(s.syntax == Syntax::Document && s.version == Version::V11)
    }

    /// The parameters of [Serialization] it serializes by [§10.15 GR4 to
    /// GR10]: \[GR7\] implementation-defined, no standalone declaration;
    /// \[GR9b\] implementation-defined, XML 1.1 undeclares prefixes.
    fn parameters(&self) -> Parameters {
        let s = &self.0;
        Parameters {
            version: s.version,
            declaration: self.declares(),
            encoding: if s.binary && s.utf16 { "UTF-16" } else { "UTF-8" },
            undeclare: s.version == Version::V11,
        }
    }
}

impl ScalarFunction for XmlSerialize {
    fn name(&self) -> &'static str {
        "XMLSERIALIZE"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.target.clone())
    }

    /// [§10.15] The null value of the null value \[GR2\]; else the value
    /// serialized \[GR11\], a document if DOCUMENT \[GR3\], and assigned to
    /// the target type \[GR11a ii, GR11b ii\]: of a binary string, its
    /// octets in UTF-8, or in UTF-16, big-endian, after its byte order
    /// mark \[GR4a i\]; \[GR4a ii\] implementation-defined: UTF-8 has none.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let x = match args.first() {
            None | Some(Value::Null) => return Ok(Value::Null),
            Some(Value::Xml(x)) => x,
            Some(other) => return Err(SqlError::new(Code::Api, format!("[DESIGN] XMLSERIALIZE was given {other}, not an XML value"))),
        };
        let sequence = x.sequence()?;
        if self.0.syntax == Syntax::Document && !sequence.is_document() {
            return Err(SqlError::new(
                Code::Data,
                "[ISO/IEC 9075-14:2006 draft §10.15 GR3] the value serialized as DOCUMENT is not an XML document",
            )
            .with_state(SqlState::NOT_AN_XML_DOCUMENT));
        }
        let text = serialize(&sequence, &self.parameters())?;
        let serialized = if !self.0.binary {
            Value::Char(text)
        } else if self.0.utf16 {
            let units = std::iter::once(BYTE_ORDER_MARK).chain(text.chars()).collect::<String>();
            Value::Binary(Binary::new(units.encode_utf16().flat_map(u16::to_be_bytes).collect()))
        } else {
            Value::Binary(Binary::new(text.into_bytes()))
        };
        let target = &self.0.target;
        target.kind().assign(serialized, target, "ISO/IEC 9075-14:2006 draft §10.15 GR11a ii")
    }
}
