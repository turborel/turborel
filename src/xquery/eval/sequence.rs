//! What the evaluation does to sequences [XQuery §2.4]: atomization
//! [XQuery §2.4.2] and the effective boolean value [XQuery §2.4.3], the
//! library's (`xquery::library`), document order with duplicates removed
//! [XQuery §2.4.1], and the bound on a sequence's length (RULES.md D18).

use super::super::error::beyond;
use super::Seq;
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, MAX_NODES, Node};

/// [Schema2 §3.2.2.2] The canonical forms of `xs:boolean`.
pub(super) const TRUE: &str = "true";
/// The canonical form of false.
pub(super) const FALSE: &str = "false";

/// The error unless a sequence of `len` items is within `MAX_NODES`.
pub(super) fn bounded(len: usize) -> Result<()> {
    if len > MAX_NODES {
        return Err(beyond(&format!("a sequence has more than {MAX_NODES} items")));
    }
    Ok(())
}

/// `more` added to `out`, within `MAX_NODES` items.
pub(super) fn extend(out: &mut Seq, more: Seq) -> Result<()> {
    bounded(out.len().saturating_add(more.len()))?;
    if out.is_empty() {
        *out = more;
    } else {
        out.extend(more);
    }
    Ok(())
}

/// The sequence of the `xs:boolean` `b`.
pub(super) fn boolean(b: bool) -> Seq {
    vec![Item::Atomic(truth(b))]
}

/// The `xs:boolean` `b` [Schema2 §3.2.2.2].
pub(super) fn truth(b: bool) -> Atomic {
    Atomic::new(AtomicType::Boolean, if b { TRUE } else { FALSE })
}

pub(in crate::xquery) use super::super::library::args::atomize;
pub(in crate::xquery) use super::super::library::effective_boolean_value as ebv;

/// The nodes of `items`, if each is a node.
pub(super) fn nodes(items: Seq) -> Option<Vec<Node>> {
    items
        .into_iter()
        .map(|item| match item {
            Item::Node(node) => Some(node),
            Item::Atomic(_) => None,
        })
        .collect()
}

/// `nodes` in document order, each once [XQuery §2.4.1, §3.2].
pub(super) fn in_document_order(mut nodes: Vec<Node>) -> Seq {
    nodes.sort_by(Node::document_order);
    nodes.dedup_by(|a, b| a.is(b));
    nodes.into_iter().map(Item::Node).collect()
}
