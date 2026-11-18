//! [ISO/IEC 9075-14:2006 draft §21.4 to §21.7] The Definition Schema's
//! tables of registered XML Schemas: XML_SCHEMAS, one row of each;
//! XML_SCHEMA_NAMESPACES, one of each namespace of each; and
//! XML_SCHEMA_ELEMENTS, one of each global element declaration of each;
//! and the rows of USAGE_PRIVILEGES of OBJECT_TYPE XML SCHEMA [§21.4].
//!
//! Each describes the permanently registered XML Schemas of Table 1
//! [§4.2.6] (`catalog::xml_schemas::PERMANENT`), then those of the
//! catalog (`catalog::XmlSchemaDesc`), from the schema read from each
//! one's document. A namespace is its URI, the zero-length string of
//! absent, as NO NAMESPACE names it [§11.6 SR2a i], since
//! XML_SCHEMA_NAMESPACE is in each table's primary key [§21.5, §21.6]. A
//! schema's target namespace is always one of its namespaces [§4.2.6], as
//! XML_SCHEMAS' foreign key to XML_SCHEMA_NAMESPACES asks [§21.7].

use std::collections::BTreeMap;
use std::sync::Arc;

use super::privileges::PUBLIC;
use super::{Columns, catalog_name, defined, ids, text, yes};
use crate::catalog::xml_schemas::{PERMANENT, Permanent};
use crate::catalog::{ColumnDesc, QualifiedName, XmlSchemaDesc, owner_of};
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable};

/// [ISO/IEC 9075-14:2006 draft §21.4] The OBJECT_TYPE of USAGE on a
/// registered XML Schema.
const XML_SCHEMA_OBJECT: &str = "XML SCHEMA";

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("XML_SCHEMAS", schemas_columns, Rows::Infallible(schemas)),
        defined("XML_SCHEMA_NAMESPACES", namespaces_columns, Rows::Infallible(namespaces)),
        defined("XML_SCHEMA_ELEMENTS", elements_columns, Rows::Infallible(elements)),
    ]
}

/// A registered XML Schema, as the tables describe it.
struct Described {
    /// Its name.
    name: QualifiedName,
    /// Its target namespace URI.
    target: String,
    /// Its schema location URI.
    location: String,
    /// Whether it is permanently registered.
    permanent: bool,
    /// Each of its namespaces, and whether it is deterministic.
    namespaces: BTreeMap<String, bool>,
    /// Each of its global element declarations: its namespace, its local
    /// name, and whether it is deterministic.
    elements: Vec<(String, String, bool)>,
}

impl Described {
    /// [§4.2.6] Whether it is deterministic: no namespace of it is not.
    fn deterministic(&self) -> bool {
        self.namespaces.values().all(|d| *d)
    }
}

/// `desc`, from the schema read from its document; of one not read, its
/// target namespace alone, deterministic, as nothing it holds is known.
fn held(desc: &XmlSchemaDesc) -> Described {
    let mut namespaces = BTreeMap::new();
    let mut elements = Vec::new();
    if let Some(s) = &desc.schema {
        let target = Some(desc.target_namespace.as_str()).filter(|t| !t.is_empty());
        for ns in s.namespaces().into_iter().chain([target]) {
            namespaces.insert(ns.unwrap_or_default().to_owned(), !s.namespace_non_deterministic(ns));
        }
        for (name, e) in &s.element_names {
            elements.push((name.uri.as_deref().unwrap_or_default().to_owned(), name.local.to_string(), !s.non_deterministic(*e)));
        }
    }
    namespaces.entry(desc.target_namespace.clone()).or_insert(true);
    let (target, location) = (desc.target_namespace.clone(), desc.location.clone());
    Described { name: desc.name.clone(), target, location, permanent: false, namespaces, elements }
}

