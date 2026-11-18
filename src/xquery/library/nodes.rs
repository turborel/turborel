//! The functions and operators on nodes [XQueryFO §14]: a node's name,
//! local name and namespace URI, `fn:number`, `fn:lang`, `fn:root`, and
//! the operators of node identity and document order.
//!
//! \[DESIGN\] RULES.md D86: `fn:lang` compares by each character's full
//! lower case, of Unicode 13.0 (`unicode.rs`), as the caseless match of
//! [XQueryFO §14.5] reads; a node's root is the first node of its tree,
//! which every tree's root is.

use std::cmp::Ordering;

use super::accessors::name_of;
use super::args::{arg, atomized, context_node, optional, optional_node, optional_string, or_context, or_context_node, string_value};
use super::qnames::parts;
use super::unicode::{Map, Table};
use super::{Builtin, Focus};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Kind, Node, NodeId, Sequence, XML_NAMESPACE, cast};

/// The functions of [XQueryFO §14].
pub(super) const FUNCTIONS: [Builtin; 6] = [
    Builtin::function("name", 0, 1, name),
    Builtin::function("local-name", 0, 1, local_name),
    Builtin::function("namespace-uri", 0, 1, namespace_uri),
    Builtin::function("number", 0, 1, number),
    Builtin::function("lang", 1, 2, lang),
    Builtin::function("root", 0, 1, root),
];

/// The local name of the attribute `xml:lang`.
const LANG: &str = "lang";
/// What a subtag of a language follows.
const HYPHEN: char = '-';

/// The node of `args`'s argument `n`, or the context node if it is not
/// given; none of the empty sequence.
fn target(focus: &Focus, args: &[Sequence], n: usize) -> Result<Option<Node>> {
    optional_node(&or_context_node(args, n, focus)?)
}

/// [XQueryFO §14.1] `fn:name($arg as node()?) as xs:string`: its name as
/// a QName's lexical form; the zero-length string of a node of no name or
/// of the empty sequence.
///
/// # Errors
///
/// err:FONC0001 if no argument is given and there is no context item;
/// err:FOTY0011 if it is no node.
fn name(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let qname = target(focus, args, 0)?.as_ref().and_then(name_of);
    string_value(qname.map_or_else(String::new, |q| q.lexical.into_string()))
}

/// [XQueryFO §14.2] `fn:local-name($arg as node()?) as xs:string`: the
/// local part of its name.
///
/// # Errors
///
/// As `name`.
fn local_name(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let qname = target(focus, args, 0)?.as_ref().and_then(name_of);
    string_value(qname.map_or_else(String::new, |q| parts(&q).1.to_owned()))
}

/// [XQueryFO §14.3] `fn:namespace-uri($arg as node()?) as xs:anyURI`: the
/// namespace URI of an element's or attribute's name; the zero-length URI
/// of a name in no namespace, of another node or of the empty sequence.
///
/// # Errors
///
/// As `name`.
fn namespace_uri(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let node = target(focus, args, 0)?.filter(|n| matches!(n.kind(), Some(Kind::Element | Kind::Attribute)));
    let uri = node.as_ref().and_then(name_of).and_then(|q| q.namespace).unwrap_or_default();
    Ok(optional(Some(Atomic::new(AtomicType::AnyUri, uri))))
}

/// [XQueryFO §14.4] `fn:number($arg as xdt:anyAtomicType?) as xs:double`:
/// its value cast to a double, NaN if it cannot be or is the empty
/// sequence; of the context item atomized if it is not given.
///
/// # Errors
///
/// err:FONC0001 if no argument is given and there is no context item;
/// err:XPTY0004 of more than one value.
fn number(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let value = or_context(args, 0, focus)?;
    let mut values = atomized(&value);
    if values.len() > 1 {
        return Err(super::args::wrong_type("one atomic value", &format!("it is {} of them", values.len())));
    }
    let not_a_number = || Atomic::new(AtomicType::Double, "NaN");
    let double = values.pop().map_or_else(not_a_number, |a| cast(&a, AtomicType::Double).unwrap_or_else(|_| not_a_number()));
    Ok(optional(Some(double)))
}

/// `text` in lower case, as a caseless match compares it.
fn caseless(table: &Table, text: &str) -> String {
    text.chars().flat_map(|c| table.mapped(c, Map::Lower).unwrap_or_else(|| vec![c])).collect()
}

/// The value of the `xml:lang` attribute of `node` or its nearest
/// ancestor that has one [XQueryFO §14.5]; none if none has.
fn language_of(node: &Node) -> Option<String> {
    let tree = node.tree();
    std::iter::successors(Some(node.id()), |&id| tree.parent(id)).find_map(|id| {
        tree.attributes(id)
            .find(|&a| tree.name(a).is_some_and(|n| n.uri.as_deref() == Some(XML_NAMESPACE) && &*n.local == LANG))
            .map(|a| tree.content(a).to_owned())
    })
}

/// [XQueryFO §14.5] `fn:lang($testlang as xs:string?, $node as node()) as
/// xs:boolean`: whether the language of the node, or of the context node,
/// is `$testlang`, or one of its sublanguages, by a caseless match; false
/// if no `xml:lang` attribute gives it one.
///
/// # Errors
///
/// err:FONC0001 if no node is given and there is no context item;
/// err:FOTY0011 if it is no node; err:XPTY0004 of a node argument that
/// is none.
fn lang(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let node = match args.get(1) {
        Some(given) => super::args::node(given)?,
        None => context_node(focus)?,
    };
    let wanted = optional_string(arg(args, 0))?.unwrap_or_default();
    let Some(language) = language_of(&node) else { return Ok(super::args::boolean(false)) };
    let table = Table::read()?;
    let (language, wanted) = (caseless(&table, &language), caseless(&table, &wanted));
    let sublanguage = language.match_indices(HYPHEN).any(|(at, _)| language.get(..at) == Some(wanted.as_str()));
    Ok(super::args::boolean(language == wanted || sublanguage))
}

/// [XQueryFO §14.9] `fn:root($arg as node()?) as node()?`: the root of
/// its tree, of the context node if it is not given; the empty sequence of
/// the empty sequence.
///
/// # Errors
///
/// err:FONC0001 if no argument is given and there is no context item;
/// err:FOTY0011 if it is no node.
fn root(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(Sequence::new(target(focus, args, 0)?.map(|n| Item::Node(n.at(NodeId::ROOT))).into_iter().collect()))
}

/// [XQueryFO §14.6] `op:is-same-node`: whether `a` and `b` are the same
/// node [XQueryDM §2.3].
#[must_use]
pub fn is_same_node(a: &Node, b: &Node) -> bool {
    a.is(b)
}

/// [XQueryFO §14.7] `op:node-before`: whether `a` is before `b` in
/// document order [XQueryDM §2.4].
#[must_use]
pub fn node_before(a: &Node, b: &Node) -> bool {
    a.document_order(b) == Ordering::Less
}

/// [XQueryFO §14.8] `op:node-after`: whether `a` is after `b` in document
/// order.
#[must_use]
pub fn node_after(a: &Node, b: &Node) -> bool {
    a.document_order(b) == Ordering::Greater
}

#[cfg(test)]
mod tests;
