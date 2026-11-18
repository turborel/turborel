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
/// RULES.md D106: a value stored and read back is the same value: its
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
        Item::Atomic(Atomic::new(AtomicType::String, "x y")),
        Item::Atomic(Atomic::new(AtomicType::UntypedAtomic, "1")),
    ]);
    let read_atomics = again(&atomics);
    let [Item::Atomic(first), Item::Atomic(second)] = read_atomics.items() else { panic!("two atomic values") };
    assert_eq!(first, &Atomic::new(AtomicType::String, "x y"));
    assert_eq!((second.kind, &*second.lexical), (AtomicType::UntypedAtomic, "1"));
    assert_eq!(again(&Sequence::default()).items().len(), 0);
    assert_eq!(encode(&Sequence::default()), [FORMAT_VERSION, 0, 0, 0, 0]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.18] [XQueryDM §6.2.1]
/// [DESIGN] RULES.md D106: a node below the root of its tree is stored as
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

/// Verifies: [DESIGN] RULES.md D30, D106: bytes that are no stored form of
/// version 1 are corruption, SQLCODE -916: another version, a field past
/// the end, an unknown tag, a name or annotation not numbered, an unknown
/// typing, events that make no tree, or bytes after the last item.
#[test]
fn corrupt_bytes_are_refused() {
    let good = encode(&Sequence::document(parse("<a>t</a>", Syntax::Document, Whitespace::Preserve).unwrap()));
    assert!(corrupt(&[2, 0, 0, 0, 0]).contains("version is 2"));
    assert!(corrupt(&good[..good.len() - 1]).contains(WHAT));
    assert!(corrupt(&[good.as_slice(), &[0]].concat()).contains(WHAT));
    assert!(corrupt(&[1, 1, 0, 0, 0, 9]).contains("an item's tag is 9"));
    assert!(corrupt(&[1, 1, 0, 0, 0, ATOMIC, 44]).contains("type is 44"));
    let no_names = [1, 1, 0, 0, 0, NODE, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(corrupt(&[&no_names[..], &[ELEMENT, 5, 0, 0, 0]].concat()).contains("the name 5 is not numbered"));
    let typed = [1, 1, 0, 0, 0, NODE, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'a', 0, 0, 0, 0];
    assert!(corrupt(&[&typed[..], &[ELEMENT, 0, 0, 0, 0, 1, 0, 0, 0]].concat()).contains("the annotation 1 is not numbered"));
    let bad_typing = [1, 1, 0, 0, 0, NODE, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'a', 1, 0, 0, 0, 0, 0, 0, 0, 9];
    assert!(corrupt(&bad_typing).contains("an annotation's typing is 9"));
    assert!(corrupt(&[&no_names[..], &[9]].concat()).contains("an event's tag is 9"));
    assert!(corrupt(&[&no_names[..], &[DOCUMENT, DOCUMENT]].concat()).contains("an event's tag is 1"));
    assert!(corrupt(&[&no_names[..], &[END]].concat()).contains("closes no node"));
    let one_name = [1, 1, 0, 0, 0, NODE, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'a', 0, 0, 0, 0];
    let attribute_in_document = [&one_name[..], &[DOCUMENT, ATTRIBUTE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, END]].concat();
    assert!(corrupt(&attribute_in_document).contains("make no tree"));
    let unclosed = [&one_name[..], &[ELEMENT, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]].concat();
    assert!(corrupt(&unclosed).contains(WHAT));
}

/// The events of `bytes`, read to their end or an error's SQLCODE.
fn events_of(bytes: &[u8]) -> Result<Vec<Event<'_>>> {
    let mut events = Events::new(bytes)?;
    let mut read = Vec::new();
    while let Some(event) = events.next_event()? {
        read.push(event);
    }
    Ok(read)
}

/// Verifies: [DESIGN] RULES.md D106: the stored form is read as events in
/// document order, each item begun, an element's attributes with it, a
/// node standing alone its own item; and bytes that are no stored form
/// are refused as `decode` refuses them, SQLCODE -916.
#[test]
fn the_stored_form_is_read_as_events() {
    let tree = parse("<a x=\"1\">t<!--c--><b/></a>", Syntax::Content, Whitespace::Preserve).unwrap();
    let atomic = Atomic::new(AtomicType::Integer, "1");
    let bytes = encode(&Sequence::new(vec![Item::Node(Node::root(tree)), Item::Atomic(atomic)]));
    let element = |name, attributes| Event::Element { name, attributes };
    let expected = [
        Event::Node,
        Event::Document,
        element(0, vec![(1, "1")]),
        Event::Text("t"),
        Event::Other,
        element(2, vec![]),
        Event::End,
        Event::End,
        Event::End,
        Event::Atomic,
    ];
    assert_eq!(events_of(&bytes).unwrap(), expected);
    let code = |given: &[u8]| events_of(given).map(drop).unwrap_err().sqlcode();
    assert_eq!(code(&[2, 0, 0, 0, 0]), -916_i32, "another version");
    assert_eq!(code(&[1, 1, 0, 0, 0, 9]), -916_i32, "an item's tag");
    let no_names = [1, 1, 0, 0, 0, NODE, 0, 0, 0, 0];
    assert_eq!(code(&[&no_names[..], &[9]].concat()), -916_i32, "an event's tag");
    assert_eq!(code(&[&no_names[..], &[END]].concat()), -916_i32, "an end of nothing");
    assert_eq!(code(&[bytes.as_slice(), &[0]].concat()), -916_i32, "bytes after the last item");
}

/// Verifies: [DESIGN] RULES.md D30, D106: each atomic type is stored by
/// its own tag, the first thirteen as before the others were added, so a
/// value stored before is read as it was; every type is read back.
#[test]
fn every_atomic_type_has_a_tag() {
    assert_eq!(ATOMIC_TYPES.get(12), Some(&AtomicType::HexBinary));
    assert_eq!(ATOMIC_TYPES.get(13), Some(&AtomicType::Duration));
    assert_eq!(ATOMIC_TYPES.get(21), Some(&AtomicType::QName));
    assert_eq!(ATOMIC_TYPES.get(23), Some(&AtomicType::NormalizedString));
    assert_eq!(ATOMIC_TYPES.get(43), Some(&AtomicType::PositiveInteger));
    for kind in AtomicType::all() {
        assert_eq!(ATOMIC_TYPES.iter().filter(|&&t| t == kind).count(), 1, "{}", kind.name());
        let value = Sequence::new(vec![Item::Atomic(Atomic::new(kind, "v"))]);
        let back = again(&value);
        let [Item::Atomic(read)] = back.items() else { panic!("an atomic value") };
        assert_eq!(read.kind, kind);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [XQueryDM §3.3.3] a
/// qualified name is stored with its namespace URI, so its value reads
/// back whole; one of no namespace reads back of none.
#[test]
fn qualified_names_keep_their_namespace() {
    let named = Atomic::qname(Some("http://e.org/n"), "p:local");
    let plain = Atomic::qname(None, "local");
    let value = Sequence::new(vec![Item::Atomic(named.clone()), Item::Atomic(plain.clone())]);
    let back = again(&value);
    let [Item::Atomic(first), Item::Atomic(second)] = back.items() else { panic!("two atomic values") };
    assert_eq!((first, second), (&named, &plain));
    assert_eq!(first.namespace.as_deref(), Some("http://e.org/n"));
}
