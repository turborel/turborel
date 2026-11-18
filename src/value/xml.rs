//! [ISO/IEC 9075-14:2006 draft §4.2] XML types and their values.
//!
//! An XML type is XML(DOCUMENT), XML(CONTENT), each UNTYPED, ANY or
//! XMLSCHEMA, the types of values valid according to a registered XML
//! Schema, or XML(SEQUENCE) [§4.2.2, §6.1]. An XML value is an XQuery
//! sequence (`xml::Sequence`) [§4.2.3].
//!
//! \[DESIGN\] RULES.md D86: a value is held as its sequence, or as a binary
//! large object of its stored form (`xml::encode`), as a table keeps it:
//! in the row if it is short, else in the table's large objects, which
//! D79 writes, logs and reads a chunk at a time. A value held stored is
//! read whole, at most `xml::MAX_STORED` octets, when its nodes are
//! needed: unlike a large object string, a value is a tree to be read.

use std::sync::Arc;

use super::Value;
use super::lob::{Lob, LobKind};
use super::types::XmlSchemaNamed;
use crate::error::{Code, Result, SqlError};
use crate::xml::{self, MAX_STORED, Parameters, Sequence, Syntax, Version, Whitespace};

/// [§6.1] The `<secondary XML type modifier>` of a DOCUMENT or CONTENT
/// type: whether its values are untyped, of any type, or valid according
/// to a registered XML Schema.
///
/// \[DESIGN\] RULES.md D86: XMLSCHEMA holds what its type descriptor
/// includes [§4.2.2, §6.1 GR1d to GR1f] behind an `Arc`, so a type is
/// cloned as cheaply as it was copied before it had one; a registered XML
/// Schema is identified by its name [§4.2.6], so the type holds the name,
/// not the schema, and is stored and sent as itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typing {
    /// UNTYPED: every element is `xdt:untyped`, every attribute
    /// `xdt:untypedAtomic` [§4.2.3].
    Untyped,
    /// ANY.
    Any,
    /// XMLSCHEMA: valid according to the indicated registered XML Schema,
    /// and its indicated XML namespace and global element declaration, if
    /// any [§4.2.3, §11.6].
    Schema(Arc<XmlSchemaNamed>),
}

/// [§4.2.2] An XML type, as its descriptor gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlType {
    /// XML(DOCUMENT(...)): one document node of one element and comments
    /// and processing instructions.
    Document(Typing),
    /// XML(CONTENT(...)): one document node.
    Content(Typing),
    /// XML(SEQUENCE): any XQuery sequence.
    Sequence,
}

/// \[DESIGN\] RULES.md D86 implementation-defined [§6.1 SR2, SR3b]: XML
/// alone is XML(SEQUENCE), and DOCUMENT or CONTENT alone is ANY, the
/// widest, so every XML value is assignable to a site of XML [§4.2.4].
pub const IMPLICIT: XmlType = XmlType::Sequence;
/// The implicit `<secondary XML type modifier>` (`IMPLICIT`).
pub const IMPLICIT_TYPING: Typing = Typing::Any;

impl XmlType {
    /// The type as a `<data type>` writes it [§6.1]: of XMLSCHEMA, its
    /// registered XML Schema by ID, then its namespace and element as an
    /// `<XML valid element clause>` writes them [§11.6].
    #[must_use]
    pub fn written(&self) -> String {
        match (self.schema(), self.secondary()) {
            (Some(named), _) => format!("XML({}(XMLSCHEMA {}))", self.primary(), according_written(named)),
            (None, Some(secondary)) => format!("XML({}({secondary}))", self.primary()),
            (None, None) => format!("XML({})", self.primary()),
        }
    }

