//! [ISO/IEC 9075-2:2023 §4.9.3, §4.9.4, §11.51, §11.52] The Definition
//! Schema's tables of structured types, as ISO/IEC 9075-11:2006 draft
//! §6.6, §6.23, §6.29 and §6.30 define them. ATTRIBUTES has one row for
//! each attribute of each structured type, inherited ones first, in their
//! order [§4.9.3.1]; DIRECT_SUPERTYPES one for each type of a direct
//! supertype; METHOD_SPECIFICATIONS one for each method specification a
//! type's definition holds, the observer and mutator of each attribute it
//! defines first [§11.51 SR10g], then its own; and
//! METHOD_SPECIFICATION_PARAMETERS one for each parameter of each, its
//! unaugmented list, SELF not among them.
//!
//! Each row holds the columns of the data type descriptor it names, as a
//! row of ELEMENT_TYPES does (`definition/descriptors.rs`). Of the object
//! type USER-DEFINED TYPE, an attribute's DTD_IDENTIFIER is its ordinal
//! position and a method specification's result type's is `METHOD_DTD`
//! then its position among its type's specifications. A parameter's is
//! its ordinal position, of the object the method specification's
//! specific name, as the draft keys them [§6.29 Description 5].
//!
//! A method specification is of LANGUAGE SQL, so its PARAMETER_STYLE, its
//! IS_DETERMINISTIC and its to-sql and from-sql routines are null [§6.30
//! Description 10a], and no type of it is passed AS LOCATOR.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::descriptors::{data_type, described};
use super::user_types::REF_DTD;
use super::{Columns, catalog_name, defined, ids, number, text};
use crate::ast::MethodKind;
use crate::catalog::{ColumnDesc, DataAccess, MethodHost, QualifiedName, RefRepresentation};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, yes};
use crate::value::{DataType, Value};

/// The prefix of the DTD_IDENTIFIER of a method specification's result
/// type, before its position.
const METHOD_DTD: &str = "M";

/// [ISO/IEC 9075-11:2006 draft §6.6 Description 5] The ATTRIBUTE_DEFAULT
/// of a default too long to show.
const TRUNCATED: &str = "TRUNCATED";

/// The name §11.51 SR10g gives a mutator's parameter.
pub(super) const ATTR: &str = "ATTR";

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("ATTRIBUTES", attributes_columns, Rows::Infallible(attributes)),
        defined("DIRECT_SUPERTYPES", supertypes_columns, Rows::Infallible(supertypes)),
        defined("METHOD_SPECIFICATIONS", methods_columns, Rows::Infallible(methods)),
        defined("METHOD_SPECIFICATION_PARAMETERS", method_parameters_columns, Rows::Infallible(method_parameters)),
    ]
}

/// A method specification of a structured type.
pub(super) struct Spec<'a> {
    /// The `<method name>`.
    pub(super) name: &'a str,
    /// Its specific name.
    pub(super) specific: QualifiedName,
    /// INSTANCE, STATIC or CONSTRUCTOR.
    pub(super) kind: MethodKind,
    /// Whether it is an overriding method specification.
    pub(super) overriding: bool,
    /// Its unaugmented parameters, each its name, if it has one, and type.
    pub(super) parameters: Vec<(Option<&'a str>, DataType)>,
    /// Its result type.
    pub(super) returns: DataType,
    /// Its SQL-data access indication.
    pub(super) access: DataAccess,
    /// RETURNS NULL ON NULL INPUT.
    pub(super) null_call: bool,
}

