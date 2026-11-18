//! Tests of a lookup's own answer of a value, read in one pass: the same
//! as that of the value decoded into a tree and walked, of many values
//! and lookups made by a generator of a fixed seed (RULES.md R-TEST-5).

use std::fmt::Write;

use super::*;
use crate::index::xml::lookup::{Anchored, MAX_FILTERS, PathPattern, Step, Target, XmlFilter, XmlQuery, steps_match};
use crate::xml::{Item, Kind, NodeId, Sequence, Syntax, Tree, Whitespace, encode, parse};

/// The answer of `query` of `sequence`, decoded into trees and walked:
/// each element below each item, in document order, then its attributes;
/// the first that holds, or raises, answers.
fn by_tree(query: &XmlQuery, sequence: &Sequence) -> Result<bool> {
    query.lookup.pattern.steps.first().ok_or_else(|| SqlError::new(Code::Api, "no step"))?;
    let anchors = query.anchors()?;
    for item in sequence.items() {
        let Item::Node(node) = item else { continue };
        let (tree, root) = (node.tree(), node.id());
        let below = std::iter::once(root).chain(tree.descendants(root)).filter(|&n| tree.kind(n) == Some(Kind::Element));
        for element in below {
            if element != root && holds_of((query, &anchors), tree, (root, element))? {
                return Ok(true);
            }
            for attribute in tree.attributes(element) {
                if holds_of((query, &anchors), tree, (root, attribute))? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Whether `node`, below item `root`, is selected, its comparisons hold
/// of its string value, and each filter of its anchor, its ancestor or
/// itself, as `anchors` place them: false if one does not, else the error
/// one raised.
fn holds_of((query, anchors): (&XmlQuery, &[Anchored]), tree: &Tree, (root, node): (NodeId, NodeId)) -> Result<bool> {
    if !selected(&query.lookup.pattern, tree, (root, node)) || !query.lookup.compared(&tree.string_value(node))? {
        return Ok(false);
    }
    let mut chain: Vec<NodeId> = std::iter::successors(Some(node), |&n| tree.parent(n)).take_while(|&n| n != root).collect();
    chain.push(root);
    chain.reverse();
    let depth = chain.len().saturating_sub(1);
    let mut raised = None;
    for (filter, anchored) in query.filters.iter().zip(anchors) {
        let at = match *anchored {
            Anchored::Above(n) => depth.checked_sub(n),
            Anchored::Below(n) => Some(n),
        };
        // An attribute has nothing below it a filter selects.
        let Some(&anchor) = at.and_then(|at| chain.get(at)).filter(|&&a| tree.kind(a) != Some(Kind::Attribute)) else { return Ok(false) };
        match filter_holds(filter, tree, anchor) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(e) => {
                raised.get_or_insert(e);
            }
        }
    }
    raised.map_or(Ok(true), Err)
}

/// Whether `pattern` selects `node` from `root`: of its kind, the names
/// from below `root` down to it its steps'.
fn selected(pattern: &PathPattern, tree: &Tree, (root, node): (NodeId, NodeId)) -> bool {
    let kind = match pattern.target {
        Target::Element => Kind::Element,
        Target::Attribute => Kind::Attribute,
    };
    if tree.kind(node) != Some(kind) || node == root {
        return false;
    }
    let above = std::iter::successors(Some(node), |&n| tree.parent(n)).take_while(|&n| n != root);
    let mut names: Vec<Named<'_>> = above.filter_map(|n| tree.name(n)).map(Named).collect();
    names.reverse();
    let steps: Vec<(Axis, Option<Named<'_>>)> = pattern.steps.iter().map(|s| (s.axis, s.name.as_ref().map(Named))).collect();
    steps_match(&steps, &names)
}

/// Whether `filter` holds of `anchor`: a node it selects below it holds;
/// else the first error one raised; else false.
fn filter_holds(filter: &XmlFilter, tree: &Tree, anchor: NodeId) -> Result<bool> {
    let mut raised = None;
    let elements = std::iter::once(anchor).chain(tree.descendants(anchor)).filter(|&n| tree.kind(n) == Some(Kind::Element));
    for node in elements.flat_map(|e| std::iter::once(e).chain(tree.attributes(e))) {
        if selected(&filter.pattern, tree, (anchor, node)) {
            match compared_by(&filter.comparisons, &tree.string_value(node)) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    raised.get_or_insert(e);
                }
            }
        }
    }
    raised.map_or(Ok(false), Err)
}

/// A name compared as an expanded QName, whatever its prefix.
struct Named<'n>(&'n QName);

impl PartialEq for Named<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same(other.0)
    }
}