    /// Its `<primary XML type modifier>`, as XML_PRIMARY_MODIFIER shows it
    /// [§6.1] [§21.1 Description 1a i].
    #[must_use]
    pub fn primary(&self) -> &'static str {
        match self {
            XmlType::Document(_) => "DOCUMENT",
            XmlType::Content(_) => "CONTENT",
            XmlType::Sequence => "SEQUENCE",
        }
    }

    /// Its `<secondary XML type modifier>`, as XML_SECONDARY_MODIFIER shows
    /// it: none of SEQUENCE [§6.1] [§21.1 Description 1a ii].
    #[must_use]
    pub fn secondary(&self) -> Option<&'static str> {
        let typing = |t: &Typing| match t {
            Typing::Untyped => "UNTYPED",
            Typing::Any => "ANY",
            Typing::Schema(_) => "XMLSCHEMA",
        };
        match self {
            XmlType::Document(t) | XmlType::Content(t) => Some(typing(t)),
            XmlType::Sequence => None,
        }
    }

    /// Of XMLSCHEMA, what its descriptor includes of the registered XML
    /// Schema it indicates [§6.1 GR1d to GR1f].
    #[must_use]
    pub fn schema(&self) -> Option<&XmlSchemaNamed> {
        match self {
            XmlType::Document(Typing::Schema(named)) | XmlType::Content(Typing::Schema(named)) => Some(named),
            XmlType::Document(Typing::Untyped | Typing::Any) | XmlType::Content(Typing::Untyped | Typing::Any) | XmlType::Sequence => None,
        }
    }

    /// Whether it is XML(DOCUMENT(UNTYPED)) or XML(CONTENT(UNTYPED)).
    #[must_use]
    pub fn is_untyped(&self) -> bool {
        matches!(self, XmlType::Document(Typing::Untyped) | XmlType::Content(Typing::Untyped))
    }

    /// Whether a value of `source` may be assigned to a site of `self`
    /// [§4.2.4] [§10.1 SR1, §10.2 SR1b], as CAST BY REF casts one [§6.5
    /// SR4]: to an UNTYPED type, a value of an UNTYPED one; to one of
    /// XMLSCHEMA, a value of one of XMLSCHEMA of the same registered XML
    /// Schema, of the namespace and element `self` indicates, if any
    /// \[SR1b ii\]; to another, any.
    #[must_use]
    pub fn takes(&self, source: &XmlType) -> bool {
        match self {
            XmlType::Document(Typing::Untyped) | XmlType::Content(Typing::Untyped) => source.is_untyped(),
            XmlType::Document(Typing::Schema(target)) | XmlType::Content(Typing::Schema(target)) => {
                source.schema().is_some_and(|s| indicates_within(target, s))
            }
            XmlType::Document(Typing::Any) | XmlType::Content(Typing::Any) | XmlType::Sequence => true,
        }
    }
}

/// [§10.3 SR1b ii 2 B III to VIII] What types of XMLSCHEMA have in common,
/// if each of `types` is one, of one registered XML Schema S: S, and the
/// namespace NS each includes, if each includes one identical \[IV, VII\],
/// and the element each includes, if each includes NS and one equivalent
/// \[III, VI\]. None if one is of another type or of another schema \[IX\].
#[must_use]
pub fn schema_combined(types: &[XmlType]) -> Option<XmlSchemaNamed> {
    let named: Vec<&XmlSchemaNamed> = types.iter().map(XmlType::schema).collect::<Option<_>>()?;
    let first = *named.first()?;
    if !named.iter().all(|n| n.schema == first.schema && n.name == first.name) {
        return None;
    }
    let namespace = first.namespace.clone().filter(|ns| named.iter().all(|n| n.namespace.as_ref() == Some(ns)));
    let element = first.element.clone().filter(|e| namespace.is_some() && named.iter().all(|n| n.element.as_ref() == Some(e)));
    Some(XmlSchemaNamed { schema: first.schema.clone(), name: first.name.clone(), namespace, element })
}

/// [§10.2 SR1b ii] Whether a type of XMLSCHEMA indicating `source` is
/// assignable to one indicating `target`: the same registered XML Schema,
/// identified by its name [§4.2.6]; the namespace `target` indicates, if
/// any, identical [SR1b ii 2]; and its element's NCName, if any,
/// equivalent [SR1b ii 1].
fn indicates_within(target: &XmlSchemaNamed, source: &XmlSchemaNamed) -> bool {
    let same_schema = target.schema == source.schema && target.name == source.name;
    let namespace = target.namespace.is_none() || target.namespace == source.namespace;
    let element = target.element.is_none() || target.element == source.element;
    same_schema && namespace && element
}

