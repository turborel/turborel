//! Tests of a lookup's own answer of a value, read in one pass: the same
//! as that of the value decoded into a tree and walked, of many values
//! and lookups made by a generator of a fixed seed (RULES.md R-TEST-5).

use std::fmt::Write;

use super::*;
use crate::index::xml::lookup::{PathPattern, Step, steps_match};
use crate::xml::{Item, Kind, NodeId, Sequence, Syntax, Tree, Whitespace, encode, parse};

/// The answer of `lookup` of `sequence`, decoded into trees and walked:
/// each element below each item, in document order, then its attributes;
/// the first that holds, or raises, answers.
fn by_tree(lookup: &XmlLookup, sequence: &Sequence) -> Result<bool> {
    lookup.pattern.steps.first().ok_or_else(|| SqlError::new(Code::Api, "no step"))?;
    for item in sequence.items() {
        let Item::Node(node) = item else { continue };
        let (tree, root) = (node.tree(), node.id());
        let below = std::iter::once(root).chain(tree.descendants(root)).filter(|&n| tree.kind(n) == Some(Kind::Element));
        for element in below {
            if element != root && holds_of(lookup, tree, (root, element))? {
                return Ok(true);
            }
            for attribute in tree.attributes(element) {
                if holds_of(lookup, tree, (root, attribute))? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Whether `node`, below item `root`, is selected and its comparisons
/// hold of its string value.
fn holds_of(lookup: &XmlLookup, tree: &Tree, (root, node): (NodeId, NodeId)) -> Result<bool> {
    let kind = match lookup.pattern.target {
        Target::Element => Kind::Element,
        Target::Attribute => Kind::Attribute,
    };
    if tree.kind(node) != Some(kind) {
        return Ok(false);
    }
    let above = std::iter::successors(Some(node), |&n| tree.parent(n)).take_while(|&n| n != root);
    let mut names: Vec<Named<'_>> = above.filter_map(|n| tree.name(n)).map(Named).collect();
    names.reverse();
    let steps: Vec<(Axis, Option<Named<'_>>)> = lookup.pattern.steps.iter().map(|s| (s.axis, s.name.as_ref().map(Named))).collect();
    if !steps_match(&steps, &names) {
        return Ok(false);
    }
    lookup.compared(&tree.string_value(node))
}

/// A name compared as an expanded QName, whatever its prefix.
struct Named<'n>(&'n QName);

impl PartialEq for Named<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same(other.0)
    }
}

/// A generator of a fixed seed: xorshift.
struct Made(u64);

impl Made {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0.wrapping_shl(13);
        self.0 ^= self.0.wrapping_shr(7);
        self.0 ^= self.0.wrapping_shl(17);
        usize::try_from(self.0.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
    }
    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len())]
    }
}

/// The names, and the texts, of the values made.
const NAMES: [&str; 3] = ["a", "b", "c"];
const TEXTS: [&str; 8] = ["1", "2.5", " 3 ", "x", "", "-INF", "NaN", "10"];

