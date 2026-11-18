//! \[EXTENSION\] The vertices of a graph its rows give, each a key value
//! (RULES.md D-GRAPH-ALGO): numbered in the order of their keys, so a
//! run's result, and a component labelled by its least vertex, is the same
//! whatever order the rows come in.

use std::cmp::Ordering;

use super::csr::{ix, vertex};
use super::{Charge, Memory, Pair, Vertex};
use crate::error::{Result, SqlError};
use crate::storage::Row;
use crate::value::{Value, compare};

/// The places of an edge row's source and destination keys.
const ENDS: [usize; 2] = [0, 1];

/// The keys of a graph's vertices, in order, each's place its number.
pub(crate) struct Keys<'m> {
    keys: Vec<Value>,
    /// The bytes the keys hold, of the run's budget, held while they are.
    _held: Charge<'m>,
}

/// The order of two keys; of keys no comparison orders, as equal.
fn order(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap_or(Ordering::Equal)
}

/// GRF-0034, of a cost `what`.
fn cost_error(what: &str) -> SqlError {
    super::cost_refused(&format!("an edge's cost is {what}"))
}

impl<'m> Keys<'m> {
    /// The keys of `edges`' sources and destinations, of each with both not
    /// null, and of the first column of `vertices`, each once.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if they hold more than `memory` has left.
    pub(crate) fn of(edges: &[Row], vertices: Option<&[Row]>, memory: &'m Memory) -> Result<Keys<'m>> {
        let most = edges.len().saturating_mul(ENDS.len()).saturating_add(vertices.map_or(0, <[Row]>::len));
        let held = memory.items::<Value>(most, "the vertices' keys")?;
        let ends = edges.iter().filter(|r| ENDS.iter().all(|&e| r.get(e).is_some_and(|v| !v.is_null())));
        let mut keys: Vec<Value> = ends.flat_map(|r| ENDS.iter().filter_map(|&e| r.get(e)).cloned()).collect();
        keys.extend(vertices.unwrap_or_default().iter().filter_map(|r| r.first()).filter(|v| !v.is_null()).cloned());
        keys.sort_by(order);
        keys.dedup_by(|a, b| order(a, b) == Ordering::Equal);
        Ok(Keys { keys, _held: held })
    }

    /// How many vertices there are.
    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    /// The number of the vertex of key `key`, if one has it.
    pub(crate) fn find(&self, key: &Value) -> Option<Vertex> {
        self.keys.binary_search_by(|k| order(k, key)).ok().map(vertex)
    }

    /// The key of vertex `v`.
    fn key(&self, v: Vertex) -> Value {
        self.keys.get(ix(v)).cloned().unwrap_or(Value::Null)
    }

    /// The edges of `rows`, each of both keys not null, numbered, each's
    /// cost that of column `cost` if named, else 1.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 (GRF-0034) if a cost is null or not a number, or
    /// negative unless `negative`.
    pub(crate) fn edges(&self, rows: &[Row], cost: Option<usize>, negative: bool) -> Result<Vec<(Vertex, Vertex, f64)>> {
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let ends = (r.first().and_then(|s| self.find(s)), r.get(1).and_then(|d| self.find(d)));
            let (Some(s), Some(d)) = ends else { continue };
            let weight = match cost {
                Some(c) => r.get(c).and_then(Value::as_f64).ok_or_else(|| cost_error("null or not a number"))?,
                None => 1.0_f64,
            };
            if weight.is_nan() || (weight < 0.0_f64 && !negative) {
                return Err(cost_error(&format!("{weight}, where the algorithm takes no cost negative or not a number")));
            }
            out.push((s, d, weight));
        }
        Ok(out)
    }

    /// The rows of each vertex's key and its value, of each vertex that
    /// has one.
    pub(crate) fn valued(&self, values: impl IntoIterator<Item = Option<Value>>) -> Vec<Row> {
        self.keys.iter().zip(values).filter_map(|(k, v)| v.map(|v| vec![k.clone(), v])).collect()
    }

    /// The rows of each vertex's key and the key of its label's vertex.
    pub(crate) fn labelled(&self, labels: &[Vertex]) -> Vec<Row> {
        self.valued(labels.iter().map(|&l| Some(self.key(l))))
    }

    /// The rows of each pair's two keys and its similarity.
    pub(crate) fn paired(&self, pairs: Vec<Pair>) -> Vec<Row> {
        pairs.into_iter().map(|(a, b, s)| vec![self.key(a), self.key(b), Value::Approx(s)]).collect()
    }
}
