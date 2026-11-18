//! The accessors [XQueryFO §2]: `fn:node-name`, `fn:nilled`, `fn:string`,
//! `fn:data`, `fn:base-uri` and `fn:document-uri`, each a function of the
//! data model's accessor of its name [XQueryDM §5].
//!
//! \[DESIGN\] RULES.md D106: a tree keeps no base URI and no document URI
//! of its own, as XMLPARSE and the constructors give it none: a document
//! read from a string has none [Infoset §1]. So a node's base URI is what
//! the `xml:base` attributes of its element and that element's ancestors
//! make, by which \[Infoset\] computes the property [Infoset §1], each
//! resolved against the one above it, none if none has one; a document
//! node's document URI is none.

use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, atomized, items, optional, optional_node, or_context, or_context_node, single, typed};
use super::uri::{is_absolute, resolve};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Kind, Node, Sequence, XML_NAMESPACE, cast};

/// The local name of the attribute `xml:base` [XQueryDM §6.2.2].
const BASE: &str = "base";

/// The functions of [XQueryFO §2].
pub(super) const FUNCTIONS: [Builtin; 6] = [
    Builtin::function("node-name", 1, 1, node_name),
    Builtin::function("nilled", 1, 1, nilled),
    Builtin::function("string", 0, 1, string),
    Builtin::function("data", 1, 1, data),
    Builtin::function("base-uri", 0, 1, base_uri),
    Builtin::function("document-uri", 1, 1, document_uri),
];

/// [XQueryDM §5.11] The name of `node` as an `xs:QName`: an element's or
/// attribute's, a processing instruction's target in no namespace; none of
/// another node.
pub(super) fn name_of(node: &Node) -> Option<Atomic> {
    match node.kind()? {
        Kind::Element | Kind::Attribute | Kind::ProcessingInstruction => {}
        Kind::Document | Kind::Text | Kind::Comment => return None,
    }
    let name = node.tree().name(node.id())?;
    let lexical = name.prefix.as_ref().map_or_else(|| name.local.to_string(), |p| format!("{p}:{}", name.local));
    Some(Atomic::qname(name.uri.as_deref(), &lexical))
}

/// [XQueryFO §2.1] `fn:node-name($arg as node()?) as xs:QName?`.
fn node_name(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    Ok(optional(optional_node(arg(args, 0))?.as_ref().and_then(name_of)))
}

/// [XQueryFO §2.2] `fn:nilled($arg as node()?) as xs:boolean?`: of an
/// element, its nilled property, true of one a validation found nilled,
/// false of every other [XQueryDM §5.9, §6.2.3]; the empty sequence of
/// another node.
fn nilled(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let element = optional_node(arg(args, 0))?.filter(|n| n.kind() == Some(Kind::Element));
    let truth = |n: Node| if n.tree().nilled(n.id()) { "true" } else { "false" };
    Ok(optional(element.map(|n| Atomic::new(AtomicType::Boolean, truth(n)))))
}

/// [XQueryFO §2.3] The string of `item`: a node's string value [XQueryDM
/// §5.13], an atomic value cast to `xs:string` [XQueryFO §17.1.2].
pub(super) fn string_of(item: &Item) -> Result<String> {
    Ok(match item {
        Item::Node(node) => node.tree().string_value(node.id()),
        Item::Atomic(a) => cast(a, AtomicType::String)?.lexical.into_string(),
    })
}

/// [XQueryFO §2.3] `fn:string($arg as item()?) as xs:string`: the string
/// of its argument, or of the context item if none is given; the
/// zero-length string of the empty sequence.
///
/// # Errors
///
/// err:FONC0001 if no argument is given and there is no context item;
/// err:XPTY0004 of more than one item.
fn string(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let value = or_context(args, 0, focus)?;
    let text = match value.items() {
        [] => String::new(),
        [item] => string_of(item)?,
        [_, ..] => return Err(super::args::wrong_type("one item", &format!("it is {} items", value.items().len()))),
    };
    Ok(single(typed(AtomicType::String, text)?))
}

/// [XQueryFO §2.4] `fn:data($arg as item()*) as xdt:anyAtomicType*`: each
/// atomic value itself, each node its typed value; every node of a tree
/// no schema typed has one.
fn data(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    items(atomized(arg(args, 0))?.into_iter().map(Item::Atomic).collect())
}

/// [XQueryDM §5.2, §6.2.3] The base URI of `node`: what the `xml:base` attributes
/// of its nearest element and that element's ancestors make, each
/// resolved against the one above it, an absolute one replacing the rest;
/// none if none has one.
pub(super) fn base_of(node: &Node) -> Option<String> {
    let tree = node.tree();
    let start = if node.kind() == Some(Kind::Element) { Some(node.id()) } else { tree.parent(node.id()) };
    let mut chain: Vec<_> = std::iter::successors(start, |&id| tree.parent(id)).collect();
    chain.reverse();
    let mut base: Option<String> = None;
    for id in chain {
        let declared =
            tree.attributes(id).find(|&a| tree.name(a).is_some_and(|n| n.uri.as_deref() == Some(XML_NAMESPACE) && &*n.local == BASE));
        let Some(value) = declared.map(|a| tree.content(a)) else { continue };
        base = Some(match base.filter(|b| is_absolute(b)) {
            Some(above) => resolve(value, &above).unwrap_or_else(|| value.to_owned()),
            None => value.to_owned(),
        });
    }
    base
}

/// [XQueryFO §2.5] `fn:base-uri($arg as node()?) as xs:anyURI?`: the base
/// URI of its argument, or of the context item if none is given.
///
/// # Errors
///
/// err:FONC0001 if no argument is given and there is no context item;
/// err:FOTY0011 if it is not a node.
fn base_uri(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let Some(node) = optional_node(&or_context_node(args, 0, focus)?)? else { return Ok(Sequence::empty()) };
    Ok(optional(base_of(&node).map(|uri| Atomic::new(AtomicType::AnyUri, uri))))
}

/// [XQueryFO §2.6] `fn:document-uri($arg as node()?) as xs:anyURI?`: the
/// empty sequence, as no document node here has a document URI.
fn document_uri(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    optional_node(arg(args, 0))?;
    Ok(Sequence::empty())
}

#[cfg(test)]
mod tests;
