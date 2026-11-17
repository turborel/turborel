//! [ISO/IEC 9075-2:2023 §4.15.3, §4.16, §11.3 GR7, §12.2 GR6] What the
//! Definition Schema shows of typed tables and views, as ISO/IEC
//! 9075-11:2006 draft §6.20, §6.22, §6.48 and §6.50 define it: the
//! columns of TABLES naming a table's structured type, self-referencing
//! column and how its values are made; whether a column of COLUMNS is
//! self-referencing; DIRECT_SUPERTABLES, one row for each typed table or
//! view of a direct supertable or superview; and TABLE_METHOD_PRIVILEGES,
//! one row for each table/method privilege.
//!
//! A table/method privilege granted by SELECT is found as §11.3 GR7 and
//! §12.2 GR6 make it: of each SELECT on a typed table, one on each method
//! of its type, of its own and of its supertypes. The owner's, of
//! "_SYSTEM", is on every method; a granted one is on each method its
//! grantor holds EXECUTE on WITH GRANT OPTION. One granted by SELECT of a
//! `<privilege method list>` is kept with the table's privileges.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::privileges::descriptors;
use super::structured::specs;
use super::{Columns, catalog_name, defined, ids, text};
use crate::catalog::{ColumnDesc, QualifiedName, StructuredDesc, TypedDesc, method_executable, udt_qualified};
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable, yes};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("DIRECT_SUPERTABLES", supertables_columns, Rows::Infallible(supertables)),
        defined("TABLE_METHOD_PRIVILEGES", method_privileges_columns, Rows::Fallible(method_privileges)),
    ]
}

/// What makes the table or view `name` typed, if it is.
pub(super) fn typed<'a>(seen: &'a Seen<'_>, name: &QualifiedName) -> Option<&'a TypedDesc> {
    let catalog = &*seen.state.catalog;
    let table = catalog.table(name).and_then(|t| t.typed.as_ref());
    table.or_else(|| catalog.view(name).and_then(|v| v.typed.as_ref()))
}

/// Then the columns of TABLES after TABLE_TYPE [§6.50].
pub(super) fn table_columns(columns: Columns) -> Columns {
    columns
        .id("SELF_REFERENCING_COLUMN_NAME", false)
        .data("REFERENCE_GENERATION", false)
        .ids("USER_DEFINED_TYPE", false)
        .data("IS_TYPED", true)
}

/// [§6.50 Description 3, 5, 6, 8] Of a table or view `typed` makes typed,
/// its self-referencing column, how its values are made, its structured
/// type and YES; of another, nulls and NO.
pub(super) fn table_values(catalog: &Value, typed: Option<&TypedDesc>) -> Vec<Value> {
    match typed {
        None => vec![Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, yes(false)],
        Some(t) => {
            let head = vec![text(&t.self_referencing), text(t.generation.name())];
            [head, ids(catalog, &t.udt).to_vec(), vec![yes(true)]].concat()
        }
    }
}

/// [§6.20] IS_SELF_REFERENCING of column `column` of a table `typed`
/// makes typed, or of another: whether it is the self-referencing column.
pub(super) fn self_referencing(typed: Option<&TypedDesc>, column: &str) -> Value {
    yes(typed.is_some_and(|t| t.self_referencing == column))
}

// ---------------------------------------------------------------------------
// [§6.22] DIRECT_SUPERTABLES
// ---------------------------------------------------------------------------

fn supertables_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).id("SUPERTABLE_NAME", true).done()
}

/// [§6.22 Description 1, 2] Each typed table of a direct supertable, then
/// each typed view of a direct superview, and the supertable's name, of
/// the subtable's schema [ISO/IEC 9075-2:2023 §11.3 SR7].
fn supertables(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let tables = seen.state.catalog.tables().into_iter().map(|t| (&t.name, t.typed.as_ref()));
    let views = seen.state.catalog.views().into_iter().map(|v| (&v.name, v.typed.as_ref()));
    let row = |(name, typed): (&QualifiedName, Option<&TypedDesc>)| {
        let supertable = typed?.supertable.as_ref()?;
        Some([ids(&catalog, name).to_vec(), vec![text(&supertable.ident)]].concat())
    };
    tables.chain(views).filter_map(row).collect()
}

