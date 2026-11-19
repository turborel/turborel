//! \[DESIGN\] Graph type and GQL graph descriptors (RULES.md D144) in the
//! catalog's bytes.
//!
//! ```text
//! graph type: name, definition (str)
//! GQL graph:  name, graph type (0 open, or 1 and its name)
//! ```

use super::super::{GqlGraphDesc, GraphTypeDesc};
use super::{put_name, qualified};
use crate::bytes::{Put, Reader};
use crate::error::Result;

/// The tag of an open graph; a typed one's is 1, its type's name after.
const OPEN: u8 = 0;

/// `t` in bytes.
pub(crate) fn put_graph_type(out: &mut Vec<u8>, t: &GraphTypeDesc) {
    put_name(out, &t.name);
    out.put_str(&t.definition);
}

/// The graph type `put_graph_type` wrote.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not one.
pub(crate) fn graph_type(r: &mut Reader<'_>) -> Result<GraphTypeDesc> {
    Ok(GraphTypeDesc { name: qualified(r)?, definition: r.str()? })
}

/// `g` in bytes.
pub(crate) fn put_gql_graph(out: &mut Vec<u8>, g: &GqlGraphDesc) {
    put_name(out, &g.name);
    match &g.graph_type {
        None => out.put_u8(OPEN),
        Some(t) => {
            out.put_u8(1);
            put_name(out, t);
        }
    }
}

/// The GQL graph `put_gql_graph` wrote.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not one.
pub(crate) fn gql_graph(r: &mut Reader<'_>) -> Result<GqlGraphDesc> {
    let name = qualified(r)?;
    let graph_type = if r.u8()? == OPEN { None } else { Some(qualified(r)?) };
    Ok(GqlGraphDesc { name, graph_type })
}
