//! \[EXTENSION\] Elements' identities as GQL's data-modifying statements
//! read and write them (graph standard §5 R5.1, RULES.md D-GQL): the text
//! ELEMENT_ID gives, `{"table":"ACCOUNT","key":[6]}`, its element table's
//! alias and its key's values, so a binding table's identities are
//! GRAPH_TABLE's. A row is found by its key, each value read from the
//! identity in SQL as its column's type, so no value is spliced into the
//! text.

use crate::binder::gql::Graph;
use crate::catalog::{ElementKind, ElementTableDesc, EndpointDesc, identifier, qualified_identifier};
use crate::db::Session;
use crate::error::{Code, Result, SqlError};
use crate::json::{Item, Keys};
use crate::logical::identity_text;
use crate::value::Value;

/// The element table alias identity `id` names.
///
/// # Errors
///
/// SQLCODE -910 if `id` is no identity.
fn alias_of(id: &str) -> Result<String> {
    let item = crate::json::parse(id, Keys::Any)?;
    let table = item.as_object().and_then(|m| m.iter().find(|m| m.key == "table")).map(|m| &m.value);
    if let Some(Item::Scalar(s)) = table
        && let Value::Char(alias) = s.value()
    {
        return Ok(alias.clone());
    }
    Err(SqlError::new(Code::Data, format!("[EXTENSION] {id} is no element's identity")))
}

/// The element table of the element whose identity is `id`.
///
/// # Errors
///
/// SQLCODE -910 if it is none of `graph`'s.
pub(super) fn table_of<'g>(graph: &'g Graph, id: &str) -> Result<&'g ElementTableDesc> {
    let alias = alias_of(id)?;
    graph
        .desc
        .element(&alias)
        .ok_or_else(|| SqlError::new(Code::Data, format!("[EXTENSION] {id} is no element of graph {}", graph.desc.name)))
}

/// The identity of the element of `e` whose key is `key`, as ELEMENT_ID
/// gives it.
///
/// # Errors
///
/// As `identity_text`.
pub(super) fn identity_of(e: &ElementTableDesc, key: &[Value]) -> Result<String> {
    identity_text(&e.alias, key)
}

/// Of an open graph, its element table of `kind`.
///
/// # Errors
///
/// SQLCODE -910 if it has none.
pub(super) fn open_element(graph: &Graph, kind: ElementKind) -> Result<&ElementTableDesc> {
    graph
        .desc
        .elements
        .iter()
        .find(|e| e.kind == kind)
        .ok_or_else(|| SqlError::new(Code::Data, format!("[EXTENSION] open graph {} has no element table of {kind:?}", graph.desc.name)))
}

/// Value `i` of the key of the identity parameter `param`, as SQL type
/// `t`.
pub(super) fn key_value(param: &str, i: usize, t: &str) -> String {
    format!("JSON_VALUE({param}, 'lax $.key[{i}]' RETURNING {t})")
}

impl Session {
    /// The condition that the row of `e`, its table written `q`, is the
    /// element of identity parameter `param`: each key column equal to its
    /// value.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `e`'s table is not there.
    pub(super) fn keyed(&self, e: &ElementTableDesc, q: &str, param: &str) -> Result<String> {
        let st = self.gql_state()?;
        let Some(t) = st.catalog.table(&e.table) else {
            return Err(SqlError::new(Code::Data, format!("[EXTENSION] element table {} is no base table", e.alias)));
        };
        let mut at = Vec::new();
        for (i, k) in e.key.iter().enumerate() {
            let column = t.columns.iter().find(|c| c.name == *k).map(|c| c.data_type.to_string()).unwrap_or_default();
            at.push(format!("{q}.{} = {}", identifier(k), key_value(param, i, &column)));
        }
        Ok(at.join(" AND "))
    }

    /// The condition that a row of an edge table, written `q`, has the
    /// vertex of identity parameter `param` as its endpoint `end`, of
    /// vertex table `v`.
    ///
    /// # Errors
    ///
    /// As `keyed`.
    pub(super) fn ends_at(&self, (v, end): (&ElementTableDesc, &EndpointDesc), q: &str, param: &str) -> Result<String> {
        let mut at = vec![self.keyed(v, "\"$v\"", param)?];
        at.extend(end.references.iter().zip(&end.columns).map(|(r, c)| format!("\"$v\".{} = {q}.{}", identifier(r), identifier(c))));
        Ok(format!("EXISTS (SELECT 1 FROM {} AS \"$v\" WHERE {})", qualified_identifier(&v.table), at.join(" AND ")))
    }
}