/// A generator of a fixed seed: xorshift.
pub(in crate::index::xml) struct Made(pub(in crate::index::xml) u64);

impl Made {
    pub(in crate::index::xml) fn below(&mut self, n: usize) -> usize {
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
pub(in crate::index::xml) fn element(made: &mut Made, depth: usize, out: &mut String) {
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

/// A query made: a lookup of one to three steps, an element or an
/// attribute, none to two comparisons; and at times one or two filters,
/// each of a step whose node is fixed of a node selected.
pub(in crate::index::xml) fn lookup(made: &mut Made) -> XmlQuery {
    lookup_of(made, true)
}

/// A query made, with filters made if `filtered`.
fn lookup_of(made: &mut Made, filtered: bool) -> XmlQuery {
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
    let lookup = XmlLookup { pattern: PathPattern { steps, target }, comparisons, row: None };
    let mut query = XmlQuery::of(lookup);
    let filters = if filtered && made.below(2) == 0 { made.below(2).saturating_add(1) } else { 0 };
    for _ in 0..filters {
        let spine = &query.lookup.pattern.steps;
        let child = |s: &[Step]| s.iter().all(|s| s.axis == Axis::Child);
        let fixed: Vec<usize> = (0..=spine.len()).filter(|&at| child(&spine[at..]) || child(&spine[..at])).collect();
        let at = fixed[made.below(fixed.len())];
        let inner = lookup_of(made, false).lookup;
        query.filters.push(XmlFilter { at, pattern: inner.pattern, comparisons: inner.comparisons });
    }
    query
}

/// The answer's kind: true, false, or an error's SQLCODE.
pub(in crate::index::xml) fn kind(answer: &Result<bool>) -> std::result::Result<bool, i32> {
    answer.as_ref().copied().map_err(SqlError::sqlcode)
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §3.2, §3.2.2, §3.5.2]
/// [XQueryDM §6.2.3]: of 400 values made, documents and sequences of
/// elements, each asked 40 lookups made, some filtered by a predicate of
/// an ancestor, the answer read in one pass over the stored form is that
/// of the value decoded and walked: true, false, or an error, in document
/// order.
#[test]
fn one_pass_answers_as_the_tree_does() {
    let mut made = Made(0x9E37_79B9_7F4A_7C15);
    let mut asked = [0_usize; 6];
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
        for _ in 0..40_usize {
            let lookup = lookup(&mut made);
            let (streamed, walked) = (lookup.matches_stored(&stored), by_tree(&lookup, &sequence));
            assert_eq!(kind(&streamed), kind(&walked), "{text} {lookup:?}");
            let answered = match kind(&walked) {
                Ok(true) => 0,
                Ok(false) => 1,
                Err(_) => 2,
            };
            asked[answered + if lookup.filters.is_empty() { 0 } else { 3 }] += 1_usize;
        }
    }
    assert!(asked.iter().all(|&n| n > 30), "every answer is met often, with a filter and without: {asked:?}");
}

/// Verifies: [EXTENSION] RULES.md D106, D18: a path of no step, or of more
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

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §3.2.2, §3.5.2]: a node
/// selected waits for its anchor's filter: of `//a/b[../c/@x = 1]`, the
/// first b's anchor raises as it ends, after a later b's anchor holds, and
/// the first b, before it, answers.
#[test]
fn a_node_waits_for_its_anchors_filter() {
    let text = r#"<a><b/><a><b/><c x="1"/></a><c x="no"/></a>"#;
    let sequence = Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap());
    let step = |axis, name: &str| Step { axis, name: Some(QName::local(name)) };
    let query = XmlQuery {
        lookup: XmlLookup {
            pattern: PathPattern { steps: vec![step(Axis::Descendant, "a"), step(Axis::Child, "b")], target: Target::Element },
            comparisons: Vec::new(),
            row: None,
        },
        filters: vec![XmlFilter {
            at: 1,
            pattern: PathPattern { steps: vec![step(Axis::Child, "c"), step(Axis::Child, "x")], target: Target::Attribute },
            comparisons: vec![Comparison { op: CompOp::Eq, literal: Literal::Number(1.0) }],
        }],
    };
    assert_eq!(kind(&by_tree(&query, &sequence)), Err(-301_i32));
    assert_eq!(kind(&query.matches_stored(&encode(&sequence))), Err(-301_i32));
}

