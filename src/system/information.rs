//! [ISO/IEC 9075:1992 §21.2] The Information Schema: its views, each defined
//! as the standard defines it, over the Definition Schema
//! (`system/definition.rs`), bound as owned by INFORMATION_SCHEMA wherever a
//! statement names one; and its base table INFORMATION_SCHEMA_CATALOG_NAME
//! [§21.2.2]. The text is the standard's but where it is not SQL, each
//! correction noted. Its assertion [§21.2.3] holds of the table, which has one
//! row; its domains [§21.2.27-§21.2.29] are not defined as domains: its
//! columns are of their types (`system.rs`).
//!
//! Where a view of ISO/IEC 9075:1992 chooses what to show by CURRENT_USER,
//! as a privilege's grantee or grantor or a schema's owner, each enabled
//! role chooses too, read from ENABLED_ROLES (`information/roles.rs`), as
//! the views of ISO/IEC 9075-11:2006 draft Clause 5 read it: what a role a
//! session has set brings it is shown as its privileges are current
//! [ISO/IEC 9075-2:2023 §4.42.4]. So TABLE_PRIVILEGES, COLUMN_PRIVILEGES
//! and USAGE_PRIVILEGES take their grantee or grantor test together in
//! parentheses, as the draft does [§5.19, §5.60, §5.71], before AND tests
//! the catalog, and the owner of a schema is tested by CURRENT_USER where
//! COLUMNS wrote USER, which is the same [ISO/IEC 9075:1992 §6.2 SR3].

mod composites;
mod constraints;
mod extensions;
mod modules;
mod objects;
mod roles;
mod short_names;
mod structured;
mod triggers;
mod usage;
mod user_types;
mod xml_schemas;

use std::sync::OnceLock;

use super::{INFORMATION_SCHEMA, Seen, SystemTable, catalog_name, identifier};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::error::Result;
use crate::storage::Row;

/// [ISO/IEC 9075:1992 §21.2.2] INFORMATION_SCHEMA_CATALOG_NAME: one row,
/// the name of the catalog in which the Information Schema resides.
pub(super) struct CatalogName;

impl SystemTable for CatalogName {
    fn name(&self) -> &'static str {
        "INFORMATION_SCHEMA_CATALOG_NAME"
    }
    fn columns(&self) -> Vec<ColumnDesc> {
        vec![identifier("CATALOG_NAME", true)]
    }
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        Ok(vec![vec![catalog_name(seen)]])
    }
}

/// A view of the Information Schema: its name, and its `<query
/// expression>`, which reads the Definition Schema.
#[derive(Debug, Clone, Copy)]
pub struct SystemView {
    /// Its name, in INFORMATION_SCHEMA.
    pub name: &'static str,
    /// Its `<query expression>`.
    pub query: &'static str,
}

/// The Information Schema view `name`, if there is one.
pub fn view(name: &QualifiedName) -> Option<&'static SystemView> {
    views().iter().find(|v| name.auth == INFORMATION_SCHEMA && v.name == name.ident)
}

/// The views of the Information Schema: those of ISO/IEC 9075:1992
/// (`objects.rs`, `constraints.rs`), then those of `information/`'s
/// modules, arrays and row types, user-defined types, structured types,
/// roles, triggers, SQL-server modules, usage (`usage.rs`) and registered
/// XML Schemas, then the short name views (`short_names.rs`), then the
/// extensions (`extensions.rs`).
pub(crate) fn views() -> &'static [SystemView] {
    static ALL: OnceLock<Vec<SystemView>> = OnceLock::new();
    let parts = [composites::VIEWS, user_types::VIEWS, structured::VIEWS, roles::VIEWS, triggers::VIEWS, modules::VIEWS, usage::VIEWS];
    let short = short_names::PARTS.as_slice();
    let standard = [objects::VIEWS, constraints::VIEWS];
    ALL.get_or_init(|| [standard.as_slice(), parts.as_slice(), &[xml_schemas::VIEWS], short, &[extensions::VIEWS]].concat().concat())
}
