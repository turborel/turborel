//! [ISO/IEC 9075:1992 §21.3.5] The data type descriptors.
//! DATA_TYPE_DESCRIPTOR has one row for each domain and one for each
//! column of each table that is of no domain. ELEMENT_TYPES and FIELDS,
//! the tables ISO/IEC 9075-11 names so, have one row for each descriptor
//! subordinate to one of those [ISO/IEC 9075-2:2023 §4.10.2, §4.12.1],
//! or to a user-defined type's source type, attribute or method's type or
//! a routine's parameter or result type (`definition/user_types.rs`,
//! `definition/structured.rs`, `definition/routines.rs`). ELEMENT_TYPES
//! holds each array and multiset type's element type, and FIELDS each
//! field of each row type. REFERENCED_TYPES, of ISO/IEC 9075-11:2006
//! draft §6.32, holds each reference type's referenced type [§4.11].
//!
//! Each descriptor's DTD_IDENTIFIER is unique among those of its object,
//! and implementation-dependent. A column's is its ordinal position and a
//! domain's is 1. The one subordinate to the descriptor of identifier D
//! is D, a full stop, then the field's ordinal position, or 1 of an
//! element type or a referenced type. DATA_TYPE_DESCRIPTOR keys its rows by object and column,
//! which a subordinate descriptor shares with the one it is subordinate
//! to, so each row of ELEMENT_TYPES and FIELDS holds its descriptor's
//! columns itself. A field or element type of a character string is of
//! the collation of its character set, SQL_TEXT, as none is named for it.

use std::sync::Arc;

use super::{Columns, SQL_TEXT, catalog_name, defined, ids, information, number, objects, text};
use super::{routines, structured, user_types};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::error::Result;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::types::MAX_TYPE_NESTING;
use crate::value::{DataType, Value};

/// The DTD_IDENTIFIER of a domain's descriptor.
const DOMAIN_DTD: &str = "1";

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("DATA_TYPE_DESCRIPTOR", data_type_columns, Rows::Fallible(data_types)),
        defined("ELEMENT_TYPES", element_columns, Rows::Fallible(elements)),
        defined("FIELDS", field_columns, Rows::Fallible(fields)),
        defined("REFERENCED_TYPES", referenced_columns, Rows::Fallible(referenced)),
    ]
}

/// Then the columns of a descriptor, from DATA_TYPE on, ending with a
/// user-defined type's name [ISO/IEC 9075-2:2023 §4.9.2] and a reference
/// type's scope [§4.11], as ISO/IEC 9075-11:2006 draft §6.21 orders them.
pub(super) fn described(columns: Columns) -> Columns {
    described_as(columns, true)
}

/// Then the columns of a descriptor, as `described` gives them, DATA_TYPE
/// NOT NULL if `not_null`: of a row that may describe no data type.
pub(super) fn described_as(columns: Columns, not_null: bool) -> Columns {
    columns
        .data("DATA_TYPE", not_null)
        .length("CHARACTER_MAXIMUM_LENGTH", false)
        .length("CHARACTER_OCTET_LENGTH", false)
        .ids("COLLATION", false)
        .number("NUMERIC_PRECISION", false)
        .number("NUMERIC_PRECISION_RADIX", false)
        .number("NUMERIC_SCALE", false)
        .number("DATETIME_PRECISION", false)
        .number("MAXIMUM_CARDINALITY", false)
        .ids("USER_DEFINED_TYPE", false)
        .ids("SCOPE", false)
}

fn data_type_columns() -> Vec<ColumnDesc> {
    described(Columns::of().ids("TABLE_OR_DOMAIN", true).id("COLUMN_NAME", true).id("DTD_IDENTIFIER", true)).done()
}

fn element_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("OBJECT", true).data("OBJECT_TYPE", true);
    described(columns.id("COLLECTION_TYPE_IDENTIFIER", true).id("DTD_IDENTIFIER", true)).done()
}

fn referenced_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("OBJECT", true).data("OBJECT_TYPE", true);
    described(columns.id("REFERENCE_TYPE_IDENTIFIER", true).id("DTD_IDENTIFIER", true)).done()
}

