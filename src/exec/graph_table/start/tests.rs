//! Unit tests of a path pattern searched from its end (RULES.md D151).

use super::*;
use crate::graph::view::ElementId;

/// A test of element variable `shape`, of no label or filter.
fn test(shape: usize) -> TestShape {
    TestShape { var: u32::try_from(shape).ok(), shape, labels: None, filter: None }
}

/// The shapes of `shape`'s element tests in order, each edge's with its
/// orientation.
fn spelled(shape: &PatternShape) -> String {
    match shape {
        PatternShape::Vertex(t) => format!("({})", t.shape),
        PatternShape::Edge(t, o) => format!("{o}{}", t.shape),
        PatternShape::Concat(parts) => parts.iter().map(spelled).collect(),
        PatternShape::Union(parts) => parts.iter().map(spelled).collect::<Vec<_>>().join("|"),
        PatternShape::Alternation(parts) => parts.iter().map(spelled).collect::<Vec<_>>().join("|+|"),
        PatternShape::Quantified(body, min, max) => format!("[{}]{{{min},{max:?}}}", spelled(body)),
        PatternShape::Group(r, body, _) => format!("{r}[{}]", spelled(body)),
    }
}

/// Verifies: [DESIGN] RULES.md D151: a pattern reversed is its parts in
/// the reverse order, within a quantifier and a group too, each edge
/// pattern's orientation reversed, an undirected one kept; its first and
/// last vertex tests are its ends, and its first edge from either end is
/// found, its orientation as walked from there.
#[test]
fn a_pattern_is_reversed() {
    let inner = PatternShape::Concat(vec![PatternShape::Edge(test(1), Orientation::RIGHT), PatternShape::Vertex(test(2))]);
    let pattern = PatternShape::Concat(vec![
        PatternShape::Vertex(test(0)),
        PatternShape::Quantified(Box::new(inner), 1, Some(3)),
        PatternShape::Edge(test(3), Orientation::UNDIRECTED),
        PatternShape::Group(crate::graph::program::Restrictor::Trail, Box::new(PatternShape::Edge(test(4), Orientation::LEFT)), None),
        PatternShape::Vertex(test(5)),
    ]);
    assert_eq!(spelled(&pattern), "(0)[->1(2)]{1,Some(3)}~3TRAIL[<-4](5)");
    assert_eq!(spelled(&flipped(&pattern)), "(5)TRAIL[->4]~3[(2)<-1]{1,Some(3)}(0)");
    assert_eq!(spelled(&flipped(&flipped(&pattern))), spelled(&pattern));
    assert_eq!(ends(&pattern).map(|(f, l)| (f.shape, l.shape)), Some((0, 5)));
    assert!(ends(&PatternShape::Vertex(test(0))).is_none());
    assert_eq!(first_edge(&pattern, false).map(|(t, o)| (t.shape, o)), Some((1, Orientation::RIGHT)));
    assert_eq!(first_edge(&pattern, true).map(|(t, o)| (t.shape, o)), Some((4, Orientation::RIGHT)));
}

/// Verifies: [DESIGN] RULES.md D151: a path found from the end is given
/// as the pattern written finds it: its elements, their variables and the
/// branches at each vertex in the reverse order; one found from the
/// first vertex is given as found.
#[test]
fn a_path_is_reversed_back() {
    let e = |row: u64| ElementId::new(0, row);
    let found =
        PathBinding { elements: vec![e(3), e(10), e(1)], vars: vec![vec![2], vec![1], vec![0]], tags: vec![vec![7], vec![8]], cost: None };
    let back = forward(vec![found.clone()], true);
    assert_eq!(
        back.first().map(|b| (b.elements.clone(), b.vars.clone(), b.tags.clone())),
        Some((vec![e(1), e(10), e(3)], vec![vec![0], vec![1], vec![2]], vec![vec![8], vec![7]]))
    );
    assert_eq!(forward(vec![found.clone()], false), vec![found]);
}
