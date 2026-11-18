//! The permanently registered XML Schemas [ISO/IEC 9075-14:2006 draft
//! §4.2.6 Table 1]: those of the namespaces of XML Schema, of XML Schema
//! instances and of SQL/XML, registered always.
//!
//! \[DESIGN\] Implementation-defined [§4.2.6; Annex B 1b]: their
//! `<registered XML Schema name>`s are INFORMATION_SCHEMA.XS, XSI and
//! SQLXML, after the prefixes Table 1 associates with them, and their
//! schema location URIs their target namespace URIs followed by `.xsd`.
//! They are not catalog objects but these constants, so no statement
//! makes, changes or drops one, and each is usable by every user, as the
//! Information Schema's objects are.

use super::super::QualifiedName;
use crate::system::INFORMATION_SCHEMA;
use crate::xml::{SQLXML_NAMESPACE, XS_NAMESPACE, XSI_NAMESPACE};

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
    Permanent { name: "XS", target_namespace: XS_NAMESPACE, location: "http://www.w3.org/2001/XMLSchema.xsd" },
    Permanent { name: "XSI", target_namespace: XSI_NAMESPACE, location: "http://www.w3.org/2001/XMLSchema-instance.xsd" },
    Permanent { name: "SQLXML", target_namespace: SQLXML_NAMESPACE, location: "http://standards.iso.org/iso/9075/2003/sqlxml.xsd" },
];

/// The permanently registered XML Schema named `name`, if one is.
#[must_use]
pub fn permanent(name: &QualifiedName) -> Option<&'static Permanent> {
    PERMANENT.iter().find(|p| name.auth == INFORMATION_SCHEMA && p.name == name.ident)
}