/// An element made, `depth` deep at most, as text.
fn element(made: &mut Made, depth: usize, out: &mut String) {
    let name = made.pick(&NAMES);
    out.push('<');
    out.push_str(name);
    for attribute in ["x", "y"] {
        if made.below(3) == 0 {
            write!(out, " {attribute}=\"{}\"", made.pick(&TEXTS)).unwrap();
        }
    }
    out.push('>');
    let children = if depth == 0 { 0 } else { made.below(4) };
    for _ in 0..children {
        if made.below(3) == 0 {
            out.push_str(made.pick(&TEXTS));
        } else {
            element(made, depth.saturating_sub(1), out);
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// A lookup made: one to three steps, an element or an attribute, none to
/// two comparisons.
fn lookup(made: &mut Made) -> XmlLookup {
    let steps = (0..=made.below(3))
        .map(|_| {
            let axis = if made.below(2) == 0 { Axis::Child } else { Axis::Descendant };
            let name = ["a", "b", "c", "x", "y", "*"][made.below(6)];
            Step { axis, name: (name != "*").then(|| QName::local(name)) }
        })
        .collect();
    let target = if made.below(3) == 0 { Target::Attribute } else { Target::Element };
    let comparisons = (0..made.below(3))
        .map(|_| {
            let op = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge][made.below(6)];
            let text = made.pick(&TEXTS).to_owned();
            let literal = match made.below(3) {
                0 => Literal::String(text),
                1 => Literal::Untyped(text),
                _ => Literal::Number([1.0_f64, 2.5, 3.0, -1.0][made.below(4)]),
            };
            Comparison { op, literal }
        })
        .collect();
    XmlLookup { pattern: PathPattern { steps, target }, comparisons, row: None }
}

/// The answer's kind: true, false, or an error's SQLCODE.
fn kind(answer: &Result<bool>) -> std::result::Result<bool, i32> {
    answer.as_ref().copied().map_err(SqlError::sqlcode)
}

/// Verifies: [EXTENSION] RULES.md D86 [XQuery §3.2, §3.5.2] [XQueryDM
/// §6.2.3]: of 400 values made, documents and sequences of elements, each
/// asked 25 lookups made, the answer read in one pass over the stored form
/// is that of the value decoded and walked: true, false, or the same
/// error, in document order.
#[test]
fn one_pass_answers_as_the_tree_does() {
    let mut made = Made(0x9E37_79B9_7F4A_7C15);
    let mut asked = [0_usize; 3];
    for n in 0..400_usize {
        let mut text = String::new();
        for _ in 0..=made.below(2) {
            element(&mut made, 4, &mut text);
        }
        let tree = parse(&text, Syntax::Content, Whitespace::Preserve).unwrap();
        let sequence = if n.is_multiple_of(2) {
            Sequence::document(tree)
        } else {
            let tree = std::sync::Arc::new(tree);
            let items = tree.children(NodeId::ROOT).map(|c| Item::Node(crate::xml::Node::new(std::sync::Arc::clone(&tree), c))).collect();
            Sequence::new(items)
        };
        let stored = encode(&sequence);
        for _ in 0..25_usize {
            let lookup = lookup(&mut made);
            let (streamed, walked) = (lookup.matches_stored(&stored), by_tree(&lookup, &sequence));
            assert_eq!(kind(&streamed), kind(&walked), "{text} {lookup:?}");
            asked[match kind(&walked) {
                Ok(true) => 0,
                Ok(false) => 1,
                Err(_) => 2,
            }] += 1_usize;
        }
    }
    assert!(asked.iter().all(|&n| n > 100), "every answer is met often: {asked:?}");
}

/// Verifies: [EXTENSION] RULES.md D86, D18: a path of no step, or of more
/// than `MAX_STEPS`, is refused; a value held as a large object is read
/// whole and answered.
#[test]
fn paths_are_bounded_and_values_read_whole() {
    let stored = encode(&Sequence::document(parse("<a><b>1</b></a>", Syntax::Content, Whitespace::Preserve).unwrap()));
    let path = |n: usize| XmlLookup {
        pattern: PathPattern { steps: vec![Step { axis: Axis::Descendant, name: Some(QName::local("b")) }; n], target: Target::Element },
        comparisons: Vec::new(),
        row: None,
    };
    assert_eq!(path(0).matches_stored(&stored).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(path(MAX_STEPS + 1).matches_stored(&stored).unwrap_err().sqlcode(), -900_i32);
    assert!(path(1).matches_stored(&stored).unwrap());
    let lob = crate::value::lob::fixtures::octets_by(&stored, 4);
    assert!(path(1).matches(&XmlValue::stored(lob)).unwrap(), "read through its chunks");
    assert!(!path(2).matches(&XmlValue::stored(crate::value::lob::Lob::of_octets(&stored))).unwrap());
}