/// [§11.6] The `<XML valid according to what>` and `<XML valid element
/// clause>` that indicate `named`: ID and its `<registered XML Schema
/// name>`; then NAMESPACE and its URI, or NO NAMESPACE of the zero-length
/// one \[SR4a i\]; then ELEMENT and its NCName, a delimited identifier, so
/// the text reads back as it was.
#[must_use]
pub fn according_written(named: &XmlSchemaNamed) -> String {
    let mut out = format!("ID {}.{}", named.schema, named.name);
    match named.namespace.as_deref() {
        Some("") => out.push_str(" NO NAMESPACE"),
        Some(uri) => {
            out.push_str(" NAMESPACE '");
            out.push_str(&uri.replace('\'', "''"));
            out.push('\'');
        }
        None => {}
    }
    if let Some(element) = &named.element {
        out.push_str(" ELEMENT \"");
        out.push_str(&element.replace('"', "\"\""));
        out.push('"');
    }
    out
}

/// A value held, stored, or sent.
#[derive(Debug, Clone)]
enum Form {
    /// Its sequence.
    Held(Arc<Sequence>),
    /// A binary large object of its stored form.
    Stored(Lob),
    /// Its serialization as CONTENT (`text`), as the protocol carries it
    /// and a client holds it.
    Text(Arc<str>),
}

/// [§4.2.3] An XML value that is not null: an XQuery sequence.
#[derive(Debug, Clone)]
pub struct XmlValue(Form);

impl XmlValue {
    /// The value of `sequence`.
    #[must_use]
    pub fn of(sequence: Sequence) -> XmlValue {
        XmlValue(Form::Held(Arc::new(sequence)))
    }

    /// The value whose serialization as CONTENT is `text`, as the protocol
    /// carries it (`text`).
    #[must_use]
    pub fn serialized(text: &str) -> XmlValue {
        XmlValue(Form::Text(Arc::from(text)))
    }

    /// The value whose stored form `lob`, a binary large object, holds.
    #[must_use]
    pub fn stored(lob: Lob) -> XmlValue {
        XmlValue(Form::Stored(lob))
    }

    /// Its sequence, read from its stored form if it is held so.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its stored form is longer than `MAX_STORED` octets
    /// or is not one; an error of reading the large object holding it.
    pub fn sequence(&self) -> Result<Arc<Sequence>> {
        match &self.0 {
            Form::Held(sequence) => Ok(Arc::clone(sequence)),
            Form::Stored(lob) => {
                let most = u64::try_from(MAX_STORED).unwrap_or(u64::MAX);
                let bytes = lob.bounded(most)?.ok_or_else(|| {
                    SqlError::new(Code::Corrupt, format!("[DESIGN] an XML value stored is longer than {MAX_STORED} octets (RULES.md D86)"))
                })?;
                xml::decode(&bytes).map(Arc::new)
            }
            Form::Text(text) => xml::parse(text, Syntax::Content, Whitespace::Preserve).map(|t| Arc::new(Sequence::document(t))),
        }
    }