// ---------------------------------------------------------------------------
// [§6.48] TABLE_METHOD_PRIVILEGES
// ---------------------------------------------------------------------------

fn method_privileges_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("TABLE", true).ids("SPECIFIC", true);
    columns.data("IS_GRANTABLE", true).done()
}

/// [§11.3 GR7, §12.2 GR6] Of each SELECT on each typed table or view, a
/// table/method privilege on each method of its type, of the same
/// grantor, grantee and grantability: every one of "_SYSTEM"'s, each its
/// grantor holds EXECUTE on WITH GRANT OPTION of another's.
///
/// # Errors
///
/// A view of the Information Schema does not bind: a defect.
fn method_privileges(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut all: BTreeMap<(String, String, QualifiedName, QualifiedName), bool> = BTreeMap::new();
    let mut put = |key, grantable: bool| *all.entry(key).or_insert(false) |= grantable;
    for ((grantor, grantee, table, kind, column), grantable) in descriptors(seen)? {
        let Some(udt) = typed(seen, &table).map(|t| &t.udt) else { continue };
        if kind != "SELECT" || column.is_some() {
            continue;
        }
        for (_, specific) in methods(seen, udt) {
            if grantor == SYSTEM_GRANTOR || method_executable(&*seen.state.catalog, &specific, &grantor) {
                put((grantor.clone(), grantee.clone(), table.clone(), specific), grantable);
            }
        }
    }
    // [§12.2 GR3] Those granted by SELECT of a <privilege method list>.
    for (table, p) in explicit(seen) {
        for specific in &p.actions.typed.methods {
            let grantee = p.grantee.clone().unwrap_or_else(|| super::privileges::PUBLIC.to_owned());
            put((p.grantor.clone(), grantee, table.clone(), specific.clone()), p.grantable);
        }
    }
    let rows = all.into_iter().map(|((grantor, grantee, table, specific), grantable)| {
        [vec![text(&grantor), text(&grantee)], ids(&catalog, &table).to_vec(), ids(&catalog, &specific).to_vec(), vec![yes(grantable)]]
            .concat()
    });
    Ok(rows.collect())
}

/// The privileges kept on each typed table and referenceable view, each
/// with its table.
fn explicit<'a>(seen: &'a Seen<'_>) -> Vec<(QualifiedName, &'a crate::catalog::PrivilegeDesc)> {
    let catalog = &*seen.state.catalog;
    let tables = catalog.tables().into_iter().filter(|t| t.typed.is_some()).map(|t| t.name.clone());
    let named: Vec<QualifiedName> =
        tables.chain(catalog.views().into_iter().filter(|v| v.typed.is_some()).map(|v| v.name.clone())).collect();
    named.into_iter().flat_map(|n| catalog.privileges(&n).into_iter().map(move |p| (n.clone(), p))).collect()
}

/// The specific name of each method of the type `udt`: its own method
/// specifications', then each proper supertype's, nearest first, each with
/// the type it is of.
fn methods<'a>(seen: &'a Seen<'_>, udt: &QualifiedName) -> Vec<(&'a StructuredDesc, QualifiedName)> {
    let catalog = &*seen.state.catalog;
    let Some(s) = catalog.structured_type(udt) else { return Vec::new() };
    let family = std::iter::once(s).chain(s.supertypes.iter().filter_map(|n| catalog.structured_type(&udt_qualified(n))));
    family.flat_map(|t| specs(crate::catalog::MethodHost::Structured(t)).into_iter().map(move |m| (t, m.specific))).collect()
}
