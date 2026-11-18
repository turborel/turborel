//! The functions that generate sequences [XQueryFO §15.5]: `fn:id`,
//! `fn:idref`, `fn:doc`, `fn:doc-available` and `fn:collection`
//! (`op:to` is with the functions of sequences).
//!
//! \[DESIGN\] RULES.md D106: no query reads an external resource, as the
//! XML parser reads none: the available documents and the available
//! collections of the dynamic context are empty, and there is no default
//! collection, each an implementation-defined choice [XQueryFO §15.5.4,
//! §15.5.6]. So `fn:doc` of a URI is err:FODC0005, `fn:doc-available`
//! false, and `fn:collection` err:FODC0002 or err:FODC0004. An element's
//! or attribute's ID and IDREF properties are those its document type
//! declaration gave it [XQueryDM §5.5, §5.6].

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, boolean, items, optional_node, optional_string, or_context_node, strings};
use crate::error::Result;
use crate::xml::{Item, Kind, Node, NodeId, Sequence, Tree, Version, is_ncname, is_space};

/// The functions of [XQueryFO §15.5.2 to §15.5.6].
pub(super) const FUNCTIONS: [Builtin; 5] = [
    Builtin::function("id", 1, 2, id),
    Builtin::function("idref", 1, 2, idref),
    Builtin::function("doc", 1, 1, doc),
    Builtin::function("doc-available", 1, 1, doc_available),
    Builtin::function("collection", 0, 1, collection),
];

/// The document node of the tree of `args`'s second argument, or of the
/// context node if it is not given.
///
/// # Errors
///
/// err:FONC0001 or err:FOTY0011 if it is not given and the context item
/// is none or no node; err:FODC0001 if the tree's root is no document
/// node.
fn target(focus: &Context<'_>, args: &[Sequence]) -> Result<Node> {
    let node = optional_node(&or_context_node(args, 1, focus)?)?;
    node.map(|n| n.at(NodeId::ROOT))
        .filter(|root| root.kind() == Some(Kind::Document))
        .ok_or_else(|| error(ErrorCode::FODC0001, "the node is in no document [XQueryFO §15.5.2]"))
}

/// The tokens of `text`, white space its separator.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_space).filter(|t| !t.is_empty())
}

/// Whether element `id` of `tree` has the ID value `value`: of itself as
/// an ID, or of an attribute of it that is one [XQueryFO §15.5.2].
fn has_id(tree: &Tree, id: NodeId, value: &str) -> bool {
    let own = tree.is_id(id) && tree.string_value(id) == value;
    own || tree.attributes(id).any(|a| tree.is_id(a) && tree.content(a) == value)
}

/// [XQueryFO §15.5.2] `fn:id($arg as xs:string*, $node as node()) as
/// element()*`: for each token of the strings that is an NCName, the
/// first element of the document with it as its ID value; in document
/// order, each once.
fn id(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let root = target(focus, args)?;
    let wanted: Vec<String> = strings(arg(args, 0))?;
    let tree = root.tree();
    let mut found: Vec<NodeId> = Vec::new();
    for token in wanted.iter().flat_map(|s| tokens(s)).filter(|t| is_ncname(Version::V11, t)) {
        let first = tree.descendants(root.id()).find(|&e| tree.kind(e) == Some(Kind::Element) && has_id(tree, e, token));
        found.extend(first);
    }
    found.sort_unstable();
    found.dedup();
    items(found.into_iter().map(|e| Item::Node(root.at(e))).collect())
}

/// [XQueryFO §15.5.3] `fn:idref($arg as xs:string*, $node as node()) as
/// node()*`: the elements and attributes of the document that are IDREFs
/// with one of the strings that is an NCName among the tokens of their
/// values; in document order, each once.
fn idref(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let root = target(focus, args)?;
    let wanted: Vec<String> = strings(arg(args, 0))?.into_iter().filter(|s| is_ncname(Version::V11, s)).collect();
    let tree = root.tree();
    let refers = |n: NodeId| tree.is_idrefs(n) && tokens(&tree.string_value(n)).any(|t| wanted.iter().any(|w| w == t));
    let mut found = Vec::new();
    for e in tree.descendants(root.id()).filter(|&e| tree.kind(e) == Some(Kind::Element)) {
        if refers(e) {
            found.push(Item::Node(root.at(e)));
        }
        found.extend(tree.attributes(e).filter(|&a| refers(a)).map(|a| Item::Node(root.at(a))));
    }
    items(found)
}

/// [XQueryFO §15.5.4] `fn:doc($uri as xs:string?) as document-node()?`:
/// the empty sequence of the empty sequence.
///
/// # Errors
///
/// err:FODC0005 of a URI, as no document is available.
fn doc(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    match optional_string(arg(args, 0))? {
        None => Ok(Sequence::empty()),
        Some(uri) => Err(error(ErrorCode::FODC0005, &format!("no document is available at '{uri}' [XQueryFO §15.5.4]"))),
    }
}

/// [XQueryFO §15.5.5] `fn:doc-available($uri as xs:string?) as
/// xs:boolean`: false, as `fn:doc` returns no document node.
fn doc_available(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    optional_string(arg(args, 0))?;
    Ok(boolean(false))
}

/// [XQueryFO §15.5.6] `fn:collection($arg as xs:string?) as node()*`.
///
/// # Errors
///
/// err:FODC0002 if no URI is given, as there is no default collection;
/// err:FODC0004 of a URI, as no collection is available.
fn collection(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    match optional_string(arg(args, 0))? {
        None => Err(error(ErrorCode::FODC0002, "there is no default collection [XQueryFO §15.5.6]")),
        Some(uri) => Err(error(ErrorCode::FODC0004, &format!("no collection is available at '{uri}' [XQueryFO §15.5.6]"))),
    }
}

#[cfg(test)]
mod tests;
