//! Abstract syntax of the SQL/XML operators [ISO/IEC 9075-14:2006 draft
//! §6], each a `ValueExpression::Xml`: XMLCAST [§6.6], XMLCOMMENT
//! [§6.11], XMLCONCAT [§6.12], XMLDOCUMENT [§6.13], XMLELEMENT [§6.14],
//! XMLFOREST [§6.15], XMLPARSE [§6.16], XMLPI [§6.17], XMLSERIALIZE [§6.8]
//! and XMLTEXT [§6.19].

mod query;

pub use query::{OnEmpty, XmlArgument, XmlArguments, XmlQuery, XmlRegularColumn, XmlTable, XmlTableColumn};

use super::{SqlStatement, TypeName, ValueExpression};
use crate::functions::BinaryEncoding;
use crate::value::DataType;
use crate::xml::{Syntax, Whitespace};

/// [ISO/IEC 9075-14:2006 draft §6.5, §6.18] `<XML passing mechanism>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Passing {
    /// BY REF: the value's nodes themselves.
    ByRef,
    /// BY VALUE: a copy of them [§10.18].
    ByValue,
}

/// [ISO/IEC 9075-14:2006 draft §11.4] `<XML returning clause>` ::=
/// RETURNING { CONTENT | SEQUENCE }.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Returning {
    /// RETURNING CONTENT.
    Content,
    /// RETURNING SEQUENCE.
    Sequence,
}

/// [§11.3] `<XML namespace declaration item>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceItem {
    /// `<XML regular namespace declaration item>` ::= `<XML namespace
    /// URI>` AS `<XML namespace prefix>`.
    Regular {
        /// The `<XML namespace URI>`.
        uri: String,
        /// The `<XML namespace prefix>`, an `<identifier>`.
        prefix: String,
    },
    /// `<XML default namespace declaration item>` ::= DEFAULT `<XML
    /// namespace URI>`.
    Default(String),
    /// NO DEFAULT.
    NoDefault,
}

/// [§11.3] `<XML lexically scoped options>` ::= `<XML lexically scoped
/// option>` \[ , `<XML lexically scoped option>` \], at most one of each
/// kind \[SR1\]: the namespaces and the binary encoding of its scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LexicallyScopedOptions {
    /// The `<XML namespace declaration>`'s items, if written.
    pub namespaces: Option<Vec<NamespaceItem>>,
    /// The `<XML binary encoding>`, if written: XMLBINARY \[USING\]
    /// BASE64 or HEX.
    pub binary: Option<BinaryEncoding>,
}

impl SqlStatement {
    /// The `<XML lexically scoped options>` of WITH after its first word,
    /// if it is a DELETE, INSERT, MERGE or UPDATE that writes them
    /// [§14.3 to §14.7]: their scope is the statement \[SR1, SR2\].
    #[must_use]
    pub fn xml_options(&self) -> Option<&LexicallyScopedOptions> {
        if let SqlStatement::DeleteSearched { xml, .. }
        | SqlStatement::UpdateSearched { xml, .. }
        | SqlStatement::UpdatePositioned { xml, .. } = self
        {
            return xml.as_deref();
        }
        if let SqlStatement::Insert(i) = self {
            return i.xml.as_ref();
        }
        if let SqlStatement::Merge(m) = self {
            return m.xml.as_ref();
        }
        None
    }
}

/// [§6.14] `<XML content option>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentOption {
    /// NULL ON NULL.
    NullOnNull,
    /// EMPTY ON NULL.
    EmptyOnNull,
    /// ABSENT ON NULL.
    AbsentOnNull,
    /// NIL ON NULL.
    NilOnNull,
    /// NIL ON NO CONTENT.
    NilOnNoContent,
}

/// [§6.14] An `<XML attribute>`, or [§6.15] a `<forest element>`: a value
/// and, if written, the name AS gives it.
#[derive(Debug, Clone)]
pub struct Named {
    /// The `<XML attribute value>` or `<forest element value>`.
    pub value: ValueExpression,
    /// The `<XML attribute name>` or `<forest element name>`, an
    /// `<identifier>`.
    pub name: Option<String>,
}

/// [§6.14] `<XML element>` ::= XMLELEMENT ( NAME `<XML element name>` \[
/// , `<XML namespace declaration>` \] \[ , `<XML attributes>` \] \[ { ,
/// `<XML element content>` }... \[ OPTION `<XML content option>` \] \]
/// \[`<XML returning clause>`\] ).
#[derive(Debug, Clone)]
pub struct ElementConstructor {
    /// The `<XML element name>`, an `<identifier>`.
    pub name: String,
    /// The `<XML namespace declaration>`'s items, if written.
    pub namespaces: Option<Vec<NamespaceItem>>,
    /// The `<XML attribute>`s of XMLATTRIBUTES, if written.
    pub attributes: Vec<Named>,
    /// The `<XML element content>`s.
    pub content: Vec<ValueExpression>,
    /// The `<XML content option>`, if written.
    pub option: Option<ContentOption>,
    /// The `<XML returning clause>`, if written.
    pub returning: Option<Returning>,
}

/// [§6.15] `<XML forest>` ::= XMLFOREST ( \[ `<XML namespace declaration>`
/// , \] `<forest element list>` \[ OPTION `<XML content option>` \]
/// \[`<XML returning clause>`\] ).
#[derive(Debug, Clone)]
pub struct ForestConstructor {
    /// The `<XML namespace declaration>`'s items, if written.
    pub namespaces: Option<Vec<NamespaceItem>>,
    /// The `<forest element>`s.
    pub elements: Vec<Named>,
    /// The `<XML content option>`, if written.
    pub option: Option<ContentOption>,
    /// The `<XML returning clause>`, if written.
    pub returning: Option<Returning>,
}

