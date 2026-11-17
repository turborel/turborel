//! [ISO/IEC 9075-2:2023 §4.9.2, §11.51 GR1, GR2] The Definition Schema's
//! tables of user-defined types and of the routines, casts and transforms
//! each distinct type's definition makes, as ISO/IEC 9075-11 names them.
//! USER_DEFINED_TYPES has one row for each distinct type, UDT_PRIVILEGES
//! one for each privilege on one, and TRANSFORMS two for each type, its
//! group's FROM SQL and TO SQL functions. ROUTINES, PARAMETERS and
//! ROUTINE_PRIVILEGES show its two functions (`definition/routines.rs`).
//!
//! ISO/IEC 9075:1992 has none of these, so §21.3 defines no table of them,
//! and each row holds the columns of the data type descriptor it names, as
//! a row of ELEMENT_TYPES does (`definition/descriptors.rs`). A type's
//! source type is of DTD_IDENTIFIER `SOURCE_DTD`. A function's parameter is
//! of its ordinal position, and its result of `RESULT_DTD`. The descriptors
//! subordinate to these are rows of ELEMENT_TYPES and FIELDS, of the object
//! types USER-DEFINED TYPE and ROUTINE (`heads`).
//!
//! GR2b defines each function as LANGUAGE SQL and DETERMINISTIC. Its other
//! characteristics are those its `<SQL-invoked routine>` leaves implicit,
//! CONTAINS SQL, CALLED ON NULL INPUT and SQL SECURITY DEFINER [§11.60 SR].
//! GR2b leaves each parameter's name arbitrary, so it is the name GR2b
//! writes.

use std::sync::Arc;

use super::descriptors::{data_type, described};
use super::privileges::PUBLIC;
use super::{Columns, catalog_name, defined, ids, text};
use crate::catalog::{ColumnDesc, QualifiedName, Routine, UsageDesc, UserTypeDesc, owner_of};
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable, yes};
use crate::value::{DataType, Value};

/// The DTD_IDENTIFIER of a distinct type's source type.
pub(super) const SOURCE_DTD: &str = "1";

/// The DTD_IDENTIFIER of a function's result type.
pub(super) const RESULT_DTD: &str = "0";

/// The DTD_IDENTIFIER of a function's one parameter's type.
const PARAMETER_DTD: &str = "1";

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("USER_DEFINED_TYPES", user_types_columns, Rows::Infallible(user_types)),
        defined("UDT_PRIVILEGES", udt_privileges_columns, Rows::Infallible(udt_privileges)),
        defined("TRANSFORMS", transforms_columns, Rows::Infallible(transforms)),
    ]
}

/// A function a distinct type's definition makes: its type, and which.
pub(super) type Function<'a> = (&'a UserTypeDesc, Routine);

/// Each distinct type's two functions.
pub(super) fn functions(t: &UserTypeDesc) -> [Function<'_>; 2] {
    [(t, Routine::ToDistinct), (t, Routine::ToSource)]
}

/// The specific name of `f`.
pub(super) fn specific((t, which): Function<'_>) -> QualifiedName {
    QualifiedName { auth: t.name.auth.clone(), ident: t.routine(which).specific.clone() }
}

/// The parameter's name and type, and the result type, of `f` [GR2b].
pub(super) fn signature((t, which): Function<'_>) -> (&'static str, DataType, DataType) {
    match which {
        Routine::ToDistinct => ("SDTP", t.source.clone(), t.data_type()),
        Routine::ToSource => ("UDTP", t.data_type(), t.source.clone()),
    }
}

/// The objects whose descriptors ELEMENT_TYPES and FIELDS show those
/// subordinate to: each type's source type, and each function's parameter
/// and result types. Each is the object's name, its OBJECT_TYPE, the
/// descriptor's DTD_IDENTIFIER, and its data type.
pub(super) fn heads(seen: &Seen<'_>) -> Vec<(QualifiedName, &'static str, String, DataType)> {
    let mut heads = Vec::new();
    for t in seen.state.catalog.user_types() {
        heads.push((t.name.clone(), "USER-DEFINED TYPE", SOURCE_DTD.to_owned(), t.source.clone()));
        for f in functions(t) {
            let (_, parameter, result) = signature(f);
            heads.push((specific(f), "ROUTINE", PARAMETER_DTD.to_owned(), parameter));
            heads.push((specific(f), "ROUTINE", RESULT_DTD.to_owned(), result));
        }
    }
    heads
}

