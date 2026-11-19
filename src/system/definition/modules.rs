//! [ISO/IEC 9075-4:2006 draft §19.1 to §19.4] The Definition Schema's base
//! tables of SQL-server modules (Feature P003): MODULES, of each module
//! descriptor [§9.21 GR3]; MODULE_PRIVILEGES, of each privilege on one;
//! and MODULE_TABLE_USAGE and MODULE_COLUMN_USAGE, of the tables and
//! columns its routines reference, as they bind now, but its declared
//! local temporary tables, which no other view shows either. The views of
//! modules read them (`system/information/modules.rs`).
//!
//! A module's character set is SQL_TEXT, the one there is. Its
//! MODULE_AUTHORIZATION is null, as no `<module authorization clause>` is
//! written of a server module [§19.4 Description 5]. CREATED and
//! LAST_ALTERED are its creation and last-altered timestamps [Description
//! 7, 8] (`ModuleDesc::stamps`), of the domain TIME_STAMP [ISO/IEC
//! 9075-11:2006 draft §5.6]; no statement alters a module, so they are
//! the same.

use std::sync::Arc;

use super::privileges::PUBLIC;
use super::triggers::stamped;
use super::{Columns, defined, ids, long, sql_text};
use crate::catalog::{ColumnDesc, ModuleDesc, QualifiedName, module_of_schema};
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, text, yes};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("MODULES", modules_columns, Rows::Infallible(modules)),
        defined("MODULE_PRIVILEGES", privileges_columns, Rows::Infallible(privileges)),
        defined("MODULE_TABLE_USAGE", table_usage_columns, Rows::Infallible(table_usage)),
        defined("MODULE_COLUMN_USAGE", column_usage_columns, Rows::Fallible(column_usage)),
    ]
}

// ---------------------------------------------------------------------------
// [§19.4] MODULES
// ---------------------------------------------------------------------------

fn modules_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("MODULE", true)
        .ids("DEFAULT_CHARACTER_SET", true)
        .id("DEFAULT_SCHEMA_CATALOG", true)
        .id("DEFAULT_SCHEMA_NAME", true)
        .data("MODULE_DEFINITION", false)
        .id("MODULE_AUTHORIZATION", false)
        .data("SQL_PATH", false)
        .time_stamp("CREATED")
        .time_stamp("LAST_ALTERED")
        .done()
}

/// Each module, in the order of creation: its text, and its path as
/// written if it was, each null if longer than CHARACTER_DATA holds
/// [Description 4, 6].
fn modules(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |m: &ModuleDesc| {
        let path = m.path_text.as_deref().map_or(Value::Null, long);
        let (created, altered) = (stamped(m.stamps.created), stamped(m.stamps.last_altered));
        let tail = vec![catalog.clone(), text(&m.schema), long(&m.text), Value::Null, path, created, altered];
        [ids(&catalog, &m.name).to_vec(), ids(&catalog, &sql_text()).to_vec(), tail].concat()
    };
    seen.state.catalog.modules().into_iter().map(row).collect()
}

// ---------------------------------------------------------------------------
// [§19.2] MODULE_PRIVILEGES
// ---------------------------------------------------------------------------

fn privileges_columns() -> Vec<ColumnDesc> {
    Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("MODULE", true).data("PRIVILEGE_TYPE", true).data("IS_GRANTABLE", true).done()
}

/// [§10.2] Each EXECUTE on each module, its owner's the one its definition
/// granted [§9.21 GR2].
fn privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for m in seen.state.catalog.modules() {
        for u in &m.execute {
            let head = vec![text(&u.grantor), text(u.grantee.as_deref().unwrap_or(PUBLIC))];
            rows.push([head, ids(&catalog, &m.name).to_vec(), vec![text("EXECUTE"), yes(u.grantable)]].concat());
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// [§19.3] MODULE_TABLE_USAGE, [§19.1] MODULE_COLUMN_USAGE
// ---------------------------------------------------------------------------

fn table_usage_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("MODULE", true).ids("TABLE", true).done()
}

/// Each table or view a routine of each module names, once, but its own
/// declared tables.
fn table_usage(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for m in seen.state.catalog.modules() {
        let mut named: Vec<&QualifiedName> = Vec::new();
        for r in seen.state.catalog.routines().into_iter().filter(|r| r.module_name().as_ref() == Some(&m.name)) {
            for t in r.reads.iter().filter(|t| module_of_schema(&t.auth).is_none()) {
                if !named.contains(&t) {
                    named.push(t);
                }
            }
        }
        rows.extend(named.into_iter().map(|t| [ids(&catalog, &m.name), ids(&catalog, t)].concat()));
    }
    rows
}

fn column_usage_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("MODULE", true).ids("TABLE", true).id("COLUMN_NAME", true).done()
}

/// Each column of a table or view a routine of each module references,
/// once, as its body binds now, but those of its own declared tables.
///
/// # Errors
///
/// A routine's body does not bind.
fn column_usage(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for m in seen.state.catalog.modules() {
        let mut used: Vec<(QualifiedName, String)> = Vec::new();
        for r in seen.state.catalog.routines().into_iter().filter(|r| r.module_name().as_ref() == Some(&m.name)) {
            for u in seen.state.routine_columns(r)? {
                if module_of_schema(&u.0.auth).is_none() && !used.contains(&u) {
                    used.push(u);
                }
            }
        }
        rows.extend(used.into_iter().map(|(t, c)| [ids(&catalog, &m.name).to_vec(), ids(&catalog, &t).to_vec(), vec![text(&c)]].concat()));
    }
    Ok(rows)
}