/// [§11.51 SR10g] The method specifications of `s`: the observer and the
/// mutator of each attribute it defines, which take no parameter and one,
/// ATTR; then its own, each it holds once.
pub(super) fn specs(host: MethodHost<'_>) -> Vec<Spec<'_>> {
    let named = |ident: &str| QualifiedName { auth: host.name().auth.clone(), ident: ident.to_owned() };
    let mut all = Vec::new();
    let own = host.structured().into_iter().flat_map(|s| s.attributes.iter().filter(|a| a.defined_by == s.name));
    for a in own {
        let implicit = |specific: &str, parameters, returns, null_call| Spec {
            name: &a.name,
            specific: named(specific),
            kind: MethodKind::Instance,
            overriding: false,
            parameters,
            returns,
            access: DataAccess::Contains,
            null_call,
        };
        all.push(implicit(&a.observer.specific, Vec::new(), a.data_type.clone(), true));
        all.push(implicit(&a.mutator.specific, vec![(Some(ATTR), a.data_type.clone())], host.data_type(), false));
    }
    let implicit: Vec<QualifiedName> = all.iter().map(|o: &Spec<'_>| o.specific.clone()).collect();
    for m in host.methods().iter().filter(|m| !implicit.contains(&m.specific)) {
        let parameters = m.parameters.iter().map(|p| ((!p.name.is_empty()).then_some(p.name.as_str()), p.data_type.clone())).collect();
        all.push(Spec {
            name: &m.name,
            specific: m.specific.clone(),
            kind: m.kind,
            overriding: m.overrides.is_some(),
            parameters,
            returns: m.returns.clone(),
            access: m.access,
            null_call: m.null_call,
        });
    }
    all
}

/// The objects whose descriptors ELEMENT_TYPES, FIELDS and
/// REFERENCED_TYPES show those subordinate to, of the structured types:
/// each attribute's type, a user-defined reference type's representation
/// type, and each method specification's result type and parameters'
/// types, as `user_types::heads` gives them.
pub(super) fn heads(seen: &Seen<'_>) -> Vec<(QualifiedName, &'static str, String, DataType)> {
    const UDT: &str = "USER-DEFINED TYPE";
    let mut heads = Vec::new();
    for s in seen.state.catalog.structured_types() {
        for (at, a) in s.attributes.iter().enumerate() {
            heads.push((s.name.clone(), UDT, at.saturating_add(1).to_string(), a.data_type.clone()));
        }
        if let RefRepresentation::User(t) = &s.reference {
            heads.push((s.name.clone(), UDT, REF_DTD.to_owned(), t.clone()));
        }
        method_heads(MethodHost::Structured(s), &mut heads);
    }
    for d in seen.state.catalog.user_types() {
        method_heads(MethodHost::Distinct(d), &mut heads);
    }
    heads
}

/// The DTD_IDENTIFIER of the result type of the method specification at
/// `at` among its type's, from 0.
fn method_dtd(at: usize) -> String {
    format!("{METHOD_DTD}{}", at.saturating_add(1))
}

// ---------------------------------------------------------------------------
// [§6.6] ATTRIBUTES, [§6.23] DIRECT_SUPERTYPES
// ---------------------------------------------------------------------------

fn attributes_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("UDT", true)
        .id("ATTRIBUTE_NAME", true)
        .number("ORDINAL_POSITION", true)
        .id("DTD_IDENTIFIER", true)
        .data("ATTRIBUTE_DEFAULT", false)
        .data("IS_DERIVED_REFERENCE_ATTRIBUTE", true);
    described(columns).done()
}

/// [§6.6 Description 1 to 6] Each attribute of each structured type, at
/// its ordinal position, from 1; its default as written, TRUNCATED if
/// longer than CHARACTER_DATA holds; whether a derived representation of
/// its type's reference type names it; and its type's descriptor, of its
/// collation.
fn attributes(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for s in seen.state.catalog.structured_types() {
        for (at, a) in s.attributes.iter().enumerate() {
            let position = at.saturating_add(1);
            let default = a.default.as_deref().map_or(Value::Null, |d| {
                let shown = super::long(d);
                if shown.is_null() { text(TRUNCATED) } else { shown }
            });
            let derived = if let RefRepresentation::Derived(names) = &s.reference { names.contains(&a.name) } else { false };
            let own = vec![text(&a.name), number(position), text(&position.to_string()), default, yes(derived)];
            rows.push([ids(&catalog, &s.name).to_vec(), own, data_type(&catalog, &a.data_type, a.collation.as_ref())].concat());
        }
    }
    rows
}

fn supertypes_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("USER_DEFINED_TYPE", true).ids("SUPERTYPE", true).done()
}

/// [§6.23 Description 1, 2] Each structured type of a direct supertype,
/// and that supertype.
fn supertypes(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let types = seen.state.catalog.structured_types().into_iter();
    types.filter_map(|s| Some([ids(&catalog, &s.name), ids(&catalog, s.supertype.as_ref()?)].concat())).collect()
}

// ---------------------------------------------------------------------------
// [§6.30] METHOD_SPECIFICATIONS, [§6.29] METHOD_SPECIFICATION_PARAMETERS
// ---------------------------------------------------------------------------

