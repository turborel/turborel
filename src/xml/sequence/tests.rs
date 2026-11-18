//! Tests of XML values as sequences.

use std::cmp::Ordering;
use std::sync::Arc;

use super::*;
use crate::xml::{Syntax, Whitespace, parse};

fn content(text: &str) -> Sequence {
    Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3, §10.2 GR1a] a value is
/// content if it is one document node, and a document if that node's
/// children are one element and comments and processing instructions.
#[test]
fn documents_and_content() {
    for text in ["<a/>", "<!--c--><a/><?p?>"] {
        assert!(content(text).is_document() && content(text).is_content(), "{text}");
    }
    for text in ["", "t", "<a/><b/>", "<a/>t", "<!--c-->"] {
        assert!(!content(text).is_document() && content(text).is_content(), "{text}");
    }
    let atomic = Item::Atomic(Atomic::new(AtomicType::UntypedAtomic, "1"));
    assert!(!Sequence::new(vec![atomic]).is_content());
    let two = Sequence::new([content("<a/>").items(), content("<b/>").items()].concat());
    assert!(!two.is_content() && !two.is_document());
    assert!(!Sequence::default().is_content());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.6 GR1c] [XQueryDM §2.3] two
/// nodes are the same node if they are one place of one tree; a copy of
/// a tree is another tree.
#[test]
fn node_identity() {
    let tree = Arc::new(parse("<a><b/></a>", Syntax::Document, Whitespace::Preserve).unwrap());
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let first = Node::new(Arc::clone(&tree), a);
    assert!(first.is(&Node::new(Arc::clone(&tree), a)));
    assert!(!first.is(&Node::new(Arc::clone(&tree), NodeId::ROOT)));
    let copy = Node::new(Arc::new((*tree).clone()), a);
    assert!(!first.is(&copy));
    assert_eq!((first.kind(), first.id()), (Some(Kind::Element), a));
    assert_eq!(first.tree().len(), tree.len());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [XQueryDM §2.4] nodes of
/// a tree are in document order, the root first, an element before its
/// attributes and children; every node of one tree is on one side of every
/// node of another, the same each time.
#[test]
fn document_order_within_and_across_trees() {
    let tree = Arc::new(parse("<a x='1'><b/></a>", Syntax::Document, Whitespace::Preserve).unwrap());
    let root = Node::new(Arc::clone(&tree), NodeId::ROOT);
    let a = root.at(tree.children(NodeId::ROOT).next().unwrap());
    let x = a.at(tree.attributes(a.id()).next().unwrap());
    let b = a.at(tree.children(a.id()).next().unwrap());
    assert_eq!(root.document_order(&a), Ordering::Less);
    assert_eq!((a.document_order(&x), x.document_order(&b)), (Ordering::Less, Ordering::Less));
    assert_eq!(b.document_order(&b), Ordering::Equal);
    let other = Node::new(Arc::new((*tree).clone()), NodeId::ROOT);
    let side = b.document_order(&other);
    assert_ne!(side, Ordering::Equal);
    assert_eq!(root.document_order(&other), side);
    assert_eq!(other.document_order(&a), side.reverse());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4c] [XQueryDM §5.15] a
/// node's typed value: an element's or attribute's string value untyped,
/// a comment's content a string.
#[test]
fn typed_values_of_nodes() {
    let tree = Arc::new(parse("<a x='1'>t<b>u</b><!--c--></a>", Syntax::Document, Whitespace::Preserve).unwrap());
    let a = Node::new(Arc::clone(&tree), tree.children(NodeId::ROOT).next().unwrap());
    assert_eq!(a.typed_value().ok(), Some(vec![Atomic::new(AtomicType::UntypedAtomic, "tu")]));
    let x = a.at(tree.attributes(a.id()).next().unwrap());
    assert_eq!(x.typed_value().ok(), Some(vec![Atomic::new(AtomicType::UntypedAtomic, "1")]));
    let comment = a.at(tree.children(a.id()).last().unwrap());
    assert_eq!(comment.typed_value().ok(), Some(vec![Atomic::new(AtomicType::String, "c")]));
}