/// The grantor, grantee and grantability of each privilege of `granted`
/// on an object of schema `schema`: its owner's, of "_SYSTEM", grantable,
/// then each granted [§11.51 GR2, §12.2].
pub(super) fn holders<'a>(seen: &'a Seen<'_>, schema: &'a str, granted: &'a [UsageDesc]) -> Vec<(&'a str, &'a str, bool)> {
    let owner = owner_of(&*seen.state.catalog, schema);
    let mut all = vec![(SYSTEM_GRANTOR, owner, true)];
    all.extend(granted.iter().map(|u| (u.grantor.as_str(), u.grantee.as_deref().unwrap_or(PUBLIC), u.grantable)));
    all
}

// ---------------------------------------------------------------------------
// USER_DEFINED_TYPES, UDT_PRIVILEGES
// ---------------------------------------------------------------------------

fn user_types_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("USER_DEFINED_TYPE", true)
        .data("USER_DEFINED_TYPE_CATEGORY", true)
        .data("IS_INSTANTIABLE", true)
        .data("IS_FINAL", true)
        .data("ORDERING_FORM", true)
        .data("ORDERING_CATEGORY", false)
        .ids("ORDERING_ROUTINE", false)
        .data("REFERENCE_TYPE", false)
        .id("SOURCE_DTD_IDENTIFIER", false)
        .id("REF_DTD_IDENTIFIER", false);
    described(columns).done()
}

/// [§11.51 GR1, GR2c] Each distinct type: instantiable and final, ordered
/// FULL BY MAP with FNSDT, of its specific name, unless its source type is
/// a collection type; and its source type's descriptor.
fn user_types(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |t: &UserTypeDesc| {
        let ordered = !matches!(t.source, DataType::Array(_) | DataType::Multiset(_));
        let routine = if ordered { ids(&catalog, &specific((t, Routine::ToSource))).to_vec() } else { vec![Value::Null; 3] };
        let ordering = if ordered { vec![text("FULL"), text("MAP")] } else { vec![text("NONE"), Value::Null] };
        let head = vec![text("DISTINCT"), yes(true), yes(true)];
        let reference = vec![Value::Null, text(SOURCE_DTD), Value::Null];
        [ids(&catalog, &t.name).to_vec(), head, ordering, routine, reference, data_type(&catalog, &t.source, None)].concat()
    };
    seen.state.catalog.user_types().into_iter().map(row).collect()
}

fn udt_privileges_columns() -> Vec<ColumnDesc> {
    Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("UDT", true).data("PRIVILEGE_TYPE", true).data("IS_GRANTABLE", true).done()
}

/// [§12.3 SR4a] Each privilege on each distinct type, TYPE USAGE, its
/// owner's and each granted.
fn udt_privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.user_types() {
        for (grantor, grantee, grantable) in holders(seen, &t.name.auth, &t.usage) {
            let privilege = vec![text("TYPE USAGE"), yes(grantable)];
            rows.push([vec![text(grantor), text(grantee)], ids(&catalog, &t.name).to_vec(), privilege].concat());
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// TRANSFORMS
// ---------------------------------------------------------------------------

fn transforms_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("UDT", true).ids("SPECIFIC", true).id("GROUP_NAME", true).data("TRANSFORM_TYPE", true).done()
}

/// [§11.51 GR2b] Each type's transform group, named FNUDT, which is FROM
/// SQL with FNSDT and TO SQL with FNUDT.
fn transforms(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.user_types() {
        let group = text(&t.to_distinct.name);
        for (which, kind) in [(Routine::ToSource, "FROM SQL"), (Routine::ToDistinct, "TO SQL")] {
            let head = [ids(&catalog, &t.name).to_vec(), ids(&catalog, &specific((t, which))).to_vec()].concat();
            rows.push([head, vec![group.clone(), text(kind)]].concat());
        }
    }
    rows
}
