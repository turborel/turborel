//! Registered XML Schemas, as the catalog describes them [ISO/IEC
//! 9075-14:2006 draft §4.2.6]: the registered XML Schema descriptor, its
//! name, target namespace and schema location, the schema document it was
//! registered with, the registered XML Schemas whose documents it
//! includes, redefines or imports, and the USAGE granted on it [§4.7.1].
//!
//! \[DESIGN\] RULES.md D106: the document is kept as its text, and the
//! schema it is read into (`xml::schema::Schema`) is made again from it
//! when the catalog is read, with the documents of those it uses, so the
//! catalog stores nothing derived. Whether the schema, a namespace or a
//! global element declaration is non-deterministic is read from that
//! schema, not stored [§4.2.6].
//!
//! The permanently registered XML Schemas are constants
//! (`xml_schemas/permanent.rs`), and USAGE, as an Access Rule asks for it,
//! is `may_use` (`xml_schemas/privileges.rs`).

mod permanent;
mod privileges;

use std::sync::Arc;

use super::domains::{UsageDesc, usage_held};
use super::{QualifiedName, Who};
use crate::value::DataType;
use crate::value::types::MAX_TYPE_NESTING;
use crate::xml::schema::Schema;
pub use permanent::{PERMANENT, Permanent, PermanentNamespace, permanent};
pub use privileges::may_use;

/// [ISO/IEC 9075-14:2006 draft §4.2.2, §6.1 GR1d] The registered XML
/// Schemas the types of XMLSCHEMA `t` is or holds indicate: itself, an
/// element, a field or a distinct type's source, no deeper than types
/// nest; each once, in the order met.
#[must_use]
pub fn xml_schemas_of(t: &DataType) -> Vec<QualifiedName> {
    let mut found = Vec::new();
    let mut pending: Vec<(&DataType, usize)> = vec![(t, 0)];
    while let Some((part, depth)) = pending.pop() {
        let deeper = depth.saturating_add(1);
        if deeper > MAX_TYPE_NESTING.saturating_add(1) {
            continue;
        }
        match part {
            DataType::Xml(x) => {
                let named = x.schema().map(|n| QualifiedName { auth: n.schema.clone(), ident: n.name.clone() });
                found.extend(named.filter(|n| !found.contains(n)));
            }
            DataType::Array(a) => pending.push((&a.element, deeper)),
            DataType::Multiset(m) => pending.push((&m.element, deeper)),
            DataType::Row(r) => pending.extend(r.fields.iter().map(|f| (&f.data_type, deeper))),
            DataType::Distinct(d) => pending.push((&d.source, deeper)),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Json
            | DataType::Extension(_) => {}
        }
    }
    found
}

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
