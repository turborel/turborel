//! [ISO/IEC 9075-11:2006 draft §6.13, §6.18, §6.35, §6.37 to §6.39,
//! §6.63] The Definition Schema's base tables of what SQL routines, views,
//! constraints and generated columns depend on: ROUTINE_TABLE_USAGE,
//! ROUTINE_COLUMN_USAGE, ROUTINE_ROUTINE_USAGE and ROUTINE_SEQUENCE_USAGE,
//! of what each SQL-invoked routine's body names; VIEW_ROUTINE_USAGE, of
//! the routines each view's query invokes; CHECK_CONSTRAINT_ROUTINE_USAGE,
//! of those each assertion's and domain's or table's check constraint's
//! condition invokes; and COLUMN_COLUMN_USAGE, of the columns each
//! generated column's generation expression references.
//!
//! Each is read from what the database keeps of each definition to refuse
//! a drop that would leave it naming nothing, RESTRICT's dependencies: the
//! tables and views a routine's body names, its subject routines and the
//! sequence generators it draws from, as the routine was defined
//! (`SqlRoutineDesc::reads`, `horizon`, `sequences`); the subject routines
//! of a view and of a check constraint, fixed as each was defined
//! (`Horizon::subjects`, RULES.md D73). The columns a generation
//! expression references are found by binding it now, as
//! VIEW_COLUMN_USAGE's are, and so are a routine's body's, counted as
//! Description 1 counts them (`binder::Counting`).

use std::sync::Arc;

use super::{Columns, catalog_name, defined, ids, text};
use crate::binder::generations;
use crate::catalog::{ColumnDesc, ConstraintDesc, ConstraintKind, QualifiedName};
use crate::error::Result;
use crate::logical::expr_columns;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("ROUTINE_TABLE_USAGE", routine_tables_columns, Rows::Infallible(routine_tables)),
        defined("ROUTINE_COLUMN_USAGE", routine_columns_columns, Rows::Fallible(routine_columns)),
        defined("ROUTINE_ROUTINE_USAGE", routine_routines_columns, Rows::Infallible(routine_routines)),
        defined("ROUTINE_SEQUENCE_USAGE", routine_sequences_columns, Rows::Infallible(routine_sequences)),
        defined("VIEW_ROUTINE_USAGE", view_routines_columns, Rows::Infallible(view_routines)),
        defined("CHECK_CONSTRAINT_ROUTINE_USAGE", check_routines_columns, Rows::Infallible(check_routines)),
        defined("COLUMN_COLUMN_USAGE", column_columns_columns, Rows::Fallible(column_columns)),
    ]
}

