//! Tests of the nodes an XML index keys.

use std::collections::HashMap;

use super::*;
use crate::index::xml::VALUE_PREFIX_BYTES;
use crate::index::xml::keys::Cursor;
use crate::xml::{Item, Node, Sequence, Syntax, Whitespace, encode, parse};

/// A node keyed, owned: its path as names, kind first, its text, whether
/// whole, its ORDPATH dotted.
type Seen = (String, String, bool, String);

/// The stored form of `text`, a document.
fn stored(text: &str) -> Vec<u8> {
    encode(&Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap()))
}

/// `ordpath`'s components, dotted.
fn dotted(ordpath: &[u8]) -> String {
    let mut at = Cursor { bytes: ordpath, at: 0 };
    let mut parts = Vec::new();
    while at.at < ordpath.len() {
        parts.push(at.ordinal().unwrap().to_string());
    }
    parts.join(".")
}

/// The nodes `bytes` keys, each name tokened by its place among those
/// met, and the names by token.
fn derived(bytes: &[u8]) -> Vec<Seen> {
    let mut tokens: HashMap<String, u64> = HashMap::new();
    let mut names = |n: &QName| {
        let next = u64::try_from(tokens.len()).unwrap().saturating_add(1);
        Ok(Some(*tokens.entry(n.local.to_string()).or_insert(next)))
    };
    let mut seen = Vec::new();
    let mut visit = |k: &Keyed<'_>| {
        seen.push((k.path.to_vec(), String::from_utf8(k.text.to_vec()).unwrap(), k.whole, dotted(k.ordpath)));
        Ok(())
    };
    derive(bytes, VALUE_PREFIX_BYTES, &mut names, &mut visit).unwrap();
    let by_token: HashMap<u64, String> = tokens.into_iter().map(|(n, t)| (t, n)).collect();
    seen.into_iter()
        .map(|(path, text, whole, ordpath)| {
            let mut at = Cursor { bytes: &path, at: 0 };
            let kind = if at.byte().unwrap() == ATTRIBUTE { "@" } else { "" };
            let mut local = Vec::new();
            while at.at < path.len() {
                local.push(by_token[&at.ordinal().unwrap()].clone());
            }
            (format!("{kind}{}", local.join("<")), text, whole, ordpath)
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D86 [Pal et al. §2.2, §3.1.1]: the nodes of
/// the paper's Figure 1, each element and attribute below the document,
/// keyed by its reversed path, its string value [XQueryDM §6.2.3, §6.3.3]
/// and its ORDPATH: the paper's labels of Figure 2 after the document
/// item's 1, attributes and children numbered together by odd numbers.
#[test]
fn the_papers_document_is_keyed_as_its_figures_label_it() {
    let text = r#"<BOOK ISBN="1-55860-438-3"><SECTION><TITLE>Bad Bugs</TITLE>Nobody loves bad bugs.<FIGURE CAPTION="Sample bug"/></SECTION><SECTION><TITLE>Tree Frogs</TITLE>All right-thinking people<BOLD> love </BOLD>tree frogs.</SECTION></BOOK>"#;
    let seen = derived(&stored(text));
    let expected: Vec<Seen> = [
        ("@ISBN<BOOK", "1-55860-438-3", "1.1.1"),
        ("TITLE<SECTION<BOOK", "Bad Bugs", "1.1.3.1"),
        ("@CAPTION<FIGURE<SECTION<BOOK", "Sample bug", "1.1.3.5.1"),
        ("FIGURE<SECTION<BOOK", "", "1.1.3.5"),
        ("SECTION<BOOK", "Bad BugsNobody loves bad bugs.", "1.1.3"),
        ("TITLE<SECTION<BOOK", "Tree Frogs", "1.1.5.1"),
        ("BOLD<SECTION<BOOK", " love ", "1.1.5.5"),
        ("SECTION<BOOK", "Tree FrogsAll right-thinking people love tree frogs.", "1.1.5"),
    ]
    .into_iter()
    .map(|(p, t, o)| (p.to_owned(), t.to_owned(), true, o.to_owned()))
    .collect();
    assert_eq!(&seen[..8], &expected[..]);
    let (book, value, complete, label) = &seen[8];
    assert_eq!((book.as_str(), complete, label.as_str()), ("BOOK", &false, "1.1"), "the book's string value is cut");
    let all = "Bad BugsNobody loves bad bugs.Tree FrogsAll right-thinking people love tree frogs.";
    assert_eq!(value.as_bytes(), &all.as_bytes()[..VALUE_PREFIX_BYTES], "its text descendants', not its attribute's");
    assert_eq!(seen.len(), 9);
}

/// Verifies: [DESIGN] RULES.md D86: a string value keyed whole up to
/// `VALUE_PREFIX_BYTES` octets, and cut past them, at that many octets
/// whatever characters they split; an attribute's alike.
#[test]
fn values_are_cut_past_their_prefix() {
    let at = "a".repeat(VALUE_PREFIX_BYTES);
    let past = format!("{at}é");
    let seen = derived(&stored(&format!("<r><v>{at}</v><w>{past}</w><x y=\"{past}\"/></r>")));
    assert_eq!((seen[0].1.len(), seen[0].2), (VALUE_PREFIX_BYTES, true), "whole at the bound");
    assert_eq!((seen[1].1.as_str(), seen[1].2), (at.as_str(), false), "cut past it");
    assert_eq!((seen[2].0.as_str(), seen[2].2), ("@y<x<r", false), "an attribute cut");
}

/// Verifies: [DESIGN] RULES.md D86: a node item that is an element is its
/// paths' context, not keyed itself; its attributes and the elements
/// below it are, their paths beginning below it. An atomic item takes a
/// place among the items, and a text item keys nothing.
#[test]
fn an_element_item_is_the_context_of_its_paths() {
    let tree = std::sync::Arc::new(parse("<r><a k=\"v\"><b>t</b></a></r>", Syntax::Content, Whitespace::Preserve).unwrap());
    let a = tree.descendants(crate::xml::NodeId::ROOT).find(|&n| tree.name(n).is_some_and(|q| &*q.local == "a")).unwrap();
    let atomic = crate::xml::Atomic { kind: crate::xml::AtomicType::String, lexical: "x".into() };
    let sequence = Sequence::new(vec![Item::Atomic(atomic), Item::Node(Node::new(std::sync::Arc::clone(&tree), a))]);
    let seen = derived(&encode(&sequence));
    let expected = [("@k".to_owned(), "v".to_owned(), true, "3.1".to_owned()), ("b".to_owned(), "t".to_owned(), true, "3.3".to_owned())];
    assert_eq!(seen, expected);
}

/// Verifies: [DESIGN] RULES.md D86, D18: a value of more elements and
/// attributes than `MAX_KEYED_NODES` is refused as it is keyed, SQLCODE
/// -900; one of as many is keyed whole.
#[test]
fn keyed_nodes_are_bounded() {
    let elements = |n: usize| stored(&format!("<r>{}</r>", "<b/>".repeat(n)));
    let count = |bytes: &[u8]| derive(bytes, VALUE_PREFIX_BYTES, &mut |_| Ok(Some(1)), &mut |_| Ok(()));
    assert_eq!(count(&elements(MAX_KEYED_NODES - 1)).unwrap(), MAX_KEYED_NODES, "the r and its b's");
    assert_eq!(count(&elements(MAX_KEYED_NODES)).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [DESIGN] RULES.md D86: a node whose name, or an ancestor's
/// name, has no token is not keyed, as no entry of it can be there.
#[test]
fn a_name_with_no_token_keys_nothing_below_it() {
    let mut names = |n: &QName| Ok((&*n.local != "s").then_some(7));
    let mut seen = Vec::new();
    derive(&stored("<r><s><t/></s><u/></r>"), VALUE_PREFIX_BYTES, &mut names, &mut |k| {
        seen.push(dotted(k.ordpath));
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, ["1.1.3", "1.1"]);
}

/// Verifies: [DESIGN] RULES.md D86: bytes that are no stored form are
/// refused, SQLCODE -916.
#[test]
fn bytes_of_no_stored_form_are_refused() {
    let mut bytes = stored("<r/>");
    bytes.push(0);
    assert_eq!(derive(&bytes, VALUE_PREFIX_BYTES, &mut |_| Ok(Some(1)), &mut |_| Ok(())).unwrap_err().sqlcode(), -916_i32);
}