/// The query `/a[c/@x = 1]/b[d]`: of each b, its parent's filter, which
/// may raise, and its own.
fn two_filters() -> XmlQuery {
    let step = |name: &str| Step { axis: Axis::Child, name: Some(QName::local(name)) };
    let lookup = XmlLookup {
        pattern: PathPattern { steps: vec![step("a"), step("b")], target: Target::Element },
        comparisons: Vec::new(),
        row: None,
    };
    let x = PathPattern { steps: vec![step("c"), step("x")], target: Target::Attribute };
    let d = PathPattern { steps: vec![step("d")], target: Target::Element };
    let one = Comparison { op: CompOp::Eq, literal: Literal::Number(1.0) };
    XmlQuery {
        lookup,
        filters: vec![XmlFilter { at: 1, pattern: x, comparisons: vec![one] }, XmlFilter { at: 2, pattern: d, comparisons: Vec::new() }],
    }
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §3.2.2, §3.6]: a node
/// selected waits for each of its anchors: of `/a[c/@x = 1]/b[d]`, a b
/// whose own filter does not hold is false though its parent's raises, as
/// `and` may be [XQuery §3.6]; one whose filter holds raises its parent's
/// error; one of both holding holds; the tree's walk answers alike.
#[test]
fn a_node_waits_for_each_of_its_anchors() {
    let query = two_filters();
    for (text, answer) in [
        (r#"<a><b/><c x="no"/></a>"#, Ok(false)),
        (r#"<a><b><d/></b><c x="no"/></a>"#, Err(-301_i32)),
        (r#"<a><b><d/></b><c x="no"/><c x="1"/></a>"#, Ok(true)),
        (r#"<a><b/><b><d/></b><c x="1"/></a>"#, Ok(true)),
    ] {
        let sequence = Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap());
        assert_eq!(kind(&by_tree(&query, &sequence)), answer, "{text}");
        assert_eq!(kind(&query.matches_stored(&encode(&sequence))), answer, "{text}");
    }
}

/// Verifies: [EXTENSION] RULES.md D106, D18: a filter of a step whose node
/// is not fixed of a node selected, of descendant steps before and after
/// it, or past the lookup's steps, is refused, SQLCODE -910; more filters
/// than `MAX_FILTERS`, -900.
#[test]
fn filters_are_of_fixed_anchors_and_bounded() {
    let step = |axis, name: &str| Step { axis, name: Some(QName::local(name)) };
    let d = PathPattern { steps: vec![step(Axis::Child, "d")], target: Target::Element };
    let steps = vec![step(Axis::Descendant, "a"), step(Axis::Child, "b"), step(Axis::Descendant, "c")];
    let lookup = XmlLookup { pattern: PathPattern { steps, target: Target::Element }, comparisons: Vec::new(), row: None };
    let stored = encode(&Sequence::document(parse("<a><b><c/></b></a>", Syntax::Content, Whitespace::Preserve).unwrap()));
    let of = |at: usize, n: usize| XmlQuery {
        lookup: lookup.clone(),
        filters: vec![XmlFilter { at, pattern: d.clone(), comparisons: Vec::new() }; n],
    };
    assert_eq!(of(2, 1).matches_stored(&stored).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(of(4, 1).matches_stored(&stored).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(of(3, MAX_FILTERS + 1).matches_stored(&stored).unwrap_err().sqlcode(), -900_i32);
    assert!(!of(3, MAX_FILTERS).matches_stored(&stored).unwrap());
    assert_eq!(of(3, 1).anchors().unwrap(), [Anchored::Above(0)]);
    assert_eq!(of(0, 1).anchors().unwrap(), [Anchored::Below(0)]);
}
