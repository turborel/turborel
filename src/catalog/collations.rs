//! \[EXTENSION\] Collations, as the catalog describes them [ISO/IEC 9075-2:2023
//! §4.3.6, §11.43]. A descriptor holds what a `<collation definition>` made:
//! the built-in collation it comes from, and its pad characteristic. The
//! built-ins themselves are in `value/collation.rs`, each in the schema
//! INFORMATION_SCHEMA.

use super::QualifiedName;
use super::domains::{UsageDesc, usage_held};
use crate::value::collation::Pad;

/// [ISO/IEC 9075-2:2023 §4.3.6, §11.43 GR5] A collation's descriptor.
#[derive(Debug, Clone)]
pub struct CollationDesc {
    /// The `<collation name>`.
    pub name: QualifiedName,
    /// The built-in collation it orders by.
    pub base: String,
    /// Its pad characteristic.
    pub pad: Pad,
    /// USAGE on it [§11.43 GR2]: each grant.
    pub usage: Vec<UsageDesc>,
}

impl CollationDesc {
    /// Whether `auth` holds USAGE on it, and whether grantably, as of a
    /// domain (`DomainDesc::usage_of`).
    pub fn usage_of(&self, auth: &str, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, auth, owner)
    }
}