fn methods_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("SPECIFIC", true)
        .ids("UDT", true)
        .id("METHOD_NAME", true)
        .data("IS_STATIC", true)
        .data("IS_OVERRIDING", true)
        .data("IS_CONSTRUCTOR", true)
        .data("METHOD_LANGUAGE", true)
        .data("PARAMETER_STYLE", false)
        .id("DTD_IDENTIFIER", true)
        .data("IS_DETERMINISTIC", false)
        .data("SQL_DATA_ACCESS", true)
        .data("IS_NULL_CALL", true)
        .ids("TO_SQL_SPECIFIC", false)
        .data("AS_LOCATOR", true)
        .id("RESULT_CAST_FROM_DTD_IDENTIFIER", false)
        .data("RESULT_CAST_AS_LOCATOR", false);
    described(columns).done()
}

/// Each user-defined type that may have method specifications: the
/// structured types, then the distinct types.
fn hosts<'a>(seen: &'a Seen<'_>) -> Vec<MethodHost<'a>> {
    let catalog = &*seen.state.catalog;
    catalog
        .structured_types()
        .into_iter()
        .map(MethodHost::Structured)
        .chain(catalog.user_types().into_iter().map(MethodHost::Distinct))
        .collect()
}

/// The heads of the method specifications of `host`: of each its result
/// type, and of each parameter its type, as `heads` gives them.
fn method_heads(host: MethodHost<'_>, heads: &mut Vec<(QualifiedName, &'static str, String, DataType)>) {
    for (at, m) in specs(host).into_iter().enumerate() {
        heads.push((host.name().clone(), "USER-DEFINED TYPE", method_dtd(at), m.returns));
        for (p, (_, t)) in m.parameters.into_iter().enumerate() {
            heads.push((m.specific.clone(), "USER-DEFINED TYPE", p.saturating_add(1).to_string(), t));
        }
    }
}

/// [§6.30 Description 1 to 12] Each method specification of each
/// structured or distinct type: static, overriding, a constructor's or not; of
/// LANGUAGE SQL; its SQL-data access and null-call indications; and its
/// result type's descriptor.
fn methods(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for s in hosts(seen) {
        for (at, m) in specs(s).iter().enumerate() {
            let head = [ids(&catalog, &m.specific).to_vec(), ids(&catalog, s.name()).to_vec(), vec![text(m.name)]].concat();
            let kind = vec![yes(m.kind == MethodKind::Static), yes(m.overriding), yes(m.kind == MethodKind::Constructor)];
            let language = vec![text("SQL"), Value::Null, text(&method_dtd(at)), Value::Null];
            // [§6.30 Description 15a] No `<result cast>`, which an SQL
            // method has none of [ISO/IEC 9075-2:2023 §11.51 SR9b xiv 6 A I].
            let cast = [Value::Null, Value::Null];
            let access = [vec![text(super::routines::access(m.access)), yes(m.null_call)], vec![Value::Null; 3], vec![yes(false)]];
            let access = [access.concat(), cast.to_vec()].concat();
            rows.push([head, kind, language, access, data_type(&catalog, &m.returns, None)].concat());
        }
    }
    rows
}

fn method_parameters_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of()
        .ids("SPECIFIC", true)
        .number("ORDINAL_POSITION", true)
        .id("DTD_IDENTIFIER", true)
        .data("PARAMETER_MODE", true)
        .data("IS_RESULT", true)
        .data("AS_LOCATOR", true)
        .id("PARAMETER_NAME", false)
        .ids("FROM_SQL_SPECIFIC", false);
    described(columns).done()
}

/// [§6.29 Description 1 to 9] Each parameter of each method specification,
/// an input parameter at its ordinal position, from 1, of no RESULT, with
/// its name, null if it has none, and its type's descriptor.
fn method_parameters(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for s in hosts(seen) {
        for m in specs(s) {
            for (at, (name, t)) in m.parameters.iter().enumerate() {
                let position = at.saturating_add(1);
                let named = name.map_or(Value::Null, text);
                let own = vec![number(position), text(&position.to_string()), text("IN"), yes(false), yes(false), named];
                let from = vec![Value::Null; 3];
                rows.push([ids(&catalog, &m.specific).to_vec(), own, from, data_type(&catalog, t, None)].concat());
            }
        }
    }
    rows
}
