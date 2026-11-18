//! Tests of `fn:deep-equal`.

use super::super::testing::{atom, below, call, code, dbl, document, elements, empty, int, one, s, seq, text};
use crate::xml::{AtomicType, Kind};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.3.1.1]
/// the examples of `fn:deep-equal`: an element and its children differ;
/// two elements of the same name and attributes, in any order, are equal,
/// of other attribute values not; an element and a string differ.
#[test]
fn deep_equal_examples() {
    let doc = document(
        "<attendees><name last='Parker' first='Peter'/><name last='Barker' first='Bob'/><name first='Peter' last='Parker'/></attendees>",
    );
    let at = elements(&doc, "attendees").remove(0);
    let names = elements(&doc, "name");
    let children = seq(&names.iter().map(one).collect::<Vec<_>>());
    assert_eq!(text(call("deep-equal", &[one(&at), children])), "false");
    assert_eq!(text(call("deep-equal", &[one(&names[0]), one(&names[1])])), "false");
    assert_eq!(text(call("deep-equal", &[one(&names[0]), one(&names[2])])), "true");
    assert_eq!(text(call("deep-equal", &[one(&names[0]), s("Peter Parker")])), "false");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.3.1]
/// atomic values deep-equal if `eq`, NaN of NaN; not of types `eq` is not
/// defined for; sequences of different lengths not; the empty sequences
/// are.
#[test]
fn atomic_values_and_lengths() {
    assert_eq!(text(call("deep-equal", &[empty(), empty()])), "true");
    assert_eq!(text(call("deep-equal", &[seq(&[int(1), dbl("NaN")]), seq(&[dbl("1"), dbl("NaN")])])), "true");
    assert_eq!(text(call("deep-equal", &[int(1), seq(&[int(1), int(1)])])), "false");
    let today = atom(AtomicType::Date, "2000-01-01");
    assert_eq!(text(call("deep-equal", &[int(1), today])), "false");
    assert_eq!(text(call("deep-equal", &[s("A"), s("a"), s("urn:turborel:collation:CASE_INSENSITIVE")])), "true");
    assert_eq!(code(call("deep-equal", &[s("A"), s("a"), s("nowhere")])), "FOCH0002");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.3.1]
/// nodes deep-equal: of one kind and name, their element and text
/// children in order, comments and processing instructions below them
/// ignored but directly compared by content; namespaces of names count,
/// prefixes not; deep trees compared without recursion.
#[test]
fn nodes_compared_in_depth() {
    let commented = document("<r><a x='1'>t<!--c--><b/></a></r>");
    let instructed = document("<r><a x='1'>t<?p?><b/></a></r>");
    assert_eq!(text(call("deep-equal", &[one(&commented), one(&instructed)])), "true");
    let longer = document("<r><a x='1'>t<b/>u</a></r>");
    assert_eq!(text(call("deep-equal", &[one(&commented), one(&longer)])), "false");
    let prefixed = document("<p:r xmlns:p='urn:x'/>");
    let renamed = document("<q:r xmlns:q='urn:x'/>");
    let other = document("<q:r xmlns:q='urn:y'/>");
    assert_eq!(text(call("deep-equal", &[one(&prefixed), one(&renamed)])), "true");
    assert_eq!(text(call("deep-equal", &[one(&prefixed), one(&other)])), "false");
    let comments = |d| below(d, |n| n.kind() == Some(Kind::Comment));
    let (first, second) = (document("<!--c-->"), document("<!--d-->"));
    assert_eq!(text(call("deep-equal", &[one(&comments(&first)[0]), one(&comments(&second)[0])])), "false");
    let text_node = below(&commented, |n| n.kind() == Some(Kind::Text)).remove(0);
    assert_eq!(text(call("deep-equal", &[one(&text_node), one(&elements(&commented, "b").remove(0))])), "false");
    let deep = format!("{}{}", "<e>".repeat(500), "</e>".repeat(500));
    assert_eq!(text(call("deep-equal", &[one(&document(&deep)), one(&document(&deep))])), "true");
    let attributes = |d| below(d, |n| n.kind() == Some(Kind::Attribute));
    assert_eq!(text(call("deep-equal", &[one(&attributes(&commented)[0]), one(&attributes(&instructed)[0])])), "true");
}
