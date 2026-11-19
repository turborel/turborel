//! \[EXTENSION\] The descriptors of Mode C of the property graph
//! implementation standard (§4.3, RULES.md D144): a graph type, kept as
//! its definition's text and read again where it is used, as a property
//! graph's is; and a GQL graph, the property graph of its name whose
//! tables CREATE GRAPH generated, typed by a graph type or open.

use std::collections::HashMap;

use super::QualifiedName;

/// \[EXTENSION\] A graph type's descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTypeDesc {
    /// Its name, of the schema it is in.
    pub name: QualifiedName,
    /// Its CREATE GRAPH TYPE statement, as written.
    pub definition: String,
}

/// \[EXTENSION\] A GQL graph's descriptor: the property graph of its name
/// holds its element tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GqlGraphDesc {
    /// Its name, the property graph's.
    pub name: QualifiedName,
    /// The graph type it is typed by, closed; none if it is open.
    pub graph_type: Option<QualifiedName>,
}

/// \[EXTENSION\] The graph types and GQL graphs of a catalog, each by its
/// name.
#[derive(Debug, Clone, Default)]
pub struct GqlCatalog {
    /// Each graph type.
    types: HashMap<QualifiedName, GraphTypeDesc>,
    /// Each GQL graph.
    graphs: HashMap<QualifiedName, GqlGraphDesc>,
}

impl GqlCatalog {
    /// Record the graph type `t`, in place of any of its name.
    pub fn put_graph_type(&mut self, t: GraphTypeDesc) {
        self.types.insert(t.name.clone(), t);
    }

    /// The graph type of this name, if any.
    pub fn graph_type(&self, name: &QualifiedName) -> Option<&GraphTypeDesc> {
        self.types.get(name)
    }

    /// Every graph type, in the order of their names.
    pub fn graph_types(&self) -> Vec<&GraphTypeDesc> {
        let mut all: Vec<&GraphTypeDesc> = self.types.values().collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Forget the graph type of this name: one dropped.
    pub fn remove_graph_type(&mut self, name: &QualifiedName) {
        self.types.remove(name);
    }

    /// Record the GQL graph `g`, in place of any of its name.
    pub fn put_gql_graph(&mut self, g: GqlGraphDesc) {
        self.graphs.insert(g.name.clone(), g);
    }

    /// The GQL graph of this name, if any.
    pub fn gql_graph(&self, name: &QualifiedName) -> Option<&GqlGraphDesc> {
        self.graphs.get(name)
    }

    /// Every GQL graph, in the order of their names.
    pub fn gql_graphs(&self) -> Vec<&GqlGraphDesc> {
        let mut all: Vec<&GqlGraphDesc> = self.graphs.values().collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Forget the GQL graph of this name: one dropped.
    pub fn remove_gql_graph(&mut self, name: &QualifiedName) {
        self.graphs.remove(name);
    }
}
