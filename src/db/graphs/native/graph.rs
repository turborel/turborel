//! \[EXTENSION\] The implicit property graph SYS_GRAPH of a schema's node
//! and edge tables (§4.2), kept by the database: described again after
//! every change of definitions, as a graph of TRUSTED MODE whose vertex
//! tables are the node tables, each keyed by `$NODE_ID` with its own name
//! as its label, and whose edge tables are the edge tables, each once for
//! each pair of node tables its edges may join. An edge table with
//! CONNECTION constraints joins the pairs every one of them allows (an
//! edge satisfies them all); one with none joins any two node tables of
//! its schema. Each label's properties are its table's columns but the
//! pseudo-columns. A schema with no node table has no SYS_GRAPH.
//!
//! It is described as CREATE PROPERTY GRAPH describes a graph
//! (`db/graphs/describe.rs`), so its rules and its views are a graph's.
//! No statement defines, alters or drops it, and a change of its tables
//! never finds it broken: it is described anew instead.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::{Describing, graph_rule};
use crate::ast::{
    ElementDefinition, EndpointDefinition, GraphDefinition, GraphModeOption, LabelDefinition, PropertiesDefinition, TableName,
};
use crate::catalog::{EDGE_ID, FROM_ID, NODE_ID, NativeKind, QualifiedName, SYS_GRAPH, TO_ID, TableDesc};
use crate::db::State;
use crate::error::{Result, SqlError};

/// Whether `name` is that of a schema's implicit graph.
pub(in crate::db) fn is_sys_graph(name: &QualifiedName) -> bool {
    name.ident == SYS_GRAPH
}

/// The refusal of a statement that would define, alter or drop the
/// implicit graph `name`.
pub(in crate::db) fn sys_graph_refused(name: &QualifiedName) -> SqlError {
    graph_rule(
        None,
        format!("{name} is the implicit graph of the node and edge tables of schema {} (§4.2), which the database keeps", name.auth),
    )
}

impl State {
    /// SYS_GRAPH of each schema with a node table described anew, its
    /// grants kept, if its tables changed; and of a schema with none,
    /// dropped.
    ///
    /// # Errors
    ///
    /// As `describe_graph`'s, which a graph of node and edge tables meets.
    pub(in crate::db) fn refresh_sys_graphs(&mut self) -> Result<()> {
        let mut schemas: BTreeSet<String> =
            self.catalog.tables().into_iter().filter(|t| t.native().is_some()).map(|t| t.name.auth.clone()).collect();
        schemas.extend(self.catalog.graphs().into_iter().filter(|g| is_sys_graph(&g.name)).map(|g| g.name.auth.clone()));
        for auth in schemas {
            let qn = QualifiedName { auth: auth.clone(), ident: SYS_GRAPH.to_owned() };
            let Some(def) = self.sys_graph_definition(&qn) else {
                let mut catalog = self.catalog.duplicate();
                catalog.remove_graph(&qn);
                self.catalog = Arc::from(catalog);
                continue;
            };
            let mut desc = self.describe_graph(&def, Describing { names: &auth, horizon: None })?;
            if let Some(before) = self.catalog.graph(&qn) {
                if before.elements == desc.elements && before.reads == desc.reads {
                    continue;
                }
                desc.select.clone_from(&before.select);
            }
            self.put_graph(desc);
            self.graph_grants_standing(&qn);
        }
        Ok(())
    }

    /// The definition of SYS_GRAPH `qn`, if its schema has a node table.
    fn sys_graph_definition(&self, qn: &QualifiedName) -> Option<GraphDefinition> {
        let of = |kind: NativeKind| -> Vec<&TableDesc> {
            self.catalog.tables().into_iter().filter(|t| t.name.auth == qn.auth && t.native() == Some(kind)).collect()
        };
        let nodes = of(NativeKind::Node);
        if nodes.is_empty() {
            return None;
        }
        let vertices = nodes.iter().map(|n| element(n, (None, None), NODE_ID)).collect();
        let mut edges = Vec::new();
        for e in of(NativeKind::Edge) {
            let pairs: Vec<(&TableDesc, &TableDesc)> =
                nodes.iter().flat_map(|f| nodes.iter().map(move |t| (*f, *t))).filter(|(f, t)| joins(e, (&f.name, &t.name))).collect();
            let single = pairs.len() == 1;
            for (f, t) in pairs {
                let alias = (!single).then(|| format!("{} {} TO {}", e.name.ident, f.name.ident, t.name.ident));
                edges.push(element(e, (alias, Some((&f.name.ident, &t.name.ident))), EDGE_ID));
            }
        }
        let name = TableName { auth: Some(qn.auth.clone()), ident: qn.ident.clone(), at: 0 };
        let mode = Some(GraphModeOption::Trusted);
        Some(GraphDefinition { name, replace: false, if_not_exists: false, vertices, edges, mode, invoker: false })
    }
}

/// Whether an edge of `e` may join a node of `from` to one of `to`: each
/// of its CONNECTION constraints allows it.
fn joins(e: &TableDesc, (from, to): (&QualifiedName, &QualifiedName)) -> bool {
    e.connections().all(|(_, k)| k.allows(from, to))
}

/// The element table definition of `t`, of alias `alias` if not its name,
/// keyed by its pseudo-column `key`, of an edge between the node tables
/// `ends` names, labelled by its name, its properties its columns but the
/// pseudo-columns.
fn element(t: &TableDesc, (alias, ends): (Option<String>, Option<(&String, &String)>), key: &str) -> ElementDefinition {
    let hidden: Vec<String> = t.columns.iter().filter(|c| c.hidden()).map(|c| c.name.clone()).collect();
    let properties = if hidden.len() == t.columns.len() { PropertiesDefinition::None } else { PropertiesDefinition::AllColumns(hidden) };
    let endpoint = |column: &str, vertex: &String| EndpointDefinition {
        key: Some(vec![column.to_owned()]),
        vertex: (vertex.clone(), 0),
        references: Some(vec![NODE_ID.to_owned()]),
    };
    ElementDefinition {
        table: TableName { auth: Some(t.name.auth.clone()), ident: t.name.ident.clone(), at: 0 },
        alias: alias.map(|a| (a, 0)),
        key: Some(vec![key.to_owned()]),
        undirected: false,
        source: ends.map(|(f, _)| endpoint(FROM_ID, f)),
        destination: ends.map(|(_, to)| endpoint(TO_ID, to)),
        labels: vec![LabelDefinition { name: Some(t.name.ident.clone()), at: 0, properties: Some(properties) }],
        query: None,
    }
}