    /// \[EXTENSION\] Its serialization as CONTENT, as the protocol carries
    /// it (RULES.md D86): in XML 1.0 with no XML declaration, or, if 1.0
    /// cannot hold it, in XML 1.1 after one, so a parse of the text as
    /// CONTENT reads it back [ISO/IEC 9075-14:2006 draft §10.15].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200W if it has no serialization, as a sequence of an
    /// attribute alone has none; the errors of reading it.
    pub fn text(&self) -> Result<String> {
        if let Form::Text(text) = &self.0 {
            return Ok(text.to_string());
        }
        let sequence = self.sequence()?;
        let v10 = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };
        xml::serialize(&sequence, &v10)
            .or_else(|_| xml::serialize(&sequence, &Parameters { version: Version::V11, declaration: true, undeclare: true, ..v10 }))
    }

    /// \[DESIGN\] Whether it may be a value of a type of XMLSCHEMA [§4.2.3]:
    /// each element of it has a type annotation, which only a validation
    /// gives (`xml::schema::validate`). A value held stored is one a table
    /// keeps, of a column whose type was checked as it was stored; a value
    /// sent as text is parsed, so untyped.
    ///
    /// # Errors
    ///
    /// The errors of reading it.
    pub fn annotated(&self) -> Result<bool> {
        if let Form::Stored(_) = &self.0 {
            return Ok(true);
        }
        let sequence = self.sequence()?;
        Ok(sequence.items().iter().all(|item| match item {
            xml::Item::Atomic(_) => true,
            xml::Item::Node(node) => {
                let tree = node.tree();
                std::iter::once(node.id())
                    .chain(tree.descendants(node.id()))
                    .all(|id| tree.kind(id) != Some(xml::Kind::Element) || tree.annotation(id).is_some())
            }
        }))
    }

    /// Its stored form, as a binary large object.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is longer than `MAX_STORED` octets (RULES.md
    /// D18, D86).
    pub fn lob(&self) -> Result<Lob> {
        match &self.0 {
            Form::Stored(lob) => Ok(lob.clone()),
            Form::Held(_) | Form::Text(_) => {
                let bytes = xml::encode(&*self.sequence()?);
                if bytes.len() > MAX_STORED {
                    return Err(SqlError::new(
                        Code::Unsupported,
                        format!(
                            "[DESIGN] the XML value is beyond turborel's limits: its stored form is longer than {MAX_STORED} octets (RULES.md D86)"
                        ),
                    ));
                }
                Ok(Lob::of_octets(&bytes))
            }
        }
    }

    /// \[DESIGN\] The octets that stand for it where a value has a key of
    /// its identity, which no XML value has in SQL [§10.9 SR1]: its stored
    /// form, if it is held, or stored in its row.
    #[must_use]
    pub fn octets(&self) -> Vec<u8> {
        match &self.0 {
            Form::Held(sequence) => xml::encode(sequence),
            Form::Stored(lob) => lob.inline().map(<[u8]>::to_vec).unwrap_or_default(),
            Form::Text(text) => text.as_bytes().to_vec(),
        }
    }

    /// \[DESIGN\] How many octets it holds, about, for what a spill counts.
    #[must_use]
    pub fn held_bytes(&self) -> usize {
        match &self.0 {
            Form::Held(sequence) => sequence.held_bytes(),
            Form::Stored(lob) => lob.held_bytes(),
            Form::Text(text) => text.len(),
        }
    }

    /// The binary large object it is held stored in, if it is.
    #[must_use]
    pub fn stored_lob(&self) -> Option<&Lob> {
        match &self.0 {
            Form::Stored(lob) => Some(lob),
            Form::Held(_) | Form::Text(_) => None,
        }
    }

    /// The same, to replace as a table keeps it.
    pub fn stored_lob_mut(&mut self) -> Option<&mut Lob> {
        match &mut self.0 {
            Form::Stored(lob) => Some(lob),
            Form::Held(_) | Form::Text(_) => None,
        }
    }
}

impl super::DataType {
    /// \[DESIGN\] The kind of large object its values are stored as, if
    /// they are: a large object string's own (RULES.md D79), or binary for
    /// an XML type, whose stored form a binary large object holds (D86); a
    /// distinct type's values as its source's are, being its source's
    /// values (D69).
    #[must_use]
    pub fn lob_kind(&self) -> Option<LobKind> {
        let represented = self.represented();
        if let super::DataType::Lob { kind, .. } = represented {
            return Some(*kind);
        }
        matches!(represented, super::DataType::Xml(_)).then_some(LobKind::Binary)
    }
}

impl Value {
    /// \[DESIGN\] Whether a table stores it as a large object: a large
    /// object string (RULES.md D79), or an XML value (D86).
    #[must_use]
    pub fn is_lob_stored(&self) -> bool {
        matches!(self, Value::Lob(_) | Value::Xml(_))
    }

    /// \[DESIGN\] The large object it is, or that holds its stored form: a
    /// large object string's, or an XML value's held stored (RULES.md D79,
    /// D86).
    #[must_use]
    pub fn as_lob(&self) -> Option<&Lob> {
        if let Value::Lob(l) = self {
            return Some(l);
        }
        if let Value::Xml(x) = self {
            return x.stored_lob();
        }
        None
    }

    /// The same, to replace as a table keeps it.
    pub fn as_lob_mut(&mut self) -> Option<&mut Lob> {
        if let Value::Lob(l) = self {
            return Some(l);
        }
        if let Value::Xml(x) = self {
            return x.stored_lob_mut();
        }
        None
    }
}