/// An SQL/XML operator.
#[derive(Debug, Clone)]
pub enum XmlOperator {
    /// [§6.6] `<XML cast specification>` ::= XMLCAST ( `<XML cast
    /// operand>` AS `<XML cast target>` \[`<XML passing mechanism>`\] ).
    Cast {
        /// The `<value expression>`, or none for NULL.
        operand: Option<ValueExpression>,
        /// The `<XML cast target>`: a `<data type>`, or a `<domain name>`.
        target: TypeName,
        /// The `<XML passing mechanism>`, if written.
        passing: Option<Passing>,
    },
    /// [§6.13] `<XML document>` ::= XMLDOCUMENT ( `<XML value expression>`
    /// \[`<XML returning clause>`\] ).
    Document {
        /// The XML value made a document node's children.
        operand: ValueExpression,
        /// The `<XML returning clause>`, if written.
        returning: Option<Returning>,
    },
    /// [§6.11] `<XML comment>` ::= XMLCOMMENT ( `<character value
    /// expression>` \[`<XML returning clause>`\] ).
    Comment {
        /// The comment's content.
        operand: ValueExpression,
        /// The `<XML returning clause>`, if written.
        returning: Option<Returning>,
    },
    /// [§6.12] `<XML concatenation>` ::= XMLCONCAT ( `<XML value
    /// expression>` { , `<XML value expression>` }... \[`<XML returning
    /// clause>`\] ).
    Concat {
        /// The XML values concatenated, two or more.
        operands: Vec<ValueExpression>,
        /// The `<XML returning clause>`, if written.
        returning: Option<Returning>,
    },
    /// [§6.14] `<XML element>`.
    Element(ElementConstructor),
    /// [§6.15] `<XML forest>`.
    Forest(ForestConstructor),
    /// [§6.17] `<XML PI>` ::= XMLPI ( NAME `<XML PI target>` \[ ,
    /// `<character value expression>` \] \[`<XML returning clause>`\] ).
    Pi {
        /// The `<XML PI target>`, an `<identifier>`.
        target: String,
        /// The processing instruction's content, if written.
        operand: Option<ValueExpression>,
        /// The `<XML returning clause>`, if written.
        returning: Option<Returning>,
    },
    /// [§6.19] `<XML text>` ::= XMLTEXT ( `<character value expression>`
    /// \[`<XML returning clause>`\] ).
    Text {
        /// The text node's content.
        operand: ValueExpression,
        /// The `<XML returning clause>`, if written.
        returning: Option<Returning>,
    },
    /// [§6.16] `<XML parse>` ::= XMLPARSE ( `<document or content>`
    /// `<string value expression>` `<XML whitespace option>` ).
    Parse {
        /// DOCUMENT or CONTENT.
        syntax: Syntax,
        /// The string parsed.
        operand: ValueExpression,
        /// PRESERVE or STRIP WHITESPACE.
        whitespace: Whitespace,
    },
    /// [§6.18] `<XML query>`.
    Query(XmlQuery),
    /// [§8.4] `<XML exists predicate>`, a boolean value.
    Exists(XmlQuery),
    /// [§7.1] `<XML table>`, the collection of a table primary's rows
    /// alone.
    Table(XmlTable),
    /// [§6.8] `<XML character string serialization>` and `<XML binary
    /// string serialization>` ::= XMLSERIALIZE ( \[`<document or
    /// content>`\] `<XML value expression>` AS `<data type>` \[ENCODING
    /// `<XML encoding name>`\] \[`<XML serialize version>`\] \[`<XML
    /// declaration option>`\] ).
    Serialize {
        /// DOCUMENT or CONTENT, CONTENT if neither is written \[SR2a,
        /// SR4a\].
        syntax: Syntax,
        /// The XML value serialized.
        operand: ValueExpression,
        /// The string type it is serialized as.
        target: DataType,
        /// The `<XML encoding name>`, if written, of a binary string.
        encoding: Option<String>,
        /// The `<XML serialize version>`'s literal, if written.
        version: Option<String>,
        /// INCLUDING (true) or EXCLUDING (false) XMLDECLARATION, if
        /// written.
        declaration: Option<bool>,
    },
}

impl XmlOperator {
    /// Calls `f` on each value expression it holds.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            XmlOperator::Cast { operand, .. } => {
                if let Some(v) = operand {
                    f(v);
                }
            }
            XmlOperator::Comment { operand, .. }
            | XmlOperator::Document { operand, .. }
            | XmlOperator::Parse { operand, .. }
            | XmlOperator::Serialize { operand, .. }
            | XmlOperator::Text { operand, .. } => f(operand),
            XmlOperator::Concat { operands, .. } => operands.iter().for_each(f),
            XmlOperator::Pi { operand, .. } => operand.iter().for_each(f),
            XmlOperator::Element(x) => {
                x.attributes.iter().for_each(|a| f(&a.value));
                x.content.iter().for_each(f);
            }
            XmlOperator::Forest(x) => x.elements.iter().for_each(|e| f(&e.value)),
            XmlOperator::Query(q) | XmlOperator::Exists(q) => q.arguments.iter().for_each(|a| a.for_each_child(f)),
            XmlOperator::Table(t) => t.arguments.iter().for_each(|a| a.for_each_child(f)),
        }
    }
}
