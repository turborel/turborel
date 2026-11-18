//! Tests of the primary XML index's answers: a value's own, of many
//! values and lookups made, from its entries alone.

use std::collections::HashMap;

use super::*;
use crate::index::xml::check::tests::{Made, element, kind, lookup};
use crate::index::xml::tests::{compared, document, index, path, stored};
use crate::index::xml::{Axis, Literal, Method, PRIMARY_VALUE_BYTES, Relation, Target, XmlRelated};
use crate::index::{Index, Key};
use crate::value::{CompOp, Value};
use crate::xml::QName;
use crate::xml::{Sequence, Syntax, Whitespace, encode, parse};

/// The answers of `index` for `query`, by row.
fn answers(index: &Arc<XmlIndex>, query: &XmlQuery) -> HashMap<RowId, XmlAnswer> {
    Arc::clone(index).xml_answers(query).unwrap().map(Result::unwrap).collect()
}

/// `lookup` alone.
fn alone(lookup: XmlLookup) -> XmlQuery {
    XmlQuery::of(lookup)
}

/// Verifies: [EXTENSION] RULES.md D106 [Pal et al. §2.2, §3.1.1, §3.1.2]
/// [XQuery §3.2, §3.2.2, §3.5.2]: of 120 values made, each a row of a
/// primary index, and of 300 lookups made, some filtered by a predicate of
/// an ancestor, the index answers each row from its entries alone as the
/// value's own check does: it holds where the check is true, is not given
/// where false, and is checked where the check raises.
#[test]
fn the_primary_answers_as_each_value_does() {
    let mut made = Made(0x2545_F491_4F6C_DD1D);
    let index = index(Method::Primary);
    let mut values = Vec::new();
    for row in 0..120_u64 {
        let mut text = String::new();
        for _ in 0..=made.below(2) {
            element(&mut made, 4, &mut text);
        }
        let tree = parse(&text, Syntax::Content, Whitespace::Preserve).unwrap();
        let bytes = encode(&Sequence::document(tree));
        index.insert(Key(vec![Value::Xml(crate::value::xml::XmlValue::stored(crate::value::lob::Lob::of_octets(&bytes)))]), row).unwrap();
        values.push(bytes);
    }
    let mut seen = [0_usize; 3];
    for _ in 0..300_usize {
        let query = lookup(&mut made);
        let answered = answers(&index, &query);
        for (row, bytes) in (0_u64..).zip(&values) {
            let wanted = match kind(&query.matches_stored(bytes)) {
                Ok(true) => Some(XmlAnswer::Holds),
                Ok(false) => None,
                Err(_) => Some(XmlAnswer::Check),
            };
            assert_eq!(answered.get(&row).copied(), wanted, "row {row} {query:?}");
            seen[wanted.map_or(0, |a| if a == XmlAnswer::Holds { 1 } else { 2 })] += 1_usize;
        }
    }
    assert!(seen.iter().all(|&n| n > 100), "every answer is given often: {seen:?}");
}

/// Verifies: [EXTENSION] RULES.md D106 [Pal et al. §3.1.2]: the paper's
/// `/BOOK[@ISBN = "1-55860-438-3"]/SECTION`, a parent join of a SECTION
/// and its BOOK's ISBN, holds of the book of that ISBN alone; of a row
/// asked, that row alone is answered.
#[test]
fn a_parent_join_answers_the_papers_query() {
    let index = index(Method::Primary);
    let book = |isbn: &str| format!(r#"<BOOK ISBN="{isbn}"><SECTION><TITLE>Bad Bugs</TITLE></SECTION></BOOK>"#);
    index.insert(Key(vec![document(&book("1-55860-438-3"))]), 1).unwrap();
    index.insert(Key(vec![document(&book("0-00000-000-0"))]), 2).unwrap();
    index.insert(Key(vec![document("<BOOK><SECTION/></BOOK>")]), 3).unwrap();
    let isbn = path(&[(Axis::Child, "ISBN")], Target::Attribute);
    let filter = XmlFilter { at: 1, pattern: isbn, comparisons: vec![compared(CompOp::Eq, Literal::String("1-55860-438-3".to_owned()))] };
    let lookup = XmlLookup {
        pattern: path(&[(Axis::Child, "BOOK"), (Axis::Child, "SECTION")], Target::Element),
        comparisons: Vec::new(),
        row: None,
    };
    let mut query = XmlQuery { lookup, filters: vec![filter], related: None };
    assert_eq!(answers(&index, &query), HashMap::from([(1, XmlAnswer::Holds)]));
    query.lookup.row = Some(2);
    assert_eq!(answers(&index, &query), HashMap::new());
    query.lookup.row = Some(1);
    assert_eq!(answers(&index, &query), HashMap::from([(1, XmlAnswer::Holds)]));
    let cost = Arc::clone(&index).xml_cost(&query.lookup).unwrap();
    assert!(cost.entries.is_some_and(|n| n <= 3), "a seek of the path, and its section: {cost:?}");
    assert!(cost.pages.is_some_and(|p| p >= 1.0_f64));
    let rows: Vec<RowId> = Arc::clone(&index).xml_rows(&query.lookup).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, [1], "of the row asked, its rows found");
}

