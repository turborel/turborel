//! Tests of the stored form of XML values.

use std::sync::Arc;

use super::*;
use crate::xml::serialize::{Parameters, serialize};
use crate::xml::{Syntax, Version, Whitespace, parse};

const TEXT: Parameters = Parameters { version: Version::V11, declaration: false, encoding: "UTF-8", undeclare: true };

fn tree(text: &str) -> Arc<Tree> {
    Arc::new(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())
}

/// `value` stored and read back.
fn again(value: &Sequence) -> Sequence {
    decode(&encode(value)).unwrap()
}

fn text_of(value: &Sequence) -> String {
    serialize(value, &TEXT).unwrap()
}

fn corrupt(bytes: &[u8]) -> String {
    let error = decode(bytes).unwrap_err();
    assert_eq!(error.sqlcode(), -916_i32, "{error}");
    error.to_string()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.2 GR1b, §10.18] [DESIGN]
/// RULES.md D86: a value stored and read back is the same value: its
/// documents, elements, attributes with their ID flags, namespace
/// bindings, text, comments and processing instructions, and its atomic
/// values, in order.
#[test]
fn a_value_reads_back_as_stored() {
    let text = "<?xml version='1.1'?><!--c--><a xmlns='d' xmlns:p='u' p:x='1'><p:b/>t&#1;<?p d?><c xmlns:p=''/></a>";
    let document = Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap());
    let read = again(&document);
    assert_eq!(text_of(&read), text_of(&document));
    assert!(read.is_document());
    let ids = Sequence::document(parse("<a xml:id='k'/>", Syntax::Document, Whitespace::Preserve).unwrap());
    let back = again(&ids);
    let Item::Node(node) = &back.items()[0] else { panic!("a node") };
    let a = node.tree().children(NodeId::ROOT).next().unwrap();
    assert!(node.tree().is_id(node.tree().attributes(a).next().unwrap()));
    let atomics = Sequence::new(vec![
        Item::Atomic(Atomic { kind: AtomicType::String, lexical: "x y".into() }),
        Item::Atomic(Atomic { kind: AtomicType::UntypedAtomic, lexical: "1".into() }),
    ]);
    let read_atomics = again(&atomics);
    let [Item::Atomic(first), Item::Atomic(second)] = read_atomics.items() else { panic!("two atomic values") };
    assert_eq!(first, &Atomic { kind: AtomicType::String, lexical: "x y".into() });
    assert_eq!((second.kind, &*second.lexical), (AtomicType::UntypedAtomic, "1"));
    assert_eq!(again(&Sequence::default()).items().len(), 0);
    assert_eq!(encode(&Sequence::default()), [FORMAT_VERSION, 0, 0, 0, 0]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.18] [XQueryDM §6.2.1]
/// [DESIGN] RULES.md D86: a node below the root of its tree is stored as
/// a copy whose root has no parent, an element declaring every binding in
/// its scope; a text, comment, processing instruction or attribute alone
/// is stored as itself.
#[test]
fn nodes_below_a_root_are_stored_as_copies() {
    let source = tree("<a xmlns:p='u' x='v'><p:b>t</p:b><!--c--><?q r?></a>");
    let a = source.children(NodeId::ROOT).next().unwrap();
    let b = source.first_child(a).unwrap();
    let parts: Vec<NodeId> = vec![
        b,
        source.first_child(b).unwrap(),
        source.next_sibling(b).unwrap(),
        source.children(a).nth(2).unwrap(),
        source.attributes(a).next().unwrap(),
    ];
    let value = Sequence::new(parts.iter().map(|&id| Item::Node(Node::new(Arc::clone(&source), id))).collect());
    let read = again(&value);
    let kinds: Vec<Option<Kind>> = read.items().iter().map(|i| if let Item::Node(n) = i { n.kind() } else { None }).collect();
    assert_eq!(
        kinds,
        [Some(Kind::Element), Some(Kind::Text), Some(Kind::Comment), Some(Kind::ProcessingInstruction), Some(Kind::Attribute)]
    );
    let element = Sequence::new(read.items()[..4].to_vec());
    assert_eq!(text_of(&element), "<p:b xmlns:p=\"u\">t</p:b>t<!--c--><?q r?>");
    let Item::Node(attribute) = &read.items()[4] else { panic!("a node") };
    assert_eq!((attribute.tree().name(NodeId::ROOT).map(|n| &*n.local), attribute.tree().content(NodeId::ROOT)), (Some("x"), "v"));
    assert_eq!(attribute.tree().parent(NodeId::ROOT), None);
}

/// Verifies: [DESIGN] RULES.md D30, D86: bytes that are no stored form of
/// version 1 are corruption, SQLCODE -916: another version, a field past
/// the end, an unknown tag, a name not numbered, events that make no
/// tree, or bytes after the last item.
#[test]
fn corrupt_bytes_are_refused() {
    let good = encode(&Sequence::document(parse("<a>t</a>", Syntax::Document, Whitespace::Preserve).unwrap()));
    assert!(corrupt(&[2, 0, 0, 0, 0]).contains("version is 2"));
    assert!(corrupt(&good[..good.len() - 1]).contains(WHAT));
    assert!(corrupt(&[good.as_slice(), &[0]].concat()).contains(WHAT));
    assert!(corrupt(&[1, 1, 0, 0, 0, 9]).contains("an item's tag is 9"));
    assert!(corrupt(&[1, 1, 0, 0, 0, ATOMIC, 13]).contains("type is 13"));
    let no_names = [1, 1, 0, 0, 0, NODE, 0, 0, 0, 0];
    assert!(corrupt(&[&no_names[..], &[ELEMENT, 5, 0, 0, 0]].concat()).contains("the name 5 is not numbered"));
    assert!(corrupt(&[&no_names[..], &[9]].concat()).contains("an event's tag is 9"));
    assert!(corrupt(&[&no_names[..], &[DOCUMENT, DOCUMENT]].concat()).contains("an event's tag is 1"));
    assert!(corrupt(&[&no_names[..], &[END]].concat()).contains("closes no node"));
    let one_name = [1, 1, 0, 0, 0, NODE, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'a'];
    let attribute_in_document = [&one_name[..], &[DOCUMENT, ATTRIBUTE, 0, 0, 0, 0, 0, 0, 0, 0, 0, END]].concat();
    assert!(corrupt(&attribute_in_document).contains("make no tree"));
    let unclosed = [&one_name[..], &[ELEMENT, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]].concat();
    assert!(corrupt(&unclosed).contains(WHAT));
}
