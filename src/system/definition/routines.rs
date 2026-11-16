//! [ISO/IEC 9075-2:2023 §4.35, §11.51 GR2, §11.60 GR3] The Definition
//! Schema's tables of SQL-invoked routines, as ISO/IEC 9075-11 names them.
//! ROUTINES has one row for each routine, PARAMETERS one for each of its
//! parameters, and ROUTINE_PRIVILEGES one for each privilege on one. The
//! routines are the two functions of each distinct type
//! (`definition/user_types.rs`), then each SQL-invoked routine a `<schema
//! routine>` defines, in the order of its creation.
//!
//! Each row holds the columns of the data type descriptor it names, as a
//! row of ELEMENT_TYPES does (`definition/descriptors.rs`). A parameter's
//! DTD_IDENTIFIER is its ordinal position, and a function's result type's
//! is `RESULT_DTD`. A procedure has no result type, so its row's
//! descriptor columns, its DTD_IDENTIFIER and its IS_NULL_CALL are null
//! [§11.60 SR9f i]. Each SQL-invoked routine is of SQL SECURITY DEFINER
//! [§11.60 SR23b], and its ROUTINE_DEFINITION is its `<SQL routine body>`
//! as written, null when longer than CHARACTER_DATA holds. A routine of an
//! SQL-server module names it, and is no schema-level routine [ISO/IEC
//! 9075-4:2006 draft §19.5].

use std::sync::Arc;

use super::descriptors::{data_type, described, described_as};
use super::privileges::PUBLIC;
use super::user_types::{RESULT_DTD, functions, holders, signature, specific};
use super::{Columns, catalog_name, defined, ids, long, number, text};
use crate::catalog::{ColumnDesc, DataAccess, GenericParameter, QualifiedName, SqlRoutineDesc};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, yes};
use crate::value::{DataType, Value};

/// The ordinal position of a distinct type's function's one parameter,
/// and the DTD_IDENTIFIER of its type.
const PARAMETER_DTD: &str = "1";
const PARAMETER_POSITION: usize = 1;

/// How many columns a data type descriptor's are, from DATA_TYPE on.
const DESCRIBED: usize = 14;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("ROUTINES", routines_columns, Rows::Infallible(routines)),
        defined("PARAMETERS", parameters_columns, Rows::Infallible(parameters)),
        defined("ROUTINE_PRIVILEGES", routine_privileges_columns, Rows::Infallible(routine_privileges)),
    ]
}

/// The objects whose descriptors ELEMENT_TYPES and FIELDS show those
/// subordinate to, of the SQL-invoked routines: each parameter's type and
/// each function's result type, as `user_types::heads` gives them.
pub(super) fn heads(seen: &Seen<'_>) -> Vec<(QualifiedName, &'static str, String, DataType)> {
    let mut heads = Vec::new();
    for r in seen.state.catalog.routines() {
        // A generic table or descriptor parameter is of no data type.
        for (at, p) in r.parameters.iter().enumerate().filter(|(_, p)| p.generic.is_none()) {
            heads.push((r.specific.clone(), "ROUTINE", at.saturating_add(1).to_string(), p.data_type.clone()));
        }
        if let Some(t) = &r.returns {
            heads.push((r.specific.clone(), "ROUTINE", RESULT_DTD.to_owned(), t.clone()));
        }
    }
    heads
}

/// The name of an SQL routine body's SQL-data access indication.
fn access(a: DataAccess) -> &'static str {
    match a {
        DataAccess::Contains => "CONTAINS SQL",
        DataAccess::Reads => "READS SQL DATA",
        DataAccess::Modifies => "MODIFIES SQL DATA",
    }
}

// ---------------------------------------------------------------------------
// ROUTINES
// ---------------------------------------------------------------------------

fn routines_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("SPECIFIC", true)
        .ids("ROUTINE", true)
        .ids("MODULE", false)
        .data("ROUTINE_TYPE", true)
        .id("DTD_IDENTIFIER", false);
    described_as(columns, false)
        .data("ROUTINE_BODY", true)
        .data("ROUTINE_DEFINITION", false)
        .data("IS_DETERMINISTIC", true)
        .data("SQL_DATA_ACCESS", true)
        .data("IS_NULL_CALL", false)
        .data("SCHEMA_LEVEL_ROUTINE", true)
        .data("SECURITY_TYPE", true)
        .done()
}

/// [§11.51 GR2b] Each distinct type's function, a schema-level SQL
/// function, with its result type's descriptor and the characteristics
/// GR2b gives it or leaves implicit; then each SQL-invoked routine.
fn routines(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.user_types() {
        for f in functions(t) {
            let (_, _, result) = signature(f);
            let named = QualifiedName { auth: t.name.auth.clone(), ident: t.routine(f.1).name.clone() };
            let head = [
                ids(&catalog, &specific(f)).to_vec(),
                ids(&catalog, &named).to_vec(),
                vec![Value::Null, Value::Null, Value::Null, text("FUNCTION"), text(RESULT_DTD)],
            ];
            let tail = vec![text("SQL"), Value::Null, yes(true), text("CONTAINS SQL"), yes(false), yes(true), text("DEFINER")];
            rows.push([head.concat(), data_type(&catalog, &result, None), tail].concat());
        }
    }
    rows.extend(seen.state.catalog.routines().into_iter().map(|r| routine(&catalog, r)));
    rows
}

