//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18 Description 2i v 1]
//! Noding a collection into the nodes and edges of a planar graph: each
//! segment is cut where another crosses or touches it, or where a point
//! lies on it, until no two pieces meet but at their ends. Whether pieces
//! meet is decided exactly; where two cross inside both, the crossing is
//! rounded to binary64 and made a vertex of both, and the rounds repeat
//! until nothing is cut.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::Point;
use super::predicates::{on_segment, same};
use crate::error::{Code, Result, SqlError};
use crate::spatial::planar::orient2d as orient;

/// The rounds of cutting after which noding gives up: each round only
/// cuts pieces whose rounded crossings moved, which settles in one or two.
const MAX_ROUNDS: usize = 16;

/// The noded graph: its nodes, in order of position, and its edges, each
/// a line from a node to a node.
#[derive(Debug, Default)]
pub(super) struct Graph {
    /// The nodes.
    pub nodes: Vec<Point>,
    /// The edges.
    pub edges: Vec<Vec<Point>>,
}

/// How two positions compare: by x, then y.
fn by_position(a: &Point, b: &Point) -> Ordering {
    let ([ax, ay], [bx, by]) = (a, b);
    ax.total_cmp(bx).then(ay.total_cmp(by))
}

/// Whether two orientations are strictly opposite.
fn opposite(x: Ordering, y: Ordering) -> bool {
    matches!((x, y), (Ordering::Less, Ordering::Greater) | (Ordering::Greater, Ordering::Less))
}

/// The crossing of segments `a`-`b` and `c`-`d`, which cross inside both,
/// rounded.
fn crossing(first: [Point; 2], second: [Point; 2]) -> Point {
    let [[ax, ay], [bx, by]] = first;
    let [[cx, cy], [dx, dy]] = second;
    let denominator = (bx - ax) * (dy - cy) - (by - ay) * (dx - cx);
    let along = ((cx - ax) * (dy - cy) - (cy - ay) * (dx - cx)) / denominator;
    [ax + along * (bx - ax), ay + along * (by - ay)]
}

/// The cuts each of segments `s` and `t` takes from the other: the
/// crossing if they cross inside both, else each end of one inside the
/// other.
fn cuts(first: [Point; 2], second: [Point; 2]) -> (Vec<Point>, Vec<Point>) {
    let ([a0, a1], [b0, b1]) = (first, second);
    if opposite(orient(a0, a1, b0), orient(a0, a1, b1)) && opposite(orient(b0, b1, a0), orient(b0, b1, a1)) {
        let at = crossing(first, second);
        return (vec![at], vec![at]);
    }
    let inside = |p: Point, [from, to]: [Point; 2]| on_segment(p, from, to) && !same(p, from) && !same(p, to);
    let on_first = [b0, b1].into_iter().filter(|p| inside(*p, first)).collect();
    let on_second = [a0, a1].into_iter().filter(|p| inside(*p, second)).collect();
    (on_first, on_second)
}

/// Whether the boxes of two segments meet.
fn boxes([a, b]: [Point; 2], [c, d]: [Point; 2]) -> bool {
    let ([ax, ay], [bx, by], [cx, cy], [dx, dy]) = (a, b, c, d);
    ax.max(bx) >= cx.min(dx) && cx.max(dx) >= ax.min(bx) && ay.max(by) >= cy.min(dy) && cy.max(dy) >= ay.min(by)
}

/// Segment `s` cut at `at`, in order along it, into pieces of length.
fn pieces(s: [Point; 2], mut at: Vec<Point>) -> Vec<[Point; 2]> {
    let [a, b] = s;
    let [ax, ay] = a;
    let along = |[x, y]: &Point| (x - ax) * (x - ax) + (y - ay) * (y - ay);
    at.sort_by(|p, q| along(p).total_cmp(&along(q)));
    let mut out = Vec::new();
    let mut from = a;
    for p in at.into_iter().chain([b]) {
        if !same(from, p) {
            out.push([from, p]);
            from = p;
        }
    }
    out
}

/// One round of cutting: the segments after it, and whether any was cut.
fn round(segments: &[[Point; 2]], points: &[Point]) -> (Vec<[Point; 2]>, bool) {
    let mut at: Vec<Vec<Point>> = vec![Vec::new(); segments.len()];
    for (i, s) in segments.iter().enumerate() {
        for (j, t) in segments.iter().enumerate().skip(i.saturating_add(1)) {
            if !boxes(*s, *t) {
                continue;
            }
            let (on_s, on_t) = cuts(*s, *t);
            if let Some(v) = at.get_mut(i) {
                v.extend(on_s);
            }
            if let Some(v) = at.get_mut(j) {
                v.extend(on_t);
            }
        }
        let [a, b] = *s;
        let on = points.iter().filter(|p| on_segment(**p, a, b) && !same(**p, a) && !same(**p, b));
        if let Some(v) = at.get_mut(i) {
            v.extend(on);
        }
    }
    let cut = at.iter().any(|v| !v.is_empty());
    (segments.iter().zip(at).flat_map(|(s, cuts)| pieces(*s, cuts)).collect(), cut)
}