/// A row of each of `objects`, each named by `of`: the name of what
/// depends, then each name it gives of what it depends on, once.
fn named_once<T>(seen: &Seen<'_>, objects: &[T], of: impl Fn(&T) -> (QualifiedName, Vec<QualifiedName>)) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for o in objects {
        let (name, used) = of(o);
        let mut seen_once: Vec<&QualifiedName> = Vec::new();
        for u in &used {
            if !seen_once.contains(&u) {
                seen_once.push(u);
                rows.push([ids(&catalog, &name), ids(&catalog, u)].concat());
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// [§6.39] ROUTINE_TABLE_USAGE, [§6.35] ROUTINE_COLUMN_USAGE
// ---------------------------------------------------------------------------

fn routine_tables_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("SPECIFIC", true).ids("TABLE", true).done()
}

/// [§6.39 Description 1] Each table and view each SQL-invoked routine's
/// body names, of the routine's specific name.
fn routine_tables(seen: &Seen<'_>) -> Vec<Row> {
    named_once(seen, &seen.state.catalog.routines(), |r| (r.specific.clone(), r.reads.clone()))
}

fn routine_columns_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("SPECIFIC", true).ids("TABLE", true).id("COLUMN_NAME", true).done()
}

/// [§6.35 Description 1] Each column of a table or view each SQL-invoked
/// routine's body references in the search condition of a searched
/// UPDATE or DELETE or in an `<update source>`, or names in an `<insert
/// column list>` or a `<set clause>`, once, as the body binds now.
///
/// # Errors
///
/// A routine's body does not bind.
fn routine_columns(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for r in seen.state.catalog.routines() {
        for (table, column) in seen.state.routine_usage_columns(r)? {
            rows.push([ids(&catalog, &r.specific).to_vec(), ids(&catalog, &table).to_vec(), vec![text(&column)]].concat());
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// [§6.37] ROUTINE_ROUTINE_USAGE, [§6.38] ROUTINE_SEQUENCE_USAGE
// ---------------------------------------------------------------------------

fn routine_routines_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("SPECIFIC", true).ids("ROUTINE", true).done()
}

/// [§6.37 Description 1 to 3] Each subject routine of each SQL routine's
/// body, by its specific name, which the draft's ROUTINE_ columns hold.
fn routine_routines(seen: &Seen<'_>) -> Vec<Row> {
    named_once(seen, &seen.state.catalog.routines(), |r| (r.specific.clone(), r.horizon.subjects().unwrap_or_default().to_vec()))
}

fn routine_sequences_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("SPECIFIC", true).ids("SEQUENCE", true).done()
}

/// [§6.38 Description 1] Each sequence generator each SQL-invoked
/// routine's body draws from.
fn routine_sequences(seen: &Seen<'_>) -> Vec<Row> {
    named_once(seen, &seen.state.catalog.routines(), |r| (r.specific.clone(), r.sequences.clone()))
}

// ---------------------------------------------------------------------------
// [§6.63] VIEW_ROUTINE_USAGE, [§6.13] CHECK_CONSTRAINT_ROUTINE_USAGE
// ---------------------------------------------------------------------------

fn view_routines_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).ids("SPECIFIC", true).done()
}

/// [§6.63 Description 1] Each subject routine of each view's query, by its
/// specific name.
fn view_routines(seen: &Seen<'_>) -> Vec<Row> {
    named_once(seen, &seen.state.catalog.views(), |v| (v.name.clone(), v.horizon.subjects().unwrap_or_default().to_vec()))
}

fn check_routines_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("CONSTRAINT", true).ids("SPECIFIC", true).done()
}

/// [§6.13 Description 1] Each subject routine of each assertion's
/// condition, then of each domain constraint's, then of each table check
/// constraint's, by its specific name. A copy of a domain constraint on a
/// column is the domain's (`db/domains.rs`), so not among the table's.
fn check_routines(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = &*seen.state.catalog;
    let domains = catalog.domains();
    let tables = catalog.tables();
    let domain_checks = domains.iter().flat_map(|d| d.constraints.iter());
    let table_checks = tables.iter().flat_map(|t| t.constraints.iter()).filter(|c| c.domain.is_none());
    let all: Vec<&ConstraintDesc> = catalog.assertions().into_iter().chain(domain_checks).chain(table_checks).collect();
    named_once(seen, &all, |c| (c.name.clone(), subjects(c)))
}

/// The subject routines of the condition of `c`, if it is a check
/// constraint.
fn subjects(c: &ConstraintDesc) -> Vec<QualifiedName> {
    match &c.kind {
        ConstraintKind::Check { horizon, .. } => horizon.subjects().unwrap_or_default().to_vec(),
        ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } | ConstraintKind::ForeignKey(_) | ConstraintKind::Connection(_) => {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// [§6.18] COLUMN_COLUMN_USAGE
// ---------------------------------------------------------------------------

fn column_columns_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).id("COLUMN_NAME", true).id("DEPENDENT_COLUMN", true).done()
}

/// [§6.18 Description 1, 2] Each column of each base table each generated
/// column of it depends on: the columns its generation expression
/// references, as it binds now.
///
/// # Errors
///
/// A generation expression does not bind.
fn column_columns(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.tables() {
        for (at, e) in generations(seen.state, t)? {
            let mut used = Vec::new();
            expr_columns(&e, &mut used);
            used.sort_unstable();
            used.dedup();
            let named = |k: usize| t.columns.get(k).map_or(Value::Null, |c| text(&c.name));
            for k in used {
                rows.push([ids(&catalog, &t.name).to_vec(), vec![named(k), named(at)]].concat());
            }
        }
    }
    Ok(rows)
}
