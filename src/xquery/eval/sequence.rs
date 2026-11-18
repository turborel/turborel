//! What the evaluation does to sequences [XQuery §2.4]: atomization
//! [XQuery §2.4.2], the effective boolean value [XQuery §2.4.3], document
//! order with duplicates removed [XQuery §2.4.1], and the bound on a
//! sequence's length (RULES.md D18).

use std::num::FpCategory;

use super::super::error::{ErrorCode, beyond, error};
use super::Seq;
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Kind, MAX_NODES, Node, Primitive, double};

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

/// The typed value of `node` [XQuery §2.5.2]: of a node no schema
/// validated, its string value as `xdt:untypedAtomic`, but of a comment or
/// processing instruction as `xs:string`.
pub(super) fn typed_value(node: &Node) -> Atomic {
    let kind = match node.kind() {
        Some(Kind::Comment | Kind::ProcessingInstruction) => AtomicType::String,
        Some(Kind::Document | Kind::Element | Kind::Attribute | Kind::Text) | None => AtomicType::UntypedAtomic,
    };
    Atomic::new(kind, node.tree().string_value(node.id()))
}

/// `items` atomized [XQuery §2.4.2]: each atomic value itself, each node
/// its typed value.
pub(in crate::xquery) fn atomize(items: &[Item]) -> Vec<Atomic> {
    items
        .iter()
        .map(|item| match item {
            Item::Atomic(a) => a.clone(),
            Item::Node(node) => typed_value(node),
        })
        .collect()
}

/// The string value of `item` [XQuery §2.5.2]: a node's, or an atomic
/// value's string [XQueryFO §2.3].
pub(in crate::xquery) fn string_value(item: &Item) -> String {
    match item {
        Item::Atomic(a) => a.lexical.to_string(),
        Item::Node(node) => node.tree().string_value(node.id()),
    }
}

/// The effective boolean value of `items` [XQuery §2.4.3]: false of none;
/// true of a node first; of one boolean, itself; of one string or untyped
/// atomic value, whether it is not empty; of one number, whether it is
/// neither NaN nor zero; else err:FORG0006.
pub(in crate::xquery) fn ebv(items: &[Item]) -> Result<bool> {
    let atomic = match items {
        [] => return Ok(false),
        [Item::Node(_), ..] => return Ok(true),
        [Item::Atomic(a)] => a,
        [Item::Atomic(_), _, ..] => {
            return Err(error(
                ErrorCode::FORG0006,
                "the effective boolean value of more than one atomic value is undefined [XQuery §2.4.3]",
            ));
        }
    };
    let p = atomic.kind.primitive();
    if p == Primitive::Boolean {
        Ok(&*atomic.lexical == TRUE)
    } else if p.textual() {
        Ok(!atomic.lexical.is_empty())
    } else if p.numeric() {
        Ok(double(&atomic.lexical).is_some_and(|x| !matches!(x.classify(), FpCategory::Nan | FpCategory::Zero)))
    } else {
        Err(error(ErrorCode::FORG0006, &format!("a value of {} has no effective boolean value [XQuery §2.4.3]", atomic.kind.name())))
    }
}

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
