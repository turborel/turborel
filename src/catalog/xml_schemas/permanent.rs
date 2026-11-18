//! The permanently registered XML Schemas [ISO/IEC 9075-14:2006 draft
//! §4.2.6 Table 1]: those of the namespaces of XML Schema, of XML Schema
//! instances and of SQL/XML, registered always.
//!
//! \[DESIGN\] Implementation-defined [§4.2.6; Annex B 1b]: their
//! `<registered XML Schema name>`s are INFORMATION_SCHEMA.XS, XSI and
//! SQLXML, after the prefixes Table 1 associates with them, and their
//! schema location URIs their target namespace URIs followed by `.xsd`.
//! Their namespaces and global element declarations are those of the
//! schema documents XML Schema Part 1 and Part 2 and Part 14 give them
//! [Schema1 Appendix A, Schema2 Appendix A, §22.1]. They are not catalog
//! objects but these constants, so no statement
//! makes, changes or drops one, and each is usable by every user, as the
//! Information Schema's objects are.

use super::super::QualifiedName;
use crate::system::INFORMATION_SCHEMA;
use crate::xml::{SQLXML_NAMESPACE, XML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE};

/// A namespace of a permanently registered XML Schema [§4.2.6]: its URI,
/// the local names of its global element declarations, and whether it
/// and each of them is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermanentNamespace {
    /// Its URI.
    pub uri: &'static str,
    /// The local names of its global element declarations.
    pub elements: &'static [&'static str],
    /// Whether it, and each of its global element declarations, is
    /// deterministic.
    pub deterministic: bool,
}

/// [Schema1 Appendix A; Schema2 Appendix A] The global element
/// declarations of the Schema for Schemas, with those of the schema for
/// datatype definitions it includes. Each may hold an `<xs:annotation>`,
/// whose `<xs:appinfo>` and `<xs:documentation>` hold a lax `<xs:any>`
/// of any namespace, so each is non-deterministic [ISO/IEC 9075-14:2006
/// draft §4.2.6].
const XS_ELEMENTS: [&str; 41] = [
    "schema",
    "anyAttribute",
    "complexContent",
    "simpleContent",
    "complexType",
    "element",
    "all",
    "choice",
    "sequence",
    "group",
    "any",
    "attribute",
    "attributeGroup",
    "include",
    "redefine",
    "import",
    "selector",
    "field",
    "unique",
    "key",
    "keyref",
    "notation",
    "appinfo",
    "documentation",
    "annotation",
    "simpleType",
    "restriction",
    "list",
    "union",
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "totalDigits",
    "fractionDigits",
    "length",
    "minLength",
    "maxLength",
    "enumeration",
    "whiteSpace",
    "pattern",
];

/// [ISO/IEC 9075-14:2006 draft §22.1] The global element declarations of
/// the SQL/XML XML Schema, of no wildcard.
const SQLXML_ELEMENTS: [&str; 2] = ["sqltype", "sqlname"];

/// [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1] A permanently registered XML
/// Schema's descriptor, as far as constants give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permanent {
    /// Its `<registered XML Schema name>`'s identifier, in
    /// INFORMATION_SCHEMA.
    pub name: &'static str,
    /// Its target namespace URI.
    pub target_namespace: &'static str,
    /// Its schema location URI.
    pub location: &'static str,
    /// Its namespaces, its target namespace among them [§4.2.6].
    pub namespaces: &'static [PermanentNamespace],
}

impl Permanent {
    /// Its `<registered XML Schema name>`.
    #[must_use]
    pub fn qualified(&self) -> QualifiedName {
        QualifiedName { auth: INFORMATION_SCHEMA.to_owned(), ident: self.name.to_owned() }
    }
}

/// [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1] The permanently registered
/// XML Schemas, in the table's order.
pub const PERMANENT: [Permanent; 3] = [
    Permanent {
        name: "XS",
        target_namespace: XS_NAMESPACE,
        location: "http://www.w3.org/2001/XMLSchema.xsd",
        namespaces: &[
            PermanentNamespace { uri: XS_NAMESPACE, elements: &XS_ELEMENTS, deterministic: false },
            // [Schema1 Appendix A] Its <xs:import> of the XML namespace,
            // for xml:lang, of attributes alone.
            PermanentNamespace { uri: XML_NAMESPACE, elements: &[], deterministic: true },
        ],
    },
    Permanent {
        name: "XSI",
        target_namespace: XSI_NAMESPACE,
        location: "http://www.w3.org/2001/XMLSchema-instance.xsd",
        // [Schema1 §3.2.7] Its components are four attribute declarations.
        namespaces: &[PermanentNamespace { uri: XSI_NAMESPACE, elements: &[], deterministic: true }],
    },
    Permanent {
        name: "SQLXML",
        target_namespace: SQLXML_NAMESPACE,
        location: "http://standards.iso.org/iso/9075/2003/sqlxml.xsd",
        namespaces: &[PermanentNamespace { uri: SQLXML_NAMESPACE, elements: &SQLXML_ELEMENTS, deterministic: true }],
    },
];

/// The permanently registered XML Schema named `name`, if one is.
#[must_use]
pub fn permanent(name: &QualifiedName) -> Option<&'static Permanent> {
    PERMANENT.iter().find(|p| name.auth == INFORMATION_SCHEMA && p.name == name.ident)
}
