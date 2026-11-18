//! Registered XML Schemas, as the catalog describes them [ISO/IEC
//! 9075-14:2006 draft §4.2.6]: the registered XML Schema descriptor, its
//! name, target namespace and schema location, the schema document it was
//! registered with, the registered XML Schemas whose documents it
//! includes, redefines or imports, and the USAGE granted on it [§4.7.1].
//!
//! \[DESIGN\] RULES.md D86: the document is kept as its text, and the
//! schema it is read into (`xml::schema::Schema`) is made again from it
//! when the catalog is read, with the documents of those it uses, so the
//! catalog stores nothing derived. Whether the schema, a namespace or a
//! global element declaration is non-deterministic is read from that
//! schema, not stored [§4.2.6].

use std::sync::Arc;

use super::domains::{UsageDesc, usage_held};
use super::{QualifiedName, Who};
use crate::xml::schema::Schema;

/// [ISO/IEC 9075-14:2006 draft §4.2.6] A registered XML Schema descriptor.
#[derive(Clone)]
pub struct XmlSchemaDesc {
    /// The `<registered XML Schema name>` [§5.2].
    pub name: QualifiedName,
    /// The target namespace URI: the zero-length string of a schema of no
    /// target namespace, as NO NAMESPACE indicates [§11.6 SR2a i].
    pub target_namespace: String,
    /// The schema location URI.
    pub location: String,
    /// The text of the schema document registered.
    pub document: String,
    /// The registered XML Schemas whose documents it includes, redefines
    /// or imports [Schema1 §4.2], which it depends on.
    pub uses: Vec<QualifiedName>,
    /// USAGE on it [§4.7.1]: each grant.
    pub usage: Vec<UsageDesc>,
    /// The schema it is, read from its document; none until read, as of a
    /// descriptor the catalog has just decoded.
    pub schema: Option<Arc<Schema>>,
}

impl std::fmt::Debug for XmlSchemaDesc {
    /// Its stored fields: its schema, derived from them, is not shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XmlSchemaDesc")
            .field("name", &self.name)
            .field("target_namespace", &self.target_namespace)
            .field("location", &self.location)
            .field("document", &self.document)
            .field("uses", &self.uses)
            .field("usage", &self.usage)
            .finish_non_exhaustive()
    }
}

impl XmlSchemaDesc {
    /// Whether `who` holds USAGE on it, and whether grantably, as of a
    /// domain (`DomainDesc::usage_of`); its schema's owner `owner` holds
    /// it, grantably.
    pub fn usage_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, who, owner)
    }

    /// [ISO/IEC 9075-14:2006 draft §4.2.6] Whether it is non-deterministic;
    /// false until its schema is read.
    #[must_use]
    pub fn is_non_deterministic(&self) -> bool {
        self.schema.as_ref().is_some_and(|s| s.is_non_deterministic())
    }
}
