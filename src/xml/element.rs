//! XQuery's computed element constructor, `element { $QNAME } { $SEQ }`
//! [XQuery §3.7.3.1], as XMLELEMENT and XMLFOREST evaluate it [ISO/IEC
//! 9075-14:2006 draft §10.13 GR4c viii].
//!
//! \[DESIGN\] RULES.md D86: the content is read once into pieces, its
//! attributes, text and nodes to copy, and the element built from them
//! with `Builder`, each node copied by the tree's links, so nothing
//! recurses.

use super::sequence::{Item, Node, Sequence};
use super::tree::{Annotation, Binding, Builder, ElementStart, Kind, NodeId, QName, Specified, Tree, XML_PREFIX};
use crate::error::{Code, Result, SqlError, SqlState};

/// The one separator XQuery puts between adjacent atomic values made a
/// text node [XQuery §3.7.1.3].
const SEPARATOR: &str = " ";

/// \[DESIGN\] What a prefix XQuery makes of its own is [XQuery §3.7.4]:
/// this, then a number.
const MADE_PREFIX: &str = "ns";

/// A part of an element's content sequence [XQuery §3.7.1.3].
enum Piece<'a> {
    /// An attribute.
    Attribute(Specified),
    /// Text, adjacent text and atomic values joined.
    Text(String),
    /// A node of a tree to copy: an element, comment or processing
    /// instruction.
    Copy(&'a Tree, NodeId),
}

/// The XQuery error `code` of the element constructor, `what` its
/// cause, raised as SQLSTATE 10000 [§10.13 GR4c viii].
fn xquery_error(what: &str, code: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §10.13 GR4c viii] XQuery error: {what} [XQuery §3.7.1.3] {code}"))
        .with_state(SqlState::XQUERY_ERROR)
}

/// Attribute `id` of `tree`, copied as construction mode preserve keeps
/// it [XQuery §3.7.1.3].
fn copied_attribute(tree: &Tree, id: NodeId) -> Specified {
    let name = tree.name(id).cloned().unwrap_or_else(|| QName::local(""));
    Specified { name, value: tree.content(id).to_owned(), is_id: tree.is_id(id), is_idrefs: tree.is_idrefs(id) }
}

/// `piece` added after `out`'s pieces: text to text before it.
fn push<'a>(out: &mut Vec<Piece<'a>>, piece: Piece<'a>) {
    if let (Some(Piece::Text(before)), Piece::Text(more)) = (out.last_mut(), &piece) {
        before.push_str(more);
        return;
    }
    out.push(piece);
}

/// The pieces of node `node`: a document node's children, else itself.
fn node_pieces<'a>(out: &mut Vec<Piece<'a>>, node: &'a Node) {
    let tree = node.tree();
    let ids: Vec<NodeId> = if node.kind() == Some(Kind::Document) { tree.children(node.id()).collect() } else { vec![node.id()] };
    for id in ids {
        let piece = match tree.kind(id) {
            Some(Kind::Attribute) => Piece::Attribute(copied_attribute(tree, id)),
            Some(Kind::Text) => Piece::Text(tree.content(id).to_owned()),
            Some(Kind::Element | Kind::Comment | Kind::ProcessingInstruction | Kind::Document) | None => Piece::Copy(tree, id),
        };
        push(out, piece);
    }
}

/// The pieces of `content`, in order: each run of adjacent atomic values
/// a text of their lexical forms a space apart; a document node its
/// children; a text node its text; adjacent text one, and none empty
/// [XQuery §3.7.1.3].
fn pieces<'a>(content: &[&'a Sequence]) -> Vec<Piece<'a>> {
    let mut out: Vec<Piece<'a>> = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    for item in content.iter().flat_map(|s| s.items()) {
        match item {
            Item::Atomic(a) => run.push(&a.lexical),
            Item::Node(node) => {
                if !run.is_empty() {
                    push(&mut out, Piece::Text(run.join(SEPARATOR)));
                    run.clear();
                }
                node_pieces(&mut out, node);
            }
        }
    }
    if !run.is_empty() {
        push(&mut out, Piece::Text(run.join(SEPARATOR)));
    }
    out.retain(|p| if let Piece::Text(t) = p { !t.is_empty() } else { true });
    out
}