/// `p`, a permanently registered XML Schema.
fn permanent(p: &Permanent) -> Described {
    let namespaces = p.namespaces.iter().map(|n| (n.uri.to_owned(), n.deterministic)).collect();
    let elements = p.namespaces.iter().flat_map(|n| n.elements.iter().map(move |e| (n.uri.to_owned(), (*e).to_owned(), n.deterministic)));
    let (target, location) = (p.target_namespace.to_owned(), p.location.to_owned());
    Described { name: p.qualified(), target, location, permanent: true, namespaces, elements: elements.collect() }
}

/// Every registered XML Schema: the permanently registered, then those of
/// the catalog, in order.
fn described(seen: &Seen<'_>) -> Vec<Described> {
    PERMANENT.iter().map(permanent).chain(seen.state.catalog.xml_schemas().into_iter().map(held)).collect()
}

// ---------------------------------------------------------------------------
// [§21.7] XML_SCHEMAS
// ---------------------------------------------------------------------------

fn schemas_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().data("XML_SCHEMA_TARGET_NAMESPACE", true).data("XML_SCHEMA_LOCATION", true);
    columns.ids("XML_SCHEMA", true).data("XML_SCHEMA_IS_DETERMINISTIC", true).data("XML_SCHEMA_IS_PERMANENT", true).done()
}

/// [§21.7 Description 1 to 6] Each registered XML Schema.
fn schemas(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |d: &Described| {
        let head = [text(&d.target), text(&d.location)];
        [head.to_vec(), ids(&catalog, &d.name).to_vec(), vec![yes(d.deterministic()), yes(d.permanent)]].concat()
    };
    described(seen).iter().map(row).collect()
}

// ---------------------------------------------------------------------------
// [§21.6] XML_SCHEMA_NAMESPACES, [§21.5] XML_SCHEMA_ELEMENTS
// ---------------------------------------------------------------------------

fn namespaces_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("XML_SCHEMA", true).data("XML_SCHEMA_NAMESPACE", true);
    columns.data("XML_SCHEMA_NAMESPACE_IS_DETERMINISTIC", true).done()
}

/// [§21.6 Description 1 to 4] Each namespace of each registered XML
/// Schema.
fn namespaces(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for d in described(seen) {
        let named = ids(&catalog, &d.name);
        rows.extend(d.namespaces.iter().map(|(ns, det)| [named.to_vec(), vec![text(ns), yes(*det)]].concat()));
    }
    rows
}

fn elements_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("XML_SCHEMA", true).data("XML_SCHEMA_NAMESPACE", true).data("XML_SCHEMA_ELEMENT", true);
    columns.data("XML_SCHEMA_ELEMENT_IS_DETERMINISTIC", true).done()
}

/// [§21.5 Description 1 to 5] Each global element declaration of each
/// registered XML Schema.
fn elements(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for d in described(seen) {
        let named = ids(&catalog, &d.name);
        rows.extend(d.elements.iter().map(|(ns, local, det)| [named.to_vec(), vec![text(ns), text(local), yes(*det)]].concat()));
    }
    rows
}

// ---------------------------------------------------------------------------
// [§21.4] USAGE_PRIVILEGES
// ---------------------------------------------------------------------------

/// [§21.4] The rows of USAGE_PRIVILEGES of registered XML Schemas: on each
/// permanently registered one, PUBLIC's, of the grantor "_SYSTEM", not
/// grantable; on each the catalog holds, its schema's owner's, of
/// "_SYSTEM", grantable, and each granted.
pub(super) fn usage(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |name: &QualifiedName, (grantor, grantee): (&str, &str), grantable: bool| -> Row {
        let kind = [text(XML_SCHEMA_OBJECT), yes(grantable)];
        [vec![text(grantor), text(grantee)], ids(&catalog, name).to_vec(), kind.to_vec()].concat()
    };
    let mut rows: Vec<Row> = PERMANENT.iter().map(|p| row(&p.qualified(), (SYSTEM_GRANTOR, PUBLIC), false)).collect();
    for x in seen.state.catalog.xml_schemas() {
        rows.push(row(&x.name, (SYSTEM_GRANTOR, owner_of(&*seen.state.catalog, &x.name.auth)), true));
        rows.extend(x.usage.iter().map(|u| row(&x.name, (&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC)), u.grantable)));
    }
    rows
}
