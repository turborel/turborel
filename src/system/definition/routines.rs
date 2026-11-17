//! [ISO/IEC 9075-2:2023 §4.35, §11.51 GR2, §11.60 GR3] The Definition
//! Schema's tables of SQL-invoked routines, as ISO/IEC 9075-11 names them.
//! ROUTINES has one row for each routine, PARAMETERS one for each of its
//! parameters, and ROUTINE_PRIVILEGES one for each privilege on one. The
//! routines are those each user-defined type's definition makes
//! (`definition/implicit.rs`), then each SQL-invoked routine a `<schema
//! routine>` or `<method specification>`'s body defines, in the order of
//! its creation. A method names its type, and its ROUTINE_TYPE is its
//! kind [ISO/IEC 9075-11:2006 draft §6.40 Description 4, 5]. A function
//! that is a user-defined cast's says so, and whether the cast is AS
//! ASSIGNMENT [Description 16, 17].
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
//!
//! ROUTINES and PARAMETERS end with the columns ISO/IEC 9075-14:2006
//! draft §21.2 and §21.3 append: the associated string type and XML option
//! of an external routine's parameter or result, and the XML passing
//! mechanism of an SQL routine's (`xml_columns`).

use std::sync::Arc;

use super::descriptors::{DESCRIBED, data_type, described, described_as};
use super::implicit::{FUNCTION, Implicit, implicit, method_type};
use super::privileges::PUBLIC;
use super::user_types::{RESULT_DTD, holders};
use super::{Columns, catalog_name, defined, ids, long, number, text};
use crate::ast::MethodKind;
use crate::catalog::{ColumnDesc, DataAccess, GenericParameter, QualifiedName, SqlRoutineDesc};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, yes};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-14:2006 draft §12.7 SR8b i, iii] The `<XML passing
/// mechanism>` implicit of a parameter or result of an XML type, or a
/// distinct type of an XML type, implementation-defined: BY REF, as CAST's
/// is, for no routine copies the nodes of a value passed to it.
const IMPLICIT_PASSING: &str = "BY REF";

/// How many of `xml_columns` come before the passing mechanism: the
/// associated string type's DTD_IDENTIFIER and the XML option.
const STRING_TYPE_COLUMNS: usize = 2;

/// How many of `xml_columns` come after it: the associated string type's
/// descriptor's.
const STRING_DESCRIBED: usize = 6;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("ROUTINES", routines_columns, Rows::Infallible(routines)),
        defined("PARAMETERS", parameters_columns, Rows::Infallible(parameters)),
        defined("ROUTINE_PRIVILEGES", routine_privileges_columns, Rows::Infallible(routine_privileges)),
    ]
}

/// The objects whose descriptors ELEMENT_TYPES, FIELDS and
/// REFERENCED_TYPES show those subordinate to, of the routines: each
/// parameter's type and each function's result type, as
/// `user_types::heads` gives them.
pub(super) fn heads(seen: &Seen<'_>) -> Vec<(QualifiedName, &'static str, String, DataType)> {
    let mut heads = Vec::new();
    for r in implicit(seen) {
        for (at, (_, t, _)) in r.parameters.iter().enumerate() {
            heads.push((r.specific.clone(), "ROUTINE", at.saturating_add(1).to_string(), t.clone()));
        }
        heads.push((r.specific.clone(), "ROUTINE", RESULT_DTD.to_owned(), r.result.clone()));
    }
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
pub(super) fn access(a: DataAccess) -> &'static str {
    match a {
        DataAccess::Contains => "CONTAINS SQL",
        DataAccess::Reads => "READS SQL DATA",
        DataAccess::Modifies => "MODIFIES SQL DATA",
    }
}

/// [ISO/IEC 9075-11:2006 draft §6.40 Description 16, 17] Of a function
/// that is a user-defined cast function, YES and whether it is implicitly
/// invocable; of another function, NO and null; of a procedure, null and
/// null.
fn cast_values(cast: Option<bool>, function: bool) -> [Value; 2] {
    match (cast, function) {
        (Some(assignment), _) => [yes(true), yes(assignment)],
        (None, true) => [yes(false), Value::Null],
        (None, false) => [Value::Null, Value::Null],
    }
}

/// [ISO/IEC 9075-2:2023 §4.10] Of the routine of specific name `specific`,
/// whether it is the function of a user-defined cast, and if so whether
/// the cast is AS ASSIGNMENT.
fn cast_of(seen: &Seen<'_>, specific: &QualifiedName) -> Option<bool> {
    seen.state.catalog.casts().into_iter().find(|c| c.function == *specific).map(|c| c.assignment)
}

