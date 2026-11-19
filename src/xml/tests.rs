//! Tests of XML values as a whole, and what the tests of its parts
//! share: a tree written as text, to compare whole trees at once.

use std::fmt::Write;

use super::*;
use crate::error::SqlState;

/// `tree` written as text: each element `<name{uri} a{uri}="v">` then
/// its children then `</>`, the URIs only where names have them and
/// `#id`/`#refs` after an attribute that is one; each text node as it
/// is; each comment `<!--c-->`; each processing instruction `<?t c?>`.
pub(super) fn shape(tree: &Tree) -> String {
    let mut out = String::new();
    for child in tree.children(NodeId::ROOT) {
        write_node(tree, child, &mut out);
    }
    out
}

fn written(name: &QName) -> String {
    let prefix = name.prefix.as_deref().map(|p| format!("{p}:")).unwrap_or_default();
    let uri = name.uri.as_deref().map(|u| format!("{{{u}}}")).unwrap_or_default();
    format!("{prefix}{}{uri}", name.local)
}

fn write_node(tree: &Tree, id: NodeId, out: &mut String) {
    match tree.kind(id).unwrap() {
        Kind::Element => {
            out.push('<');
            out.push_str(&written(tree.name(id).unwrap()));
            for attribute in tree.attributes(id) {
                write!(out, " {}=\"{}\"", written(tree.name(attribute).unwrap()), tree.content(attribute)).unwrap();
                if tree.is_id(attribute) {
                    out.push_str("#id");
                }
                if tree.is_idrefs(attribute) {
                    out.push_str("#refs");
                }
            }
            out.push('>');
            for child in tree.children(id) {
                write_node(tree, child, out);
            }
            out.push_str("</>");
        }
        Kind::Text => out.push_str(tree.content(id)),
        Kind::Comment => write!(out, "<!--{}-->", tree.content(id)).unwrap(),
        Kind::ProcessingInstruction => write!(out, "<?{} {}?>", tree.name(id).unwrap().local, tree.content(id)).unwrap(),
        Kind::Document | Kind::Attribute => panic!("no such child"),
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XQueryDM §6] a
/// parse makes a document node whose children are the text's nodes in
/// document order, with their names, attributes, text, comments and
/// processing instructions.
#[test]
fn a_parse_makes_a_tree_of_the_data_model() {
    let tree = parse("<?xml version='1.0'?><!--c--><a x='1'><?p d?>t<b/></a>", Syntax::Document, Whitespace::Preserve).unwrap();
    assert_eq!(shape(&tree), "<!--c--><a x=\"1\"><?p d?>t<b></></>");
    assert_eq!(tree.kind(NodeId::ROOT), Some(Kind::Document));
    assert_eq!(tree.string_value(NodeId::ROOT), "t");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR6a i] [DESIGN] RULES.md
/// D18, D106: elements nested deeper than `MAX_NESTING` are SQLCODE -900,
/// and as deep as it are read.
#[test]
fn nesting_is_bounded() {
    let deep = |n: usize| format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
    assert!(parse(&deep(MAX_NESTING), Syntax::Document, Whitespace::Preserve).is_ok());
    let error = parse(&deep(MAX_NESTING + 1), Syntax::Document, Whitespace::Preserve).unwrap_err();
    assert_eq!(error.sqlcode(), -900_i32);
    assert_ne!(error.sqlstate(), SqlState::INVALID_XML_DOCUMENT);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR7c] [DESIGN] RULES.md
/// D18, D106: entities that expand to more than `MAX_TEXT` characters are
/// SQLCODE -900, not read for ever.
#[test]
fn entity_expansion_is_bounded() {
    let mut subset = String::from("<!ENTITY e0 '0123456789abcdef0123456789abcdef'>");
    for level in 1_usize..=6 {
        let below = level - 1;
        write!(subset, "<!ENTITY e{level} '{}'>", format!("&e{below};").repeat(16)).unwrap();
    }
    let text = format!("<!DOCTYPE a [{subset}]><a>&e6;</a>");
    let error = parse(&text, Syntax::Document, Whitespace::Preserve).unwrap_err();
    assert_eq!(error.sqlcode(), -900_i32);
    assert!(error.to_string().contains("expand"), "{error}");
}
