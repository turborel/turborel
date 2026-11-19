//! The operators that combine sequences of nodes [XQueryFO §15.3.2 to
//! §15.3.4]: `op:union`, `op:intersect` and `op:except`, each giving its
//! nodes in document order, each node once, two nodes the same if they
//! are by `op:is-same-node`.

use super::args::{items, wrong_type};
use crate::error::Result;
use crate::xml::{Item, Node, Sequence};

/// The nodes of `value`.
///
/// # Errors
///
/// err:XPTY0004 of an atomic value among them.
fn nodes(value: &Sequence) -> Result<Vec<Node>> {
    value
        .items()
        .iter()
        .map(|item| match item {
            Item::Node(n) => Ok(n.clone()),
            Item::Atomic(a) => Err(wrong_type("a node", &format!("it is an {}", a.kind.name()))),
        })
        .collect()
}

/// `nodes` in document order, each once.
fn ordered(mut nodes: Vec<Node>) -> Result<Sequence> {
    nodes.sort_by(Node::document_order);
    nodes.dedup_by(|a, b| a.is(b));
    items(nodes.into_iter().map(Item::Node).collect())
}

/// `nodes` in document order, each once, to be searched.
fn sorted(mut nodes: Vec<Node>) -> Vec<Node> {
    nodes.sort_by(Node::document_order);
    nodes.dedup_by(|a, b| a.is(b));
    nodes
}

/// Whether `node` is among `others`, in document order.
fn among(node: &Node, others: &[Node]) -> bool {
    others.binary_search_by(|o| o.document_order(node)).is_ok()
}

/// [XQueryFO §15.3.2] `op:union`: every node of either.
///
/// # Errors
///
/// err:XPTY0004 of an atomic value.
pub fn union(a: &Sequence, b: &Sequence) -> Result<Sequence> {
    let mut all = nodes(a)?;
    all.extend(nodes(b)?);
    ordered(all)
}

/// [XQueryFO §15.3.3] `op:intersect`: every node of both.
///
/// # Errors
///
/// As `union`.
pub fn intersect(a: &Sequence, b: &Sequence) -> Result<Sequence> {
    let theirs = sorted(nodes(b)?);
    ordered(nodes(a)?.into_iter().filter(|n| among(n, &theirs)).collect())
}

/// [XQueryFO §15.3.4] `op:except`: every node of the first not of the
/// second.
///
/// # Errors
///
/// As `union`.
pub fn except(a: &Sequence, b: &Sequence) -> Result<Sequence> {
    let theirs = sorted(nodes(b)?);
    ordered(nodes(a)?.into_iter().filter(|n| !among(n, &theirs)).collect())
}

#[cfg(test)]
mod tests;
