//! A schema's type definitions by the names nodes are annotated with
//! [XQueryDM §3.3.1.1], and the names two schemas define from different
//! schema documents [XQuery §4.10]: what XQuery's Schema Import Feature
//! reads of a schema imported.

use std::sync::Arc;

use super::assess::{ANONYMOUS, ANONYMOUS_SIMPLE, type_name};
use super::builtin;
use super::component::{ComplexId, Expanded, Schema, Simple, SimpleId, TypeDef};
use super::read::tree_namespace;
use crate::xml::{QName, XS_NAMESPACE};

/// The built-in type definition of the XML Schema namespace named
/// `local` [Schema1 §3.4.7, §3.14.7], if there is one.
#[must_use]
pub fn built_in_type(local: &str) -> Option<TypeDef> {
    builtin::named(local)
}

/// The name a node validated by `t` of `schema` is annotated with
/// [XQueryDM §3.3.1.1], an anonymous type's made from its place.
#[must_use]
pub fn annotation_name(schema: &Schema, t: TypeDef) -> QName {
    type_name(schema, t)
}

/// The type definition of `schema` a node's type annotation `name`
/// names: a built-in one of the XML Schema namespace, a named one of
/// `schema`, or an anonymous one of `schema`, by the place its name gives
/// (`annotation_name`); none if `schema` has none of the name.
#[must_use]
pub fn annotated_type(schema: &Schema, name: &QName) -> Option<TypeDef> {
    let uri = name.uri.as_deref();
    if uri == Some(XS_NAMESPACE)
        && let Some(built_in) = builtin::named(&name.local)
    {
        return Some(built_in);
    }
    if let Some(named) = schema.type_names.get(&Expanded::new(uri, &name.local)) {
        return Some(*named);
    }
    if let Some(n) = name.local.strip_prefix(ANONYMOUS_SIMPLE).and_then(|n| n.parse::<usize>().ok()) {
        let def = schema.simple(SimpleId(n)).filter(|d| d.name.is_none() && d.namespace.as_deref() == uri);
        return def.map(|_| TypeDef::Simple(Simple::Defined(SimpleId(n))));
    }
    let n = name.local.strip_prefix(ANONYMOUS).and_then(|n| n.parse::<usize>().ok())?;
    let def = schema.complex(ComplexId(n)).filter(|d| d.name.is_none() && d.namespace.as_deref() == uri);
    def.map(|_| TypeDef::Complex(ComplexId(n)))
}

/// [XQuery §4.10 err:XQST0035] A name of one symbol space both `a` and `b`
/// define from schema documents of its namespace that are not the same
/// documents, so two definitions of it are imported; none if there is
/// none, or `a` is `b`.
#[must_use]
pub fn defined_twice(a: &Schema, b: &Schema) -> Option<Expanded> {
    if std::ptr::eq(a, b) {
        return None;
    }
    let documents = |s: &Schema, namespace: Option<&str>| {
        let mut found: Vec<usize> =
            s.documents.iter().filter(|(t, _)| tree_namespace(t) == namespace).map(|(t, _)| Arc::as_ptr(t).addr()).collect();
        found.sort_unstable();
        found
    };
    let both = a
        .type_names
        .keys()
        .filter(|n| b.type_names.contains_key(*n))
        .chain(a.element_names.keys().filter(|n| b.element_names.contains_key(*n)))
        .chain(a.attribute_names.keys().filter(|n| b.attribute_names.contains_key(*n)))
        .chain(a.attribute_group_names.keys().filter(|n| b.attribute_group_names.contains_key(*n)))
        .chain(a.group_names.keys().filter(|n| b.group_names.contains_key(*n)))
        .chain(a.notation_names.keys().filter(|n| b.notation_names.contains_key(*n)));
    let mut different = both.filter(|n| documents(a, n.uri.as_deref()) != documents(b, n.uri.as_deref()));
    different.next().cloned()
}

#[cfg(test)]
mod tests;