/// A prefix of no binding in `bindings`, made from `like`, else
/// `MADE_PREFIX`, and a number [XQuery §3.7.4].
fn made_prefix(bindings: &[Binding], like: Option<&str>) -> Box<str> {
    let stem = like.unwrap_or(MADE_PREFIX);
    let taken = |p: &str| p == XML_PREFIX || bindings.iter().any(|b| b.prefix.as_deref() == Some(p));
    let mut n = 1_usize;
    loop {
        let candidate = format!("{stem}{n}");
        if !taken(&candidate) {
            return candidate.into();
        }
        n = n.saturating_add(1);
    }
}

/// `name` bound in `bindings` [XQuery §3.7.4]: a binding added for its
/// prefix, or the default namespace if it has none and is an element's,
/// unless one binds it to its URI; its prefix changed to one made if a
/// binding names another URI, or it is an attribute's in a namespace
/// with none. A name in no namespace has no prefix; one of `xml` is
/// bound already.
fn bind(bindings: &mut Vec<Binding>, name: &mut QName, attribute: bool) {
    let Some(uri) = name.uri.clone().filter(|u| !u.is_empty()) else {
        name.prefix = None;
        return;
    };
    if name.prefix.as_deref() == Some(XML_PREFIX) {
        return;
    }
    if attribute && name.prefix.is_none() {
        name.prefix = Some(made_prefix(bindings, None));
    }
    let bound = bindings.iter().find(|b| b.prefix == name.prefix).map(|b| b.uri.clone());
    if bound.as_ref() == Some(&uri) {
        return;
    }
    if bound.is_some() {
        name.prefix = Some(made_prefix(bindings, name.prefix.as_deref()));
    }
    bindings.push(Binding { prefix: name.prefix.clone(), uri });
}

/// [§10.13 GR4c iv to ix] The element `element { $QNAME } { $SEQ }` makes
/// [XQuery §3.7.3.1], $QNAME `name` and $SEQ `attributes` then the items
/// of `content`, standing alone: its attributes those of $SEQ, before any
/// other node of it; its children copies of the other nodes and text of
/// the atomic values [XQuery §3.7.1.3]; its bindings those its names need
/// [XQuery §3.7.4].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, if an attribute follows another node
/// \[err:XQTY0024\] or two attributes have one name \[err:XQDY0025\];
/// SQLCODE -900 if the tree would be beyond turborel's limits.
pub fn element(mut name: QName, mut attributes: Vec<Specified>, content: &[&Sequence]) -> Result<Sequence> {
    let mut children = Vec::new();
    for piece in pieces(content) {
        match piece {
            Piece::Attribute(a) => {
                if !children.is_empty() {
                    return Err(xquery_error("an attribute follows a node that is not one", "err:XQTY0024"));
                }
                attributes.push(a);
            }
            other @ (Piece::Text(_) | Piece::Copy(..)) => children.push(other),
        }
    }
    for (i, a) in attributes.iter().enumerate() {
        if attributes.iter().skip(i.saturating_add(1)).any(|b| b.name.same(&a.name)) {
            return Err(xquery_error(&format!("two attributes are named {}", a.name.local), "err:XQDY0025"));
        }
    }
    let mut bindings = Vec::new();
    bind(&mut bindings, &mut name, false);
    for a in &mut attributes {
        bind(&mut bindings, &mut a.name, true);
    }
    let mut builder = Builder::parentless();
    builder.start_element(ElementStart { name, bindings, attributes })?;
    // [§10.13 GR4c viii] `element { $QNAME } { $SEQ }` in the static
    // context of §10.20, whose construction mode is preserve [XQuery
    // Appendix C.1], annotates the element `xs:anyType` [XQuery §3.7.3.1].
    if let Some(element) = builder.current() {
        builder.annotate(element, &Annotation::any_type(), (false, false, false))?;
    }
    for child in children {
        match child {
            Piece::Text(text) => builder.text(&text)?,
            Piece::Copy(tree, id) => builder.copy(tree, id)?,
            Piece::Attribute(_) => {}
        }
    }
    builder.end_element()?;
    Ok(Sequence::new(vec![Item::Node(Node::root(builder.finish()?))]))
}

#[cfg(test)]
mod tests;