/// The name of the type `udt`, or nulls of none.
fn udt_ids(catalog: &Value, udt: Option<&QualifiedName>) -> Vec<Value> {
    udt.map_or_else(|| vec![Value::Null; 3], |u| ids(catalog, u).to_vec())
}

// ---------------------------------------------------------------------------
// [ISO/IEC 9075-14:2006 draft §21.2, §21.3] XML parameters and results
// ---------------------------------------------------------------------------

/// Then XML_STRING_DTD_IDENTIFIER, XML_OPTION and `mechanism`,
/// XML_PASSING_MECHANISM or XML_RETURN_MECHANISM; then \[DESIGN\] the
/// columns of the descriptor of the associated string type the views show,
/// as each row holds those of its own type's, by the names the views give
/// them [§20.10, §20.11]. Its character set is that of its collation.
fn xml_columns(columns: Columns, mechanism: &str) -> Columns {
    let columns = columns.id("XML_STRING_DTD_IDENTIFIER", false).data("XML_OPTION", false).data(mechanism, false);
    let described = columns.data("XML_CHAR_TYPE", false).length("XML_CHAR_MAX_LEN", false).length("XML_CHAR_OCT_LEN", false);
    described.id("XML_CHAR_COLL_CAT", false).id("XML_CHAR_COLL_SCH", false).id("XML_CHAR_COLL_NAME", false)
}

/// The values of `xml_columns` of a parameter or result of type `t`. Only
/// an external routine's has an associated string type and XML option
/// [§12.7 SR1, SR3, SR4], so each is null. An SQL routine's of an XML
/// type, or a distinct type of one, is passed by its implicit mechanism
/// \[SR8b\], as none is written; another's by none [§21.2 Description 3].
fn xml_values(t: Option<&DataType>) -> Vec<Value> {
    let xml = t.is_some_and(|t| matches!(t.represented(), DataType::Xml(_)));
    let mechanism = if xml { text(IMPLICIT_PASSING) } else { Value::Null };
    [vec![Value::Null; STRING_TYPE_COLUMNS], vec![mechanism], vec![Value::Null; STRING_DESCRIBED]].concat()
}

// ---------------------------------------------------------------------------
// ROUTINES
// ---------------------------------------------------------------------------

fn routines_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("SPECIFIC", true)
        .ids("ROUTINE", true)
        .ids("MODULE", false)
        .ids("UDT", false)
        .data("ROUTINE_TYPE", true)
        .id("DTD_IDENTIFIER", false);
    let columns = described_as(columns, false)
        .data("ROUTINE_BODY", true)
        .data("ROUTINE_DEFINITION", false)
        .data("IS_DETERMINISTIC", true)
        .data("SQL_DATA_ACCESS", true)
        .data("IS_NULL_CALL", false)
        .data("SCHEMA_LEVEL_ROUTINE", true)
        .data("IS_USER_DEFINED_CAST", false)
        .data("IS_IMPLICITLY_INVOCABLE", false)
        .data("SECURITY_TYPE", true);
    xml_columns(columns, "XML_RETURN_MECHANISM").done()
}

/// Each routine a type's definition makes, a schema-level SQL routine,
/// with its result type's descriptor and the characteristics its rule
/// gives it or leaves implicit (`definition/implicit.rs`); then each
/// SQL-invoked routine.
fn routines(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows: Vec<Row> = implicit(seen).iter().map(|r| implicit_routine(&catalog, r)).collect();
    rows.extend(seen.state.catalog.routines().into_iter().map(|r| routine(seen, &catalog, r)));
    rows
}

/// The row of the routine `r` a type's definition makes.
fn implicit_routine(catalog: &Value, r: &Implicit<'_>) -> Row {
    let head = [
        ids(catalog, &r.specific).to_vec(),
        ids(catalog, &r.name).to_vec(),
        vec![Value::Null; 3],
        udt_ids(catalog, r.method_of),
        vec![text(&r.kind), text(RESULT_DTD)],
    ];
    let traits = vec![text("SQL"), Value::Null, yes(r.deterministic), text("CONTAINS SQL"), yes(r.null_call), yes(true)];
    let tail = [traits, cast_values(r.cast, true).to_vec(), vec![text("DEFINER")], xml_values(Some(&r.result))].concat();
    [head.concat(), data_type(catalog, &r.result, None), tail].concat()
}

