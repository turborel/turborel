//! The registered XML Schema an `<XML valid according to what>` of a
//! definition indicates, fixed as the definition is created [ISO/IEC
//! 9075-14:2006 draft §4.2.6]: noted then (`Invoked::indicated`), so the
//! definition's horizon keeps it (`catalog::Horizon::xml_schemas`), and,
//! of a URI, taken again of those whenever the definition is bound again
//! (`Horizon::pins`), so a registered XML Schema of a lesser name, or one
//! its owner may use later, never takes its place.

use std::borrow::Cow;

use super::super::Context;
use crate::ast::{TableName, TypeName, XmlValidAccording, XmlValidWhat};
use crate::catalog::xml_schemas::xml_schemas_of;
use crate::catalog::{Catalog, DomainDesc, QualifiedName, Who};
use crate::error::Result;
use crate::value::DataType;
use crate::value::types::MAX_TYPE_NESTING;

/// [§6.1 SR4] The data type `named` writes, of the cell `who`, as
/// `State::resolve_type` resolves it in `cx`: each URI of a type of
/// XMLSCHEMA taken of the registered XML Schemas `cx`'s definition
/// indicated as it was created, if it is a definition's; and each
/// registered XML Schema the type indicates noted, for a definition to
/// keep.
///
/// # Errors
///
/// As `State::resolve_type`.
pub(in crate::binder) fn resolved_type<'a>(cx: &Context<'a>, named: &TypeName, who: Who<'_>) -> Result<(DataType, Option<&'a DomainDesc>)> {
    let pins = cx.horizon.pins();
    let written = if pins.is_empty() { Cow::Borrowed(named) } else { Cow::Owned(pinned(named, pins, &*cx.db.catalog, 0)) };
    let (t, domain) = cx.db.resolve_type(&written, (cx.schema, who))?;
    if let Some(noted) = cx.invoked {
        for name in xml_schemas_of(&t) {
            noted.indicated(&name);
        }
    }
    Ok((t, domain))
}

/// `t` with each `<XML valid according to URI>` it writes, `depth` types
/// down, made the ID of the one of `pins` it indicates, if one does.
fn pinned(t: &TypeName, pins: &[QualifiedName], catalog: &dyn Catalog, depth: usize) -> TypeName {
    let deeper = depth.saturating_add(1);
    if deeper > MAX_TYPE_NESTING.saturating_add(1) {
        return t.clone();
    }
    match t {
        TypeName::XmlSchema(syntax, according) => TypeName::XmlSchema(*syntax, Box::new(pinned_according(according, pins, catalog))),
        TypeName::Collection(element, kind) => TypeName::Collection(Box::new(pinned(element, pins, catalog, deeper)), *kind),
        TypeName::Row(fields) => TypeName::Row(fields.iter().map(|(n, f)| (n.clone(), pinned(f, pins, catalog, deeper))).collect()),
        TypeName::Data(_) | TypeName::Domain(_) | TypeName::Reference(..) | TypeName::Extension { .. } => t.clone(),
    }
}

/// `according`, its URI, if it writes one, made the ID of the registered
/// XML Schema of `pins` of that target namespace and location, if one is.
pub(in crate::binder) fn pinned_according(
    according: &XmlValidAccording,
    pins: &[QualifiedName],
    catalog: &dyn Catalog,
) -> XmlValidAccording {
    let XmlValidWhat::Uri { namespace, location } = &according.what else { return according.clone() };
    let indicates = |name: &&QualifiedName| {
        catalog.xml_schema(name).is_some_and(|d| d.target_namespace == *namespace && location.as_ref().is_none_or(|l| *l == d.location))
    };
    let Some(pin) = pins.iter().find(indicates) else { return according.clone() };
    let id = TableName { auth: Some(pin.auth.clone()), ident: pin.ident.clone(), at: according.at };
    XmlValidAccording { what: XmlValidWhat::Id(id), ..according.clone() }
}
