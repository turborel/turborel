//! \[DESIGN\] Property graph descriptors (RULES.md D106) in the
//! catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! graph:      name, elements (count u32, each an element), mode u8 (0
//!             enforced, 1 trusted), invoker u8, names (str), reads (count
//!             u32, names), horizon, valid u8, select (as a domain's usage)
//! element:    alias (str), table name, kind u8 (0 vertex, 1 edge), key
//!             (count u32, each str), labels (count u32, each a label),
//!             endpoints (0 none, or 1, the source, the destination and
//!             undirected u8)
//! label:      name (str), properties (count u32, each name (str),
//!             expression (str) and type)
//! endpoint:   vertex alias (str), columns (count u32, each str),
//!             references (likewise)
//! ```

use super::super::{ElementKind, ElementTableDesc, EndpointDesc, Endpoints, GraphDesc, GraphMode, LabelDesc, PropertyDesc};
use super::{flag, get_all, horizon, put_all, put_flag, put_horizon, put_name, put_usage, qualified, usage};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::types::tags::{data_type, put_type};

/// The tag of ENFORCED MODE; TRUSTED MODE's is 1.
const ENFORCED: u8 = 0;
/// The tag of a vertex table; an edge table's is 1.
const VERTEX: u8 = 0;

/// `graph` in bytes: every part of its descriptor.
pub(crate) fn put_graph(out: &mut Vec<u8>, graph: &GraphDesc) {
    put_name(out, &graph.name);
    put_all(out, graph.elements.iter(), put_element);
    out.put_u8(if graph.mode == GraphMode::Enforced { ENFORCED } else { 1 });
    put_flag(out, graph.invoker);
    out.put_str(&graph.names);
    put_all(out, graph.reads.iter(), put_name);
    put_horizon(out, &graph.horizon);
    put_flag(out, graph.valid);
    put_usage(out, &graph.select);
}

/// The graph descriptor `put_graph` wrote.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not one.
pub(crate) fn graph(r: &mut Reader<'_>) -> Result<GraphDesc> {
    let (name, elements) = (qualified(r)?, get_all(r, element)?);
    let mode = if r.u8()? == ENFORCED { GraphMode::Enforced } else { GraphMode::Trusted };
    let (invoker, names, reads, horizon) = (flag(r)?, r.str()?, get_all(r, qualified)?, horizon(r)?);
    Ok(GraphDesc { name, elements, mode, invoker, names, reads, horizon, valid: flag(r)?, select: usage(r)? })
}

/// An element table in bytes.
fn put_element(out: &mut Vec<u8>, e: &ElementTableDesc) {
    out.put_str(&e.alias);
    put_name(out, &e.table);
    out.put_u8(if e.kind == ElementKind::Vertex { VERTEX } else { 1 });
    put_all(out, e.key.iter(), |out, c| out.put_str(c));
    put_all(out, e.labels.iter(), put_label);
    put_flag(out, e.endpoints.is_some());
    if let Some(ends) = &e.endpoints {
        put_endpoint(out, &ends.source);
        put_endpoint(out, &ends.destination);
        put_flag(out, ends.undirected);
    }
    put_flag(out, e.query.is_some());
    if let Some(q) = &e.query {
        out.put_str(q);
    }
}

/// What `put_element` wrote.
fn element(r: &mut Reader<'_>) -> Result<ElementTableDesc> {
    let (alias, table) = (r.str()?, qualified(r)?);
    let kind = if r.u8()? == VERTEX { ElementKind::Vertex } else { ElementKind::Edge };
    let (key, labels) = (get_all(r, text)?, get_all(r, label)?);
    let endpoints = if flag(r)? { Some(Endpoints { source: endpoint(r)?, destination: endpoint(r)?, undirected: flag(r)? }) } else { None };
    let query = if flag(r)? { Some(r.str()?) } else { None };
    Ok(ElementTableDesc { alias, table, kind, key, labels, endpoints, query })
}

/// A label and its properties in bytes.
fn put_label(out: &mut Vec<u8>, l: &LabelDesc) {
    out.put_str(&l.name);
    put_all(out, l.properties.iter(), |out, p| {
        out.put_str(&p.name);
        out.put_str(&p.expression);
        put_type(out, &p.data_type);
    });
}

/// What `put_label` wrote.
fn label(r: &mut Reader<'_>) -> Result<LabelDesc> {
    let name = r.str()?;
    let properties = get_all(r, |r| Ok(PropertyDesc { name: r.str()?, expression: r.str()?, data_type: data_type(r)? }))?;
    Ok(LabelDesc { name, properties })
}

/// An endpoint in bytes.
fn put_endpoint(out: &mut Vec<u8>, e: &EndpointDesc) {
    out.put_str(&e.vertex);
    put_all(out, e.columns.iter(), |out, c| out.put_str(c));
    put_all(out, e.references.iter(), |out, c| out.put_str(c));
}

/// What `put_endpoint` wrote.
fn endpoint(r: &mut Reader<'_>) -> Result<EndpointDesc> {
    Ok(EndpointDesc { vertex: r.str()?, columns: get_all(r, text)?, references: get_all(r, text)? })
}

/// A string.
fn text(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}
