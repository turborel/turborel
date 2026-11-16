//! \[EXTENSION\] Domains, as the catalog describes them [ISO/IEC 9075:1992
//! §11.21].

use super::{ColumnDefault, ConstraintDesc, QualifiedName, Who};
use crate::value::DataType;

/// [ISO/IEC 9075:1992 §11.21 GR3] A domain's descriptor.
#[derive(Debug, Clone)]
pub struct DomainDesc {
    /// The `<domain name>`.
    pub name: QualifiedName,
    /// Its `<data type>`.
    pub data_type: DataType,
    /// Its default [§11.21 GR3, §11.23, §11.24].
    pub default: Option<ColumnDefault>,
    /// Its domain constraints: each a CHECK, VALUE in it the column VALUE
    /// (`parser::VALUE_COLUMN`); a copy of each is on each column of the
    /// domain.
    pub constraints: Vec<ConstraintDesc>,
    /// USAGE on it [§10.3, §11.21 GR4, §11.36]: each grant.
    pub usage: Vec<UsageDesc>,
    /// [ISO/IEC 9075-2:2023 §11.34] The declared type collation, if not
    /// that of the character set.
    pub collation: Option<QualifiedName>,
}

/// [ISO/IEC 9075:1992 §10.3] USAGE of a domain granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDesc {
    /// The grantor: "_SYSTEM" for its owner's [§11.21 GR4].
    pub grantor: String,
    /// The grantee: an `<authorization identifier>`, or none for PUBLIC.
    pub grantee: Option<String>,
    /// WITH GRANT OPTION.
    pub grantable: bool,
}

impl DomainDesc {
    /// Whether `who` holds USAGE on it, and whether grantably: its
    /// schema's owner, `owner`, holds it grantably [§11.21 GR4]; another
    /// holds what is granted it or PUBLIC [§10.3 GR2].
    pub fn usage_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, who, owner)
    }
}

/// Whether `who` holds USAGE by `usage`, and whether grantably: of an
/// object whose schema's owner, `owner`, it enables, grantably; else as
/// granted an identifier it enables or PUBLIC [§10.3 GR2; ISO/IEC
/// 9075-2:2023 §4.42.4].
pub fn usage_held(usage: &[UsageDesc], who: Who<'_>, owner: &str) -> (bool, bool) {
    if who.includes(owner) {
        return (true, true);
    }
    let granted = usage.iter().filter(|u| who.holds(u.grantee.as_deref()));
    granted.fold((false, false), |(_, grantable), u| (true, grantable || u.grantable))
}