/// Verifies: [EXTENSION] RULES.md D106, D29: a value longer than the
/// primary keys whole, whose comparison its entries cannot answer, is
/// checked; so is a row whose versions held are of two values, until one
/// is taken back.
#[test]
fn a_cut_value_and_versions_alike_are_checked() {
    let index = index(Method::Primary);
    let long = "x".repeat(PRIMARY_VALUE_BYTES + 1);
    index.insert(Key(vec![document(&format!("<a>{long}</a>"))]), 1).unwrap();
    let lookup = |text: &str| {
        alone(XmlLookup {
            pattern: path(&[(Axis::Child, "a")], Target::Element),
            comparisons: vec![compared(CompOp::Eq, Literal::String(text.to_owned()))],
            row: None,
        })
    };
    assert_eq!(answers(&index, &lookup(&long)), HashMap::from([(1, XmlAnswer::Check)]));
    index.insert(Key(vec![document("<a>one</a>")]), 2).unwrap();
    index.insert(Key(vec![document("<a>two</a>")]), 2).unwrap();
    assert_eq!(answers(&index, &lookup("one")), HashMap::from([(2, XmlAnswer::Check)]), "of two versions");
    index.remove(&Key(vec![document("<a>two</a>")]), 2).unwrap();
    assert_eq!(answers(&index, &lookup("one")), HashMap::from([(2, XmlAnswer::Holds)]));
    assert_eq!(answers(&index, &lookup("two")), HashMap::new());
    let stored_one = stored("<a>one</a>");
    assert!(lookup("one").matches_stored(&stored_one).unwrap());
}

/// Verifies: [EXTENSION] RULES.md D106: a secondary method answers no row
/// from its entries alone; the primary's ancestor of a node is its
/// ORDPATH less as many components [Pal et al. §2.2].
#[test]
fn only_the_primary_answers_and_ancestors_are_prefixes() {
    let lookup = XmlLookup { pattern: path(&[(Axis::Child, "a")], Target::Element), comparisons: Vec::new(), row: None };
    assert!(index(Method::Path).xml_answers(&alone(lookup)).is_none());
    let label = [ordinal_of(1), ordinal_of(3), ordinal_of(300)].concat();
    assert_eq!(ancestor(&label, 1).unwrap(), &label[..4]);
    assert_eq!(ancestor(&label, 2).unwrap(), &label[..2]);
    assert!(ancestor(&label, 3).is_err());
}

/// Verifies: [EXTENSION] RULES.md D106 [Pal et al. §2.2, §3.1.1] [XQuery
/// §3.2.1.1]: of the paper's document, the primary answers a relation of
/// an element to a node by their ORDPATHs: its following and preceding
/// elements past and before its range of keys, not its ancestors; its
/// siblings of its parent's range and as many components; its ancestors
/// of the prefixes of its ORDPATH; an attribute has no sibling, and its
/// element is its ancestor.
#[test]
fn relations_are_answered_by_ordpaths() {
    let index = index(Method::Primary);
    let book = r#"<BOOK ISBN="1"><SECTION><TITLE>Bad Bugs</TITLE>Nobody<FIGURE CAPTION="Sample bug"/></SECTION><SECTION><TITLE>Tree Frogs</TITLE>All<BOLD> love </BOLD></SECTION></BOOK>"#;
    index.insert(Key(vec![document(book)]), 1).unwrap();
    let asked = |node: &str, target: Target, relation: Relation, name: Option<&str>| {
        let lookup = XmlLookup { pattern: path(&[(Axis::Descendant, node)], target), comparisons: Vec::new(), row: None };
        let related = Some(XmlRelated { relation, name: name.map(QName::local) });
        let query = XmlQuery { related, ..XmlQuery::of(lookup) };
        let wanted = query.matches_stored(&stored(book)).unwrap();
        let answered = answers(&index, &query).contains_key(&1);
        assert_eq!(answered, wanted, "{node} {relation:?} {name:?}");
        answered
    };
    let (e, a) = (Target::Element, Target::Attribute);
    for (node, target, relation, name, holds) in [
        ("TITLE", e, Relation::FollowingSibling, Some("FIGURE"), true),
        ("FIGURE", e, Relation::FollowingSibling, Some("TITLE"), false),
        ("FIGURE", e, Relation::PrecedingSibling, Some("TITLE"), true),
        ("TITLE", e, Relation::PrecedingSibling, None, false),
        ("BOLD", e, Relation::Preceding, Some("FIGURE"), true),
        ("FIGURE", e, Relation::Preceding, Some("BOLD"), false),
        // Its own section is above it.
        ("FIGURE", e, Relation::Preceding, Some("SECTION"), false),
        ("FIGURE", e, Relation::Following, None, true),
        ("BOLD", e, Relation::Following, None, false),
        // The figure is below its section.
        ("SECTION", e, Relation::Following, Some("FIGURE"), false),
        ("TITLE", e, Relation::Ancestor, Some("BOOK"), true),
        ("BOOK", e, Relation::Ancestor, None, false),
        ("CAPTION", a, Relation::AncestorOrSelf, Some("FIGURE"), true),
        ("FIGURE", e, Relation::AncestorOrSelf, Some("FIGURE"), true),
        ("CAPTION", a, Relation::FollowingSibling, None, false),
        ("CAPTION", a, Relation::Following, Some("SECTION"), true),
    ] {
        assert_eq!(asked(node, target, relation, name), holds, "{node} {relation:?} {name:?}");
    }
}
