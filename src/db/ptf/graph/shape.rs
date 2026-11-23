//! \[EXTENSION\] The rows of a property graph named in place of the edges
//! of a function of GRAPH_ALGO (RULES.md D145), made into the
//! edges and vertices the library reads: each vertex is its identity
//! [GS §5 R5.1], its element table and its key, so keys of two vertex
//! tables, of any number of columns, never meet.
//!
//! An identity is ELEMENT_ID's text, `{"table":"ACCOUNT","key":[6]}`, made
//! by the one function that makes ELEMENT_ID's (`logical/graph/identity.rs`),
//! so the result joins to a GRAPH_TABLE's ELEMENT_ID. Of a
//! schema's SYS_GRAPH, a vertex is its `$NODE_ID`, an identity of its own
//! that its node table holds (§4.2).

use crate::error::Result;
use crate::logical::identity_text;
use crate::storage::Row;
use crate::value::Value;

/// \[EXTENSION\] How the table arguments of a graph named in place of the
/// edges are read: the rows of each edge table, then of each vertex table.
#[derive(Debug, Clone)]
pub(crate) struct GraphShape {
    /// Of each edge table, its rows' shape, in the order of its plan.
    pub(crate) edges: Vec<EdgeShape>,
    /// Of each vertex table, its alias, in the order of its plan, after
    /// the edge tables'.
    pub(crate) vertices: Vec<String>,
    /// Whether it is a SYS_GRAPH, each vertex its `$NODE_ID`.
    pub(crate) native: bool,
}

/// \[EXTENSION\] The rows of an edge table: its source's key, its
/// destination's, then the columns its graph's edge tables have alike.
#[derive(Debug, Clone)]
pub(crate) struct EdgeShape {
    /// The alias of its source's vertex table and its key's columns.
    pub(crate) source: (String, usize),
    /// The alias of its destination's vertex table and its key's columns.
    pub(crate) destination: (String, usize),
    /// Whether its edges are undirected, so each joins its ends both ways.
    pub(crate) undirected: bool,
}

impl GraphShape {
    /// The identity of the vertex of table `alias` whose key is `key`;
    /// none if a value of the key is null, which no vertex has.
    fn identity(&self, alias: &str, key: &[Value]) -> Result<Option<Value>> {
        if key.iter().any(Value::is_null) {
            return Ok(None);
        }
        if let (true, [id]) = (self.native, key) {
            return Ok(Some(id.clone()));
        }
        Ok(Some(Value::Char(identity_text(alias, key)?.into())))
    }

    /// The edges and the vertices of `inputs`, the rows of its edge tables
    /// then its vertex tables': each edge its ends' identities and the
    /// columns alike, an undirected one both ways; each vertex its
    /// identity.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an identity cannot be serialized.
    pub(crate) fn rows(&self, inputs: &[Vec<Row>]) -> Result<(Vec<Row>, Vec<Row>)> {
        let mut edges = Vec::new();
        for (shape, rows) in self.edges.iter().zip(inputs) {
            let ((from, a), (to, b)) = (&shape.source, &shape.destination);
            for row in rows {
                let (keys, rest) = row.split_at((*a).saturating_add(*b).min(row.len()));
                let (source, destination) = keys.split_at((*a).min(keys.len()));
                let (Some(s), Some(d)) = (self.identity(from, source)?, self.identity(to, destination)?) else { continue };
                if shape.undirected {
                    edges.push([d.clone(), s.clone()].into_iter().chain(rest.iter().cloned()).collect());
                }
                edges.push([s, d].into_iter().chain(rest.iter().cloned()).collect());
            }
        }
        let mut vertices = Vec::new();
        for (alias, rows) in self.vertices.iter().zip(inputs.iter().skip(self.edges.len())) {
            for row in rows {
                vertices.extend(self.identity(alias, row)?.map(|v| vec![v]));
            }
        }
        Ok((edges, vertices))
    }
}
