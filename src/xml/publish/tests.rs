//! Tests of the computed text, comment and processing instruction
//! constructors, and of concatenation.

use super::*;
use crate::xml::tree::{Kind, NodeId};
use crate::xml::{Parameters, Syntax, Version, Whitespace, decode, encode, parse, serialize};

/// The serialization of `value` as content, in XML 1.1.
fn written(value: &Sequence) -> String {
    serialize(value, &Parameters { version: Version::V11, declaration: false, encoding: "UTF-8", undeclare: false }).unwrap()
}

/// The one node of `value`, its kind and content, standing alone.
fn only(value: &Sequence) -> (Kind, String) {
    let [Item::Node(node)] = value.items() else { panic!("one node") };
    assert!(node.tree().parent(node.id()).is_none(), "a node standing alone");
    (node.kind().unwrap(), node.tree().content(node.id()).to_owned())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.19 GR6, GR7] [XQuery
/// §3.7.3.4] [XQueryDM §6.7.1] a text node of the string, standing alone,
/// one of the zero-length string too, which its stored form keeps and a
/// document node's content drops.
#[test]
fn text_nodes_6_19() {
    assert_eq!(only(&text("a<b").unwrap()), (Kind::Text, "a<b".to_owned()));
    assert_eq!(written(&text("a<b").unwrap()), "a&lt;b");
    let empty = text("").unwrap();
    assert_eq!(only(&empty), (Kind::Text, String::new()));
    assert_eq!(only(&decode(&encode(&empty)).unwrap()), (Kind::Text, String::new()));
    assert_eq!(written(&empty), "");
    let doc = crate::xml::document(&empty).unwrap();
    let [Item::Node(d)] = doc.items() else { panic!("one node") };
    assert_eq!(d.tree().children(NodeId::ROOT).count(), 0, "no text node empty in a document node");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.11 GR3, GR4, GR5] [XML 1.1
/// §2.5 [15]] a comment node of the string, standing alone; one holding
/// two hyphens together, ending with one, or holding a character XML 1.1
/// does not, 2200S.
#[test]
fn comment_nodes_6_11() {
    assert_eq!(only(&comment("a - b").unwrap()), (Kind::Comment, "a - b".to_owned()));
    assert_eq!(written(&comment("").unwrap()), "<!---->");
    for bad in ["a--b", "a-", "-", "a\u{0}", "\u{FFFE}"] {
        assert_eq!(comment(bad).unwrap_err().sqlstate(), SqlState::INVALID_COMMENT, "{bad:?}");
    }
    assert!(comment("-a").is_ok() && comment("\u{1}").is_ok(), "a hyphen first, and a restricted character of XML 1.1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.17 GR4, GR9] [XML 1.1 §2.6
/// [16]] [XQuery §3.7.3.5] a processing instruction of the target and
/// string, standing alone, its leading white space removed; one holding
/// '?>' or a character XML 1.1 does not, 2200T.
#[test]
fn processing_instructions_6_17() {
    let pi = processing_instruction("t", " \t\r\n a ?b ").unwrap();
    assert_eq!(only(&pi), (Kind::ProcessingInstruction, "a ?b ".to_owned()));
    assert_eq!(written(&pi), "<?t a ?b ?>");
    assert_eq!(written(&processing_instruction("t", "").unwrap()), "<?t?>");
    assert!(processing_instruction("t", "a?").is_ok());
    for bad in ["a?>", "?>", "\u{0}"] {
        assert_eq!(processing_instruction("t", bad).unwrap_err().sqlstate(), SqlState::INVALID_PROCESSING_INSTRUCTION, "{bad:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.14 GR2, §6.12 GR1 to GR4]
/// the concatenation of XML values: every item of each in order, the null
/// values skipped; the null value if every one is, or there is none.
#[test]
fn concatenation_10_14() {
    let a = Sequence::document(parse("<a/>", Syntax::Content, Whitespace::Preserve).unwrap());
    let b = text("t").unwrap();
    let both = concatenate([Some(&a), None, Some(&b), Some(&a)]).unwrap();
    assert_eq!(both.items().len(), 3);
    assert_eq!(written(&both), "<a/>t<a/>");
    assert_eq!(concatenate([None, Some(&Sequence::default())]).unwrap().items().len(), 0);
    assert!(concatenate([None, None]).is_none());
    assert!(concatenate([]).is_none());
}
