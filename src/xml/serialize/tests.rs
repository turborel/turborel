//! Tests of serializing XML values.

use std::sync::Arc;

use super::*;
use crate::xml::sequence::{Atomic, AtomicType};
use crate::xml::{Syntax, Whitespace, parse};

const PLAIN: Parameters = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };
const V11: Parameters = Parameters { version: Version::V11, declaration: false, encoding: "UTF-8", undeclare: true };

fn value(text: &str) -> Sequence {
    Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())
}

fn written(text: &str, parameters: &Parameters) -> String {
    serialize(&value(text), parameters).unwrap()
}

fn refused(sequence: &Sequence, parameters: &Parameters) -> String {
    let error = serialize(sequence, parameters).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_SERIALIZATION_ERROR, "{error}");
    error.to_string()
}

fn string(s: &str) -> Item {
    Item::Atomic(Atomic::new(AtomicType::String, s))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15 GR11a] [Serialization §5]
/// a value is written so a parse of the text makes the same tree: tags,
/// empty elements, attributes, text, comments and processing
/// instructions, with an XML declaration if one is asked for.
#[test]
fn the_text_parses_to_the_same_tree() {
    for text in ["<a x=\"1\"><b/>t<!--c--><?p d?><?q?></a>", "x<a/>y", "", "<!--c--><a/>", "<a>\u{E9}\u{10000}</a>"] {
        assert_eq!(written(text, &PLAIN), text);
    }
    let declared = Parameters { declaration: true, ..PLAIN };
    assert_eq!(written("<a/>", &declared), "<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15 GR11a] [Serialization §5]
/// markup characters are escaped; a carriage return, NEL, the line
/// separator and control characters are references in text, and line
/// feeds, tabs and quotes too in attribute values.
#[test]
fn characters_a_parse_would_change_are_references() {
    assert_eq!(written("<a>&lt;&amp;&gt;\t\n&#13;\u{85}\u{2028}\u{7F}</a>", &PLAIN), "<a>&lt;&amp;&gt;\t\n&#xD;&#x85;&#x2028;&#x7F;</a>");
    assert_eq!(written("<a x='&#9;&#10;&#13;&quot;&lt;'/>", &PLAIN), "<a x=\"&#x9;&#xA;&#xD;&quot;&lt;\"/>");
    let tree = parse("<?xml version='1.1'?><a>&#1;</a>", Syntax::Document, Whitespace::Preserve).unwrap();
    assert_eq!(serialize(&Sequence::document(tree.clone()), &V11).unwrap(), "<a>&#x1;</a>");
    assert!(refused(&Sequence::document(tree), &PLAIN).contains("err:SERE0006"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15 GR11a] [Serialization
/// §5.1.1] a name, or a comment's character, the version cannot hold is
/// an error.
#[test]
fn what_the_version_cannot_hold_is_an_error() {
    let name = Sequence::document(parse("<?xml version='1.1'?><\u{0221}/>", Syntax::Document, Whitespace::Preserve).unwrap());
    assert!(refused(&name, &PLAIN).contains("err:SERE0005"));
    assert_eq!(serialize(&name, &V11).unwrap(), "<\u{0221}/>");
    let comment = Sequence::document(parse("<a><!--\u{7F}--></a>", Syntax::Document, Whitespace::Preserve).unwrap());
    assert_eq!(serialize(&comment, &PLAIN).unwrap(), "<a><!--\u{7F}--></a>");
    assert!(refused(&comment, &V11).contains("err:SERE0006"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15 GR11a] [Serialization §5,
/// §5.1.7] namespace declarations are written where the tree makes them,
/// an element written alone declares what is in its scope, and a prefix
/// undeclared is written so only if the version can.
#[test]
fn namespaces_are_declared_as_the_tree_binds_them() {
    let text = "<a xmlns=\"d\" xmlns:p=\"u\"><p:b p:x=\"1\"/><c xmlns=\"\"/></a>";
    assert_eq!(written(text, &PLAIN), text);
    let tree = Arc::new(parse(text, Syntax::Document, Whitespace::Preserve).unwrap());
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let b = tree.children(a).next().unwrap();
    let alone = Sequence::new(vec![Item::Node(Node::new(Arc::clone(&tree), b))]);
    assert_eq!(serialize(&alone, &PLAIN).unwrap(), "<p:b xmlns=\"d\" xmlns:p=\"u\" p:x=\"1\"/>");
    let undeclared = "<?xml version='1.1'?><a xmlns:p='u'><b xmlns:p=''/></a>";
    assert_eq!(written(undeclared, &PLAIN), "<a xmlns:p=\"u\"><b/></a>");
    assert_eq!(written(undeclared, &V11), "<a xmlns:p=\"u\"><b xmlns:p=\"\"/></a>");
    assert_eq!(written("<xml:a xml:lang='en'/>", &PLAIN), "<xml:a xml:lang=\"en\"/>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.15 GR11a] [Serialization §2]
/// sequence normalization: atomic values as their lexical forms, adjacent
/// ones a space apart; document nodes as their children; nothing for an
/// empty sequence; an attribute alone an error.
#[test]
fn sequences_are_normalized() {
    let tree = Arc::new(parse("<a x='1'>t</a>", Syntax::Document, Whitespace::Preserve).unwrap());
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let document = Item::Node(Node::new(Arc::clone(&tree), NodeId::ROOT));
    let items =
        vec![string("1"), string("a<"), document, string("2"), Item::Node(Node::new(Arc::clone(&tree), tree.first_child(a).unwrap()))];
    assert_eq!(serialize(&Sequence::new(items), &PLAIN).unwrap(), "1 a&lt;<a x=\"1\">t</a>2t");
    assert_eq!(serialize(&Sequence::default(), &PLAIN).unwrap(), "");
    let attribute = Sequence::new(vec![Item::Node(Node::new(Arc::clone(&tree), tree.attributes(a).next().unwrap()))]);
    assert!(refused(&attribute, &PLAIN).contains("err:SENR0001"));
}
