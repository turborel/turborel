//! \[EXTENSION\] INTO PROPERTY of a property graph named in place of the
//! edges (RULES.md D-GRAPH-ALGO), the standard's "written as a new
//! property" [GS §18]: the result's value written to a column of each of
//! the graph's vertex tables, or of the one OF names, each row's of the
//! vertex the result names.
//!
//! A result names a vertex by its identity, ELEMENT_ID's text of its
//! element table and key (`db/ptf/graph/shape.rs`). Each vertex table's
//! MERGE reads its own rows of the result, those of its alias, and their
//! key's values from the identity in order, by JSON_VALUE as of the key
//! columns' types, and sets the column of the row of that key. Of a
//! SYS_GRAPH a vertex is its `$NODE_ID`, which the MERGE matches as it is.
//! The column is added first if the table has none of its name, of the
//! result's value's type. The statement holds its runs' results
//! (`GraphRuns::hold`), so the algorithm runs once for every table. Each MERGE checks SELECT and UPDATE as any
//! does, and all are of the statement's transaction.

use crate::ast::{IntoProperty, SqlStatement};
use crate::catalog::{ElementKind, ElementTableDesc, GraphDesc, NODE_ID, QualifiedName, SYS_GRAPH, identifier};
use crate::db::ptf::identity_type;
use crate::db::{Completion, Session, StatementContext, rule};
use crate::error::Result;
use crate::params::Params;
use crate::parser::Parser;

use super::written;

/// The member of an identity naming its element table, as a JSON path.
const TABLE_PATH: &str = "lax $.table";

impl Session {
    /// The INTO PROPERTY `p` of the result of graph `g`, written to its
    /// vertex table `only` if OF names one, else to each, in `cx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `p` names a key column, which a graph's vertex
    /// tables have of their own, or `only` is none of `g`'s vertex
    /// tables; as ALTER TABLE's and MERGE's otherwise.
    pub(super) fn property_of_graph(
        &self,
        cx: StatementContext<'_>,
        p: &IntoProperty,
        (g, only): (&GraphDesc, Option<QualifiedName>),
        params: &Params,
    ) -> Result<Completion> {
        if let Some(key) = &p.key {
            return Err(rule(format!(
                "[EXTENSION] INTO PROPERTY of graph {} names no key column {key}: its vertex tables have keys",
                g.name
            )));
        }
        let vertices: Vec<&ElementTableDesc> =
            g.elements.iter().filter(|e| e.kind == ElementKind::Vertex && only.as_ref().is_none_or(|t| *t == e.table)).collect();
        if let (Some(t), true) = (&only, vertices.is_empty()) {
            return Err(rule(format!("[EXTENSION] INTO PROPERTY: {t} is no vertex table of graph {}", g.name)));
        }
        let native = g.name.ident == SYS_GRAPH;
        // The algorithm runs once, its rows read by each table's MERGE.
        let _held = cx.st.graph_runs.hold(cx.tx.id);
        let mut st = std::sync::Arc::clone(cx.st);
        let mut completion = Completion::NoData;
        for v in vertices {
            let desc = st.base_table(&v.table)?;
            let keys = if native { vec![(NODE_ID.to_owned(), String::new())] } else { key_columns(&desc, v)? };
            let typed = match (p.value_type, native) {
                (Some(t), _) => t.to_owned(),
                (None, true) => desc
                    .column(NODE_ID)
                    .and_then(|k| desc.columns.get(k))
                    .map_or_else(|| identity_type().to_string(), |c| c.data_type.to_string()),
                (None, false) => identity_type().to_string(),
            };
            st = self.with_column(StatementContext { st: &st, tx: cx.tx }, (&written(&v.table), &desc), &p.column, &typed)?;
            let SqlStatement::Merge(merge) = Parser::new(&vertex_merge(p, v, &keys, native))?.sql_statement()? else {
                return Err(rule("[DESIGN] INTO PROPERTY is a MERGE"));
            };
            let done = self.merge_completed(StatementContext { st: &st, tx: cx.tx }, &merge, params, None)?;
            if done != Completion::NoData {
                completion = done;
            }
        }
        Ok(completion)
    }
}

/// The key columns of vertex table `v`, of descriptor `desc`, each's name
/// and type.
fn key_columns(desc: &crate::catalog::TableDesc, v: &ElementTableDesc) -> Result<Vec<(String, String)>> {
    v.key
        .iter()
        .map(|k| {
            let column = desc.column(k).and_then(|at| desc.columns.get(at));
            column
                .map(|c| (k.clone(), c.data_type.to_string()))
                .ok_or_else(|| rule(format!("[EXTENSION] INTO PROPERTY: {} has no column {k}", v.table)))
        })
        .collect()
}

/// The MERGE writing `p` to vertex table `v` of key columns `keys`: each
/// row of the result of `v`'s alias, its key read from its identity, or of
/// a SYS_GRAPH its `$NODE_ID` as it is.
fn vertex_merge(p: &IntoProperty, v: &ElementTableDesc, keys: &[(String, String)], native: bool) -> String {
    let picked: Vec<String> = keys
        .iter()
        .enumerate()
        .map(|(i, (_, typed))| {
            if native {
                format!("GRAPH_ALGO_RESULT.VERTEX AS GRAPH_ALGO_KEY_{i}")
            } else {
                format!("JSON_VALUE(GRAPH_ALGO_RESULT.VERTEX, 'lax $.key[{i}]' RETURNING {typed}) AS GRAPH_ALGO_KEY_{i}")
            }
        })
        .collect();
    let on: Vec<String> = keys
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("GRAPH_ALGO_TARGET.{} = GRAPH_ALGO_ROWS.GRAPH_ALGO_KEY_{i}", identifier(name)))
        .collect();
    let alias = v.alias.replace('\'', "''");
    let only_its = if native { String::new() } else { format!(" WHERE JSON_VALUE(GRAPH_ALGO_RESULT.VERTEX, '{TABLE_PATH}') = '{alias}'") };
    format!(
        "MERGE INTO {} AS GRAPH_ALGO_TARGET USING (SELECT {}, GRAPH_ALGO_RESULT.{} AS GRAPH_ALGO_VALUE FROM {} AS GRAPH_ALGO_RESULT{only_its}) \
         AS GRAPH_ALGO_ROWS ON {} WHEN MATCHED THEN UPDATE SET {} = GRAPH_ALGO_ROWS.GRAPH_ALGO_VALUE",
        written(&v.table),
        picked.join(", "),
        identifier(&p.value),
        p.invocation,
        on.join(" AND "),
        identifier(&p.column),
    )
}
