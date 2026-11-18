//! Abstract syntax of what indicates a registered XML Schema [ISO/IEC
//! 9075-14:2006 draft §11.6]: an `<XML valid according to what>` and its
//! `<XML valid element clause>`, as the XML types of XMLSCHEMA [§6.1]
//! write them after XMLSCHEMA, and XMLVALIDATE [§6.20] and IS VALID
//! [§8.5] after ACCORDING TO XMLSCHEMA.

use super::super::TableName;

/// [§6.20] `<document or content or sequence>`, DCS [§6.20 SR1, §8.5
/// SR2]: what shape the value validated has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dcs {
    /// DOCUMENT: one document node of one element and comments and
    /// processing instructions, validated whole.
    Document,
    /// CONTENT: one document node, each element child validated.
    Content,
    /// SEQUENCE: a sequence of document, element, comment and processing
    /// instruction nodes.
    Sequence,
}

/// [§11.6] `<XML valid according to what>`: the registered XML Schema
/// meant.
#[derive(Debug, Clone)]
pub enum XmlValidWhat {
    /// `<XML valid according to URI>` ::= URI `<XML valid target namespace
    /// URI>` \[`<XML valid schema location>`\] | NO NAMESPACE \[`<XML valid
    /// schema location>`\]: the target namespace URI, the zero-length string
    /// of NO NAMESPACE \[SR2a\], and the schema location URI, if written.
    Uri {
        /// NSURI \[SR2a\].
        namespace: String,
        /// The `<XML valid schema location URI>`, if written.
        location: Option<String>,
    },
    /// `<XML valid according to identifier>` ::= ID `<registered XML
    /// Schema name>` \[SR1\].
    Id(TableName),
}

/// [§11.6] `<XML valid according to what>` \[`<XML valid element
/// clause>`\]: a registered XML Schema, and the namespace and global
/// element declaration of it the clause indicates, if written.
#[derive(Debug, Clone)]
pub struct XmlValidAccording {
    /// The `<XML valid according to what>`.
    pub what: XmlValidWhat,
    /// The `<XML valid element namespace specification>`, if written: its
    /// `<XML valid element namespace URI>`, the zero-length string of NO
    /// NAMESPACE \[SR4a i\].
    pub namespace: Option<String>,
    /// The `<XML valid element name>` of the `<XML valid element name
    /// specification>`, if written.
    pub element: Option<String>,
    /// Where it begins.
    pub at: usize,
}

impl XmlValidAccording {
    /// Whether it has an `<XML valid element clause>`.
    #[must_use]
    pub fn has_element_clause(&self) -> bool {
        self.namespace.is_some() || self.element.is_some()
    }
}