fn field_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("OBJECT", true).data("OBJECT_TYPE", true).id("ROW_IDENTIFIER", true);
    described(columns.id("FIELD_NAME", true).number("ORDINAL_POSITION", true).id("DTD_IDENTIFIER", true)).done()
}

/// A descriptor of a domain, or of a column of no domain, which no other
/// is subordinate to.
struct Top {
    /// The domain, or the column's table.
    object: QualifiedName,
    /// Whether the object is a domain.
    domain: bool,
    /// The column's name, empty of a domain.
    column: String,
    /// Its DTD_IDENTIFIER.
    dtd: String,
    /// The data type it describes.
    data_type: DataType,
    /// The collation named for it.
    collation: Option<QualifiedName>,
}

/// Each domain's descriptor, then each of each column of no domain of
/// each table.
///
/// # Errors
///
/// A view of the Information Schema does not bind: a defect.
fn tops(seen: &Seen<'_>) -> Result<Vec<Top>> {
    let domains = seen.state.catalog.domains().into_iter().map(|d| Top {
        object: d.name.clone(),
        domain: true,
        column: String::new(),
        dtd: DOMAIN_DTD.to_owned(),
        data_type: d.data_type.clone(),
        collation: d.collation.clone(),
    });
    let mut tops: Vec<Top> = domains.collect();
    for o in objects(seen)? {
        for (at, c) in o.columns.iter().enumerate().filter(|(_, c)| c.domain.is_none()) {
            tops.push(Top {
                object: o.name.clone(),
                domain: false,
                column: c.name.clone(),
                dtd: at.saturating_add(1).to_string(),
                data_type: c.data_type.clone(),
                collation: c.collation.clone(),
            });
        }
    }
    Ok(tops)
}

/// [§21.3.5] One row for each domain, its column name empty, and one for
/// each column of each table that is of no domain.
fn data_types(seen: &Seen<'_>) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let row = |t: Top| {
        let key = vec![text(&t.column), text(&t.dtd)];
        [ids(&catalog, &t.object).to_vec(), key, data_type(&catalog, &t.data_type, t.collation.as_ref())].concat()
    };
    Ok(tops(seen)?.into_iter().map(row).collect())
}

/// What a descriptor subordinate to another describes: an array type's
/// element type, a row type's field of a name and an ordinal position, or
/// a reference type's referenced type.
#[derive(Clone, Copy)]
enum Part<'a> {
    Element,
    Field(&'a str, usize),
    Referenced,
}

/// Which table's rows `subordinates` gives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nested {
    /// ELEMENT_TYPES'.
    Elements,
    /// FIELDS'.
    Fields,
    /// REFERENCED_TYPES'.
    Referenced,
}

/// The descriptors directly subordinate to that of `t`.
fn parts(t: &DataType) -> Vec<(Part<'_>, DataType)> {
    if let DataType::Array(a) = t {
        vec![(Part::Element, a.element.clone())]
    } else if let DataType::Multiset(m) = t {
        vec![(Part::Element, m.element.clone())]
    } else if let DataType::Row(r) = t {
        r.fields.iter().enumerate().map(|(i, f)| (Part::Field(&f.name, i.saturating_add(1)), f.data_type.clone())).collect()
    } else if let DataType::Reference(r) = t {
        let referenced = DataType::structured(r.referenced.name.clone(), r.referenced.supertypes.clone());
        vec![(Part::Referenced, referenced)]
    } else {
        Vec::new()
    }
}

/// The descriptors subordinate to that of `t`, of identifier `dtd`, each
/// with its own identifier and then those subordinate to it: of `kind`'s
/// table, its rows. Each row begins with `head`, the object's name and
/// type. Types nest at most `MAX_TYPE_NESTING` deep, so `depth` does too
/// (RULES.md R-CF-6).
fn subordinates(t: &DataType, (dtd, depth): (&str, usize), head: &[Value], kind: Nested, out: &mut Vec<Row>) {
    if depth >= MAX_TYPE_NESTING {
        return;
    }
    let catalog = head.first().cloned().unwrap_or(Value::Null);
    for (part, s) in parts(t) {
        let own = match part {
            Part::Element | Part::Referenced => format!("{dtd}.1"),
            Part::Field(_, at) => format!("{dtd}.{at}"),
        };
        let named = match (part, kind) {
            (Part::Element, Nested::Elements) | (Part::Referenced, Nested::Referenced) => Some(vec![text(dtd), text(&own)]),
            (Part::Field(name, at), Nested::Fields) => Some(vec![text(dtd), text(name), number(at), text(&own)]),
            (Part::Element, Nested::Fields | Nested::Referenced)
            | (Part::Field(..), Nested::Elements | Nested::Referenced)
            | (Part::Referenced, Nested::Elements | Nested::Fields) => None,
        };
        if let Some(named) = named {
            out.push([head.to_vec(), named, data_type(&catalog, &s, None)].concat());
        }
        subordinates(&s, (&own, depth.saturating_add(1)), head, kind, out);
    }
}