/// [§11.60 GR3] The row of the SQL-invoked routine `r`: of a method, its
/// kind and type [ISO/IEC 9075-11:2006 draft §6.40 Description 4, 5].
fn routine(seen: &Seen<'_>, catalog: &Value, r: &SqlRoutineDesc) -> Row {
    let (kind, dtd, described, null_call) = match (&r.returns, r.function()) {
        (Some(t), _) => (FUNCTION.to_owned(), text(RESULT_DTD), data_type(catalog, t, None), yes(r.null_call)),
        // [DESIGN] A polymorphic table function returns a table of no one
        // data type: of no data type descriptor, as the 2006 draft has
        // none for it.
        (None, true) => (FUNCTION.to_owned(), Value::Null, vec![Value::Null; DESCRIBED], yes(r.null_call)),
        (None, false) => ("PROCEDURE".to_owned(), Value::Null, vec![Value::Null; DESCRIBED], Value::Null),
    };
    let kind = r.method.as_ref().map_or(kind, |(_, k)| method_type(*k));
    // [ISO/IEC 9075-4:2006 draft §19.5 Description 1] its module's name, if
    // it is of one.
    let module = r.module_name().map_or_else(|| vec![Value::Null; 3], |m| ids(catalog, &m).to_vec());
    let udt = udt_ids(catalog, r.method.as_ref().map(|(u, _)| u));
    let head = [ids(catalog, &r.specific).to_vec(), ids(catalog, &r.name).to_vec(), module, udt, vec![text(&kind), dtd]].concat();
    let level = yes(r.module.is_none());
    let traits = vec![text("SQL"), long(&r.body), yes(r.deterministic), text(access(r.access)), null_call, level];
    let cast = cast_values(cast_of(seen, &r.specific), r.returns.is_some() || r.function());
    [head, described, traits, cast.to_vec(), vec![text("DEFINER")], xml_values(r.returns.as_ref())].concat()
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
    xml_columns(described(columns).id("DTD_IDENTIFIER", true), "XML_PASSING_MECHANISM").done()
}

/// Each parameter of each routine a type's definition makes, an input
/// parameter; then [§11.60 GR3j] each parameter of each SQL-invoked
/// routine, with its mode, a method's SELF the RESULT parameter if the
/// method is SELF AS RESULT [§4.9.4].
fn parameters(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for r in implicit(seen) {
        for (at, (name, t, result)) in r.parameters.iter().enumerate() {
            let position = at.saturating_add(1);
            let head = vec![number(position), text("IN"), yes(*result), yes(false), text(name)];
            let dtd = vec![text(&position.to_string())];
            rows.push([ids(&catalog, &r.specific).to_vec(), head, data_type(&catalog, t, None), dtd, xml_values(Some(t))].concat());
        }
    }
    for r in seen.state.catalog.routines() {
        let preserving = self_as_result(seen, r);
        for (at, p) in r.parameters.iter().enumerate() {
            let position = at.saturating_add(1);
            let head = vec![number(position), text(p.mode.name()), yes(preserving && at == 0), yes(false), text(&p.name)];
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
            let xml = xml_values(p.generic.is_none().then_some(&p.data_type));
            rows.push([ids(&catalog, &r.specific).to_vec(), head, described, vec![dtd], xml].concat());
        }
    }
    rows
}

/// Whether `r` is an instance or constructor method whose specification
/// is SELF AS RESULT: a type-preserving function, of SELF its RESULT
/// parameter [ISO/IEC 9075-2:2023 §4.9.4, §11.60 SR6e].
fn self_as_result(seen: &Seen<'_>, r: &SqlRoutineDesc) -> bool {
    let Some((udt, kind)) = &r.method else { return false };
    let instance = match kind {
        MethodKind::Instance | MethodKind::Constructor => true,
        MethodKind::Static => false,
    };
    let spec = crate::catalog::method_host(&*seen.state.catalog, udt).and_then(|h| h.methods().iter().find(|m| m.specific == r.specific));
    instance && spec.is_some_and(|m| m.self_as_result)
}

// ---------------------------------------------------------------------------
// ROUTINE_PRIVILEGES
// ---------------------------------------------------------------------------

fn routine_privileges_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("SPECIFIC", true).ids("ROUTINE", true);
    columns.data("PRIVILEGE_TYPE", true).data("IS_GRANTABLE", true).done()
}

/// [§12.3 SR4d] Each privilege on each routine, EXECUTE: of a routine a
/// type's definition makes its owner's and each granted; of an SQL-invoked
/// routine each it holds, its owner's the one its definition granted
/// [§11.60 GR1].
fn routine_privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    let mut push = |object: &[Value], (grantor, grantee, grantable): (&str, &str, bool)| {
        rows.push([vec![text(grantor), text(grantee)], object.to_vec(), vec![text("EXECUTE"), yes(grantable)]].concat());
    };
    for r in implicit(seen) {
        let object = [ids(&catalog, &r.specific).to_vec(), ids(&catalog, &r.name).to_vec()].concat();
        for h in holders(seen, r.schema, r.execute) {
            push(&object, h);
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
