//! \[DESIGN\] What a GRAPH_TABLE's searches are chosen by (RULES.md D118):
//! the rows of each of its stored element tables, as the storage counts
//! them, the edges a step of each path pattern may follow, as its graph's
//! statistics count the label transitions (source, edge, destination)
//! its edge patterns and the node patterns beside them match, and the
//! settings of the workers (`logical::GraphSizing`).

use super::Context;
use crate::graph::program::LabelExpr;
use crate::graph::view::{Orientation, Way};
use crate::logical::{ElementRead, GraphMatch, GraphSizing, PatternShape, TestShape};
use crate::settings::{GRAPH_PARALLEL_MIN_STATES, GRAPH_THREADS};
use crate::stats::graph::GraphStatistics;

/// The sizing of GRAPH_TABLE `g`.
pub(super) fn of(cx: Context<'_>, g: &GraphMatch) -> GraphSizing {
    let rows = g
        .tables
        .iter()
        .map(|t| match t.read {
            ElementRead::Stored => cx.db.storage_of(&t.table).ok().and_then(|s| u64::try_from(s.row_count()).ok()),
            ElementRead::Held(_) => None,
        })
        .collect();
    let statistics = cx.db.graph_statistics(&g.graph);
    let steps = g.paths.iter().map(|p| statistics.as_ref().and_then(|s| step(s, &g.labels, &p.pattern))).collect();
    let (threads, parallel_min_states) = (cx.db.settings.count(GRAPH_THREADS), cx.db.settings.count(GRAPH_PARALLEL_MIN_STATES));
    GraphSizing { rows, threads, parallel_min_states, steps }
}

/// An edge pattern and the label expressions of the node patterns
/// written beside it, none of a side with none: its left, its own, its
/// right, and its orientation.
type Step<'a> = (Option<&'a LabelExpr>, Option<&'a LabelExpr>, Option<&'a LabelExpr>, Orientation);

/// Of path pattern `shape` of a graph of `statistics` and label names
/// `labels`, the most edges any of its steps matches, of the label
/// transitions counted; none if an edge pattern's labels are of `!` or
/// `&`, or none were counted.
fn step(statistics: &GraphStatistics, labels: &[String], shape: &PatternShape) -> Option<u64> {
    let mut most: Option<f64> = None;
    steps(shape, &mut |each| {
        let edges = matched(statistics, labels, each);
        most = most.zip(edges).map(|(a, b)| a.max(b)).or(edges);
        edges.is_some()
    })
    .then_some(())?;
    // A sum of row counts, rounded: no float converts to an integer without a cast (R-NUM-2).
    most.and_then(|m| format!("{:.0}", m.max(0.0_f64)).parse().ok())
}

/// Calls `each` with each edge pattern of `shape` and the node patterns
/// beside it, while it answers true; whether every call did.
fn steps<'a>(shape: &'a PatternShape, each: &mut dyn FnMut(Step<'a>) -> bool) -> bool {
    match shape {
        PatternShape::Vertex(_) => true,
        PatternShape::Edge(test, orientation) => each((None, test.labels.as_ref(), None, *orientation)),
        PatternShape::Concat(parts) => parts.iter().enumerate().all(|(i, part)| match part {
            PatternShape::Edge(test, orientation) => {
                let beside = |at: Option<usize>| at.and_then(|at| vertex(parts.get(at)?));
                each((beside(i.checked_sub(1)), test.labels.as_ref(), beside(i.checked_add(1)), *orientation))
            }
            PatternShape::Vertex(_)
            | PatternShape::Concat(_)
            | PatternShape::Union(_)
            | PatternShape::Alternation(_)
            | PatternShape::Quantified(..)
            | PatternShape::Group(..) => steps(part, each),
        }),
        PatternShape::Union(parts) | PatternShape::Alternation(parts) => parts.iter().all(|p| steps(p, each)),
        PatternShape::Quantified(inner, _, _) | PatternShape::Group(_, inner, _) => steps(inner, each),
    }
}

/// The label expression of node pattern `shape`, if it is one with one.
fn vertex(shape: &PatternShape) -> Option<&LabelExpr> {
    let PatternShape::Vertex(TestShape { labels, .. }) = shape else { return None };
    labels.as_ref()
}

/// The labels a label expression matches: any, or these.
enum Names<'a> {
    Any,
    Of(Vec<&'a str>),
}

/// The labels `test` matches, of label names `labels`; none if it is of
/// `!` or `&`.
fn names<'a>(labels: &'a [String], test: Option<&LabelExpr>) -> Option<Names<'a>> {
    match test {
        None | Some(LabelExpr::Wildcard) => Some(Names::Any),
        Some(LabelExpr::Label(id)) => Some(Names::Of(vec![labels.get(usize::try_from(*id).ok()?)?.as_str()])),
        Some(LabelExpr::Or(a, b)) => match (names(labels, Some(a))?, names(labels, Some(b))?) {
            (Names::Of(mut x), Names::Of(y)) => {
                x.extend(y);
                Some(Names::Of(x))
            }
            (Names::Any, _) | (_, Names::Any) => Some(Names::Any),
        },
        Some(LabelExpr::Not(_) | LabelExpr::And(..)) => None,
    }
}

/// The edges of the label transitions step `(left, edge, right,
/// orientation)` matches, each way it walks; none if the edge's labels
/// are of `!` or `&`, or no transition was counted. A node pattern's `!`
/// or `&` matches any label.
fn matched(statistics: &GraphStatistics, labels: &[String], (left, edge, right, orientation): Step<'_>) -> Option<f64> {
    statistics.transitions(None, None, None)?;
    let of = |names: Option<Names<'_>>| -> Vec<Option<String>> {
        match names {
            None | Some(Names::Any) => vec![None],
            Some(Names::Of(list)) => list.into_iter().map(|n| Some(n.to_owned())).collect(),
        }
    };
    let (left, right) = (of(names(labels, left)), of(names(labels, right)));
    let edge = of(Some(names(labels, edge)?));
    let either = orientation.allows(Way::Undirected);
    let ways = [(orientation.allows(Way::Right) || either, &left, &right), (orientation.allows(Way::Left) || either, &right, &left)];
    let mut total = 0.0_f64;
    for (_, source, destination) in ways.into_iter().filter(|(walked, _, _)| *walked) {
        for (s, e, d) in source.iter().flat_map(|s| edge.iter().flat_map(move |e| destination.iter().map(move |d| (s, e, d)))) {
            total += statistics.transitions(s.as_deref(), e.as_deref(), d.as_deref()).unwrap_or(0.0_f64);
        }
    }
    Some(total)
}