/// The rows of `kind`'s table.
fn nested(seen: &Seen<'_>, kind: Nested) -> Result<Vec<Row>> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in tops(seen)? {
        let head = [ids(&catalog, &t.object).to_vec(), vec![text(if t.domain { "DOMAIN" } else { "TABLE" })]].concat();
        subordinates(&t.data_type, (&t.dtd, 0), &head, kind, &mut rows);
    }
    let heads = user_types::heads(seen).into_iter().chain(structured::heads(seen)).chain(routines::heads(seen));
    for (object, object_type, dtd, t) in heads {
        let head = [ids(&catalog, &object).to_vec(), vec![text(object_type)]].concat();
        subordinates(&t, (&dtd, 0), &head, kind, &mut rows);
    }
    Ok(rows)
}

/// Each array type's element type, with the identifier of the array
/// type's descriptor, COLLECTION_TYPE_IDENTIFIER.
fn elements(seen: &Seen<'_>) -> Result<Vec<Row>> {
    nested(seen, Nested::Elements)
}

/// Each row type's fields, with the identifier of the row type's
/// descriptor, ROW_IDENTIFIER, and each field's name and ordinal position.
fn fields(seen: &Seen<'_>) -> Result<Vec<Row>> {
    nested(seen, Nested::Fields)
}

/// [ISO/IEC 9075-11:2006 draft §6.32 Description 1, 2] Each reference
/// type's referenced type, with the identifier of the reference type's
/// descriptor, REFERENCE_TYPE_IDENTIFIER.
fn referenced(seen: &Seen<'_>) -> Result<Vec<Row>> {
    nested(seen, Nested::Referenced)
}

/// [§21.3.5 Description 3] The data type `t`, from DATA_TYPE on, as its
/// kind describes it (`SqlType::descriptor`): a character string's
/// collation `named`, else the character set's, SQL_TEXT; a user-defined
/// type's name; a reference type's scope, the table's name [ISO/IEC
/// 9075-11:2006 draft §6.21].
pub(super) fn data_type(catalog: &Value, t: &DataType, named: Option<&QualifiedName>) -> Vec<Value> {
    let d = t.kind().descriptor(t);
    let optional = |n: Option<u32>| n.map_or(Value::Null, |n| number(usize::try_from(n).unwrap_or(usize::MAX)));
    let default = information(SQL_TEXT);
    let collation = if d.collated { ids(catalog, named.unwrap_or(&default)).to_vec() } else { vec![Value::Null, Value::Null, Value::Null] };
    let long = |n: Option<u64>| n.map_or(Value::Null, |n| number(usize::try_from(n).unwrap_or(usize::MAX)));
    let head = vec![text(&d.name), long(d.length), long(d.octets)];
    let tail = vec![optional(d.precision), optional(d.radix), optional(d.scale), optional(d.datetime_precision), optional(d.cardinality)];
    let user_defined = match &d.user_defined {
        Some((auth, ident)) => ids(catalog, &QualifiedName { auth: auth.clone(), ident: ident.clone() }).to_vec(),
        None => vec![Value::Null, Value::Null, Value::Null],
    };
    let scope = if let DataType::Reference(r) = t { r.scope.as_ref() } else { None };
    let scope = scope.map(|s| ids(catalog, &QualifiedName { auth: s.schema.clone(), ident: s.name.clone() }));
    let scope = scope.map_or_else(|| vec![Value::Null, Value::Null, Value::Null], |s| s.to_vec());
    [head, collation, tail, user_defined, scope].concat()
}
