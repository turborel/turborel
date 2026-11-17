//! [ISO/IEC 9075-2:2023 §4.9.2, §4.9.3, §11.51 GR1, GR2] The Definition
//! Schema's tables of user-defined types and of the routines, casts and
//! transforms each distinct type's definition makes, as ISO/IEC 9075-11
//! names them. USER_DEFINED_TYPES has one row for each distinct type, then
//! one for each structured type, UDT_PRIVILEGES one for each privilege on
//! one, and TRANSFORMS two for each distinct type, its group's FROM SQL
//! and TO SQL functions, then one for each function of each transform
//! group a `<transform definition>` made [§11.67]. ROUTINES, PARAMETERS
//! and ROUTINE_PRIVILEGES show the routines each definition makes
//! (`definition/implicit.rs`). A type's ordering is the one the catalog
//! holds [§4.13], and of a distinct type of none the one GR2c gives.
//!
//! ISO/IEC 9075:1992 has none of these, so §21.3 defines no table of them,
//! and each row holds the columns of the data type descriptor it names, as
//! a row of ELEMENT_TYPES does (`definition/descriptors.rs`). A distinct
//! type's source type is of DTD_IDENTIFIER `SOURCE_DTD`, and the
//! representation type of a structured type's user-defined reference type
//! of `REF_DTD`. A function's parameter is of its ordinal position, and its
//! result of `RESULT_DTD`. The descriptors subordinate to these are rows of
//! ELEMENT_TYPES and FIELDS, of the object types USER-DEFINED TYPE and
//! ROUTINE (`heads`).
//!
//! GR2b defines each function as LANGUAGE SQL and DETERMINISTIC. Its other
//! characteristics are those its `<SQL-invoked routine>` leaves implicit,
//! CONTAINS SQL, CALLED ON NULL INPUT and SQL SECURITY DEFINER [§11.60 SR].
//! GR2b leaves each parameter's name arbitrary, so it is the name GR2b
//! writes.

use std::sync::Arc;

use super::descriptors::{DESCRIBED, data_type, described_as};
use super::privileges::PUBLIC;
use super::{Columns, catalog_name, defined, ids, text};
use crate::ast::{OrderingForm, TransformKind};
use crate::catalog::{
    ColumnDesc, OrderingDesc, QualifiedName, RefRepresentation, Routine, StructuredDesc, UsageDesc, UserTypeDesc, owner_of, sole,
};
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable, yes};
use crate::value::{DataType, Value};

/// The DTD_IDENTIFIER of a distinct type's source type.
pub(super) const SOURCE_DTD: &str = "1";

/// The DTD_IDENTIFIER of a function's result type.
pub(super) const RESULT_DTD: &str = "0";

/// The DTD_IDENTIFIER of the representation type of a structured type's
/// user-defined reference type: none of its attributes', which are their
/// ordinal positions (`definition/structured.rs`).
pub(super) const REF_DTD: &str = "0";

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
/// subordinate to: each distinct type's source type. Each is the object's
/// name, its OBJECT_TYPE, the descriptor's DTD_IDENTIFIER, and its data
/// type. Its functions' are among the routines' (`routines::heads`).
pub(super) fn heads(seen: &Seen<'_>) -> Vec<(QualifiedName, &'static str, String, DataType)> {
    let types = seen.state.catalog.user_types().into_iter();
    types.map(|t| (t.name.clone(), "USER-DEFINED TYPE", SOURCE_DTD.to_owned(), t.source.clone())).collect()
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
    // A structured type of no user-defined representation names no data
    // type descriptor.
    described_as(columns, false).done()
}

/// [§11.51 GR1, GR2c] Each distinct type: instantiable and final, of its
/// ordering, and its source type's descriptor; then each structured type.
fn user_types(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |t: &UserTypeDesc| {
        let head = vec![text("DISTINCT"), yes(true), yes(true)];
        let reference = vec![Value::Null, text(SOURCE_DTD), Value::Null];
        let ordering = distinct_ordering(seen, &catalog, t);
        [ids(&catalog, &t.name).to_vec(), head, ordering, reference, data_type(&catalog, &t.source, None)].concat()
    };
    let mut rows: Vec<Row> = seen.state.catalog.user_types().into_iter().map(row).collect();
    rows.extend(seen.state.catalog.structured_types().into_iter().map(|s| structured_type(seen, &catalog, s)));
    rows
}

/// [§11.51 GR2c] A distinct type's ordering: the catalog's, else FULL BY
/// MAP with FNSDT, of its specific name, unless its source type is a
/// collection type, which leaves it none.
fn distinct_ordering(seen: &Seen<'_>, catalog: &Value, t: &UserTypeDesc) -> Vec<Value> {
    if let Some(o) = seen.state.catalog.ordering(&t.name) {
        return ordering(catalog, Some(o));
    }
    if matches!(t.source, DataType::Array(_) | DataType::Multiset(_)) {
        return ordering(catalog, None);
    }
    let routine = ids(catalog, &specific((t, Routine::ToSource))).to_vec();
    [vec![text("FULL"), text("MAP")], routine].concat()
}