/// [Description 2i v 1] The graph of `points` and `lines`: noded, each
/// point, each end of a line and each place where other than two pieces
/// meet a node.
///
/// # Errors
///
/// SQLCODE -900 if cutting does not settle within `MAX_ROUNDS`.
pub(super) fn node(points: &[Point], lines: &[Vec<Point>]) -> Result<Graph> {
    let mut segments: Vec<[Point; 2]> = lines
        .iter()
        .flat_map(|l| l.windows(2).filter_map(|w| if let [a, b] = w { Some([*a, *b]) } else { None }))
        .filter(|[a, b]| !same(*a, *b))
        .collect();
    let mut settled = false;
    for _ in 0..MAX_ROUNDS {
        let (next, cut) = round(&segments, points);
        segments = next;
        if !cut {
            settled = true;
            break;
        }
    }
    if !settled {
        return Err(SqlError::new(Code::Unsupported, "[DESIGN] noding the collection did not settle"));
    }
    let mut ends: Vec<Point> = points.to_vec();
    ends.extend(lines.iter().flat_map(|l| [l.first(), l.last()]).flatten());
    Ok(chains(&segments, &ends))
}

/// The graph of noded `segments`, the positions `ends` nodes among others.
fn chains(segments: &[[Point; 2]], ends: &[Point]) -> Graph {
    let mut vertices: Vec<Point> = segments.iter().flatten().chain(ends).copied().collect();
    vertices.sort_by(by_position);
    vertices.dedup_by(|a, b| same(*a, *b));
    let index = |p: &Point| vertices.binary_search_by(|q| by_position(q, p)).unwrap_or_default();
    let mut links: BTreeSet<(usize, usize)> = BTreeSet::new();
    for [a, b] in segments {
        let (i, j) = (index(a), index(b));
        links.insert((i.min(j), i.max(j)));
    }
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); vertices.len()];
    for (i, j) in &links {
        if let Some(v) = next.get_mut(*i) {
            v.push(*j);
        }
        if let Some(v) = next.get_mut(*j) {
            v.push(*i);
        }
    }
    let mut is_node: Vec<bool> = next.iter().map(|n| n.len() != 2).collect();
    for p in ends {
        if let Some(v) = is_node.get_mut(index(p)) {
            *v = true;
        }
    }
    let mut graph = Graph::default();
    let mut used: BTreeSet<(usize, usize)> = BTreeSet::new();
    // Chains from nodes first; what is left are rings no node is on.
    for start in 0..vertices.len() {
        if is_node.get(start).copied().unwrap_or_default() {
            walk_from(start, (&next, &mut is_node), &mut used, &mut graph, &vertices);
        }
    }
    for start in 0..vertices.len() {
        walk_from(start, (&next, &mut is_node), &mut used, &mut graph, &vertices);
    }
    graph.nodes = vertices.iter().zip(&is_node).filter(|(_, n)| **n).map(|(p, _)| *p).collect();
    graph
}

/// Each chain from vertex `start`, if it is a node or lies on a ring of no
/// node (when it becomes one), walked to the next node and added to
/// `graph`.
fn walk_from(
    start: usize,
    (next, is_node): (&[Vec<usize>], &mut Vec<bool>),
    used: &mut BTreeSet<(usize, usize)>,
    graph: &mut Graph,
    vertices: &[Point],
) {
    let neighbours = next.get(start).cloned().unwrap_or_default();
    for first in neighbours {
        if used.contains(&(start.min(first), start.max(first))) {
            continue;
        }
        if !is_node.get(start).copied().unwrap_or(true) {
            // A vertex of a ring no node is on: it becomes the ring's node.
            if let Some(v) = is_node.get_mut(start) {
                *v = true;
            }
        }
        let mut chain = vec![start];
        let (mut from, mut at) = (start, first);
        loop {
            used.insert((from.min(at), from.max(at)));
            chain.push(at);
            if is_node.get(at).copied().unwrap_or(true) {
                break;
            }
            let Some(to) = next.get(at).and_then(|n| n.iter().find(|v| **v != from && !used.contains(&((**v).min(at), (**v).max(at)))))
            else {
                break;
            };
            (from, at) = (at, *to);
        }
        graph.edges.push(chain.iter().filter_map(|i| vertices.get(*i).copied()).collect());
    }
}
