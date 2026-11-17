//! Tests of XML values as sequences.

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
    let atomic = Item::Atomic(Atomic { kind: AtomicType::UntypedAtomic, lexical: "1".into() });
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