/// [ISO/IEC 9075-11:2006 draft §6.61 Description 6 to 8] ORDERING_FORM,
/// ORDERING_CATEGORY and the specific name of the ordering function of
/// `o`; of none, NONE and nulls.
fn ordering(catalog: &Value, o: Option<&OrderingDesc>) -> Vec<Value> {
    let Some(o) = o else { return vec![text("NONE"), Value::Null, Value::Null, Value::Null, Value::Null] };
    let form = text(match o.form {
        OrderingForm::Equals => "EQUALS",
        OrderingForm::Full => "FULL",
    });
    let routine = o.function().map_or_else(|| vec![Value::Null; 3], |f| ids(catalog, f).to_vec());
    [vec![form, text(o.category_name())], routine].concat()
}

/// [ISO/IEC 9075-11:2006 draft §6.61 Description 2 to 10] A structured
/// type: STRUCTURED, instantiable or not, final or not, of its ordering,
/// of no source type, and how its reference type's values are made; of a
/// user-defined representation, its representation type's descriptor.
fn structured_type(seen: &Seen<'_>, catalog: &Value, s: &StructuredDesc) -> Row {
    let head = vec![text("STRUCTURED"), yes(s.instantiable), yes(s.is_final)];
    // [Description 9] The draft's names: a user-defined representation's
    // values are USER GENERATED.
    let (kind, represented) = match &s.reference {
        RefRepresentation::System => ("SYSTEM GENERATED", None),
        RefRepresentation::User(t) => ("USER GENERATED", Some(t)),
        RefRepresentation::Derived(_) => ("DERIVED", None),
    };
    let reference = vec![text(kind), Value::Null, represented.map_or(Value::Null, |_| text(REF_DTD))];
    let described = represented.map_or_else(|| vec![Value::Null; DESCRIBED], |t| data_type(catalog, t, None));
    let ordered = ordering(catalog, seen.state.catalog.ordering(&s.name));
    [ids(catalog, &s.name).to_vec(), head, ordered, reference, described].concat()
}

fn udt_privileges_columns() -> Vec<ColumnDesc> {
    Columns::of().id("GRANTOR", true).id("GRANTEE", true).ids("UDT", true).data("PRIVILEGE_TYPE", true).data("IS_GRANTABLE", true).done()
}

/// [§12.3 SR4a] Each privilege on each distinct type, TYPE USAGE, its
/// owner's and each granted; then on each structured type, TYPE USAGE
/// likewise, and UNDER [§11.51 GR5]. \[EXTENSION\] UNDER is shown as
/// TYPE USAGE is, though the 2006 draft's check names TYPE USAGE alone.
fn udt_privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    let mut push = |name: &QualifiedName, privilege: &str, (grantor, grantee, grantable): (&str, &str, bool)| {
        let tail = vec![text(privilege), yes(grantable)];
        rows.push([vec![text(grantor), text(grantee)], ids(&catalog, name).to_vec(), tail].concat());
    };
    for t in seen.state.catalog.user_types() {
        for h in holders(seen, &t.name.auth, &t.usage) {
            push(&t.name, "TYPE USAGE", h);
        }
    }
    for s in seen.state.catalog.structured_types() {
        for h in holders(seen, &s.name.auth, &s.usage) {
            push(&s.name, "TYPE USAGE", h);
        }
        let owner = owner_of(&*seen.state.catalog, &s.name.auth);
        push(&s.name, "UNDER", (SYSTEM_GRANTOR, owner, under_grantable(seen, s)));
        for u in &s.under {
            push(&s.name, "UNDER", (&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC), u.grantable));
        }
    }
    rows
}

/// [§11.51 GR5] Whether the owner's UNDER on `s` is grantable: if its
/// owner holds UNDER on its direct supertype WITH GRANT OPTION, or it has
/// none.
fn under_grantable(seen: &Seen<'_>, s: &StructuredDesc) -> bool {
    let catalog = &*seen.state.catalog;
    let Some(supertype) = s.supertype.as_ref() else { return true };
    let owner = owner_of(catalog, &s.name.auth);
    catalog.structured_type(supertype).is_some_and(|t| t.under_of(sole(catalog, owner), owner_of(catalog, &supertype.auth)).1)
}

// ---------------------------------------------------------------------------
// TRANSFORMS
// ---------------------------------------------------------------------------

fn transforms_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("UDT", true).ids("SPECIFIC", true).id("GROUP_NAME", true).data("TRANSFORM_TYPE", true).done()
}

/// [§11.51 GR2b] Each distinct type's transform group, named FNUDT, which
/// is FROM SQL with FNSDT and TO SQL with FNUDT; then each function of
/// each transform group the catalog holds [§11.67 GR1].
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
    for (udt, groups) in seen.state.catalog.all_transforms() {
        // A distinct type's own group is shown above.
        let implied = seen.state.catalog.user_type(udt).map(|t| t.to_distinct.name.as_str());
        for g in groups.iter().filter(|g| Some(g.name.as_str()) != implied) {
            for (kind, name) in [(TransformKind::FromSql, "FROM SQL"), (TransformKind::ToSql, "TO SQL")] {
                if let Some(f) = g.function(kind) {
                    let head = [ids(&catalog, udt).to_vec(), ids(&catalog, f).to_vec()].concat();
                    rows.push([head, vec![text(&g.name), text(name)]].concat());
                }
            }
        }
    }
    rows
}