/// [§11.60 GR3] The row of the SQL-invoked routine `r`.
fn routine(catalog: &Value, r: &SqlRoutineDesc) -> Row {
    let (kind, dtd, described, null_call) = match (&r.returns, r.function()) {
        (Some(t), _) => ("FUNCTION", text(RESULT_DTD), data_type(catalog, t, None), yes(r.null_call)),
        // [DESIGN] A polymorphic table function returns a table of no one
        // data type: of no data type descriptor, as the 2006 draft has
        // none for it.
        (None, true) => ("FUNCTION", Value::Null, vec![Value::Null; DESCRIBED], yes(r.null_call)),
        (None, false) => ("PROCEDURE", Value::Null, vec![Value::Null; DESCRIBED], Value::Null),
    };
    // [ISO/IEC 9075-4:2006 draft §19.5 Description 1] its module's name, if
    // it is of one.
    let module = r.module_name().map_or_else(|| vec![Value::Null; 3], |m| ids(catalog, &m).to_vec());
    let head = [ids(catalog, &r.specific).to_vec(), ids(catalog, &r.name).to_vec(), module, vec![text(kind), dtd]].concat();
    let level = yes(r.module.is_none());
    let tail = vec![text("SQL"), long(&r.body), yes(r.deterministic), text(access(r.access)), null_call, level, text("DEFINER")];
    [head, described, tail].concat()
}

// ---------------------------------------------------------------------------
// PARAMETERS
// ---------------------------------------------------------------------------

fn parameters_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("SPECIFIC", true)
        .number("ORDINAL_POSITION", true)
        .data("PARAMETER_MODE", true)
        .data("IS_RESULT", true)
        .data("AS_LOCATOR", true)
        .id("PARAMETER_NAME", false);
    described(columns).id("DTD_IDENTIFIER", true).done()
}

/// [§11.51 GR2b] Each distinct type's function's one parameter, an input
/// parameter; then [§11.60 GR3j] each parameter of each SQL-invoked
/// routine, with its mode.
fn parameters(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.user_types() {
        for f in functions(t) {
            let (name, parameter, _) = signature(f);
            let head = vec![number(PARAMETER_POSITION), text("IN"), yes(false), yes(false), text(name)];
            let described = data_type(&catalog, &parameter, None);
            rows.push([ids(&catalog, &specific(f)).to_vec(), head, described, vec![text(PARAMETER_DTD)]].concat());
        }
    }
    for r in seen.state.catalog.routines() {
        for (at, p) in r.parameters.iter().enumerate() {
            let position = at.saturating_add(1);
            let head = vec![number(position), text(p.mode.name()), yes(false), yes(false), text(&p.name)];
            let dtd = text(&position.to_string());
            let described = match p.generic {
                None => data_type(&catalog, &p.data_type, None),
                // [EXTENSION] A generic table or descriptor parameter is of
                // no data type: DATA_TYPE the key word of its <parameter
                // type>, TABLE or DESCRIPTOR, as the 2006 draft has none.
                Some(g) => {
                    let mut none = vec![Value::Null; DESCRIBED];
                    if let Some(first) = none.first_mut() {
                        *first = text(match g {
                            GenericParameter::Table(_) => "TABLE",
                            GenericParameter::Descriptor => "DESCRIPTOR",
                        });
                    }
                    none
                }
            };
            rows.push([ids(&catalog, &r.specific).to_vec(), head, described, vec![dtd]].concat());
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// ROUTINE_PRIVILEGES
// ---------------------------------------------------------------------------

fn routine_privileges_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("SPECIFIC", true).ids("ROUTINE", true);
    columns.data("PRIVILEGE_TYPE", true).data("IS_GRANTABLE", true).done()
}

/// [§12.3 SR4d] Each privilege on each routine, EXECUTE: of a distinct
/// type's function its owner's and each granted; of an SQL-invoked
/// routine each it holds, its owner's the one its definition granted
/// [§11.60 GR1].
fn routine_privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    let mut push = |object: &[Value], (grantor, grantee, grantable): (&str, &str, bool)| {
        rows.push([vec![text(grantor), text(grantee)], object.to_vec(), vec![text("EXECUTE"), yes(grantable)]].concat());
    };
    for t in seen.state.catalog.user_types() {
        for f in functions(t) {
            let r = t.routine(f.1);
            let named = QualifiedName { auth: t.name.auth.clone(), ident: r.name.clone() };
            let object = [ids(&catalog, &specific(f)).to_vec(), ids(&catalog, &named).to_vec()].concat();
            for h in holders(seen, &t.name.auth, &r.execute) {
                push(&object, h);
            }
        }
    }
    for r in seen.state.catalog.routines() {
        let object = [ids(&catalog, &r.specific).to_vec(), ids(&catalog, &r.name).to_vec()].concat();
        for u in &r.execute {
            push(&object, (&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC), u.grantable));
        }
    }
    rows
}
