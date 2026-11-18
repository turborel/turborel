//! \[DESIGN\] What a GRAPH_TABLE's searches are chosen by (RULES.md D118):
//! the rows of each of its stored element tables, as the storage counts
//! them, the edges a step of each path pattern may follow, as its graph's
//! statistics count the label transitions its edge patterns' labels
//! match, and the settings of the workers (`logical::GraphSizing`).

use super::Context;
use crate::graph::program::LabelExpr;
use crate::logical::{ElementRead, GraphMatch, GraphSizing, PatternShape};
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

/// Of path pattern `shape` of a graph of `statistics` and label names
/// `labels`, the most edges any of its edge patterns' labels match, of the
/// label transitions counted; none if one's labels are not a label or an
/// alternation of labels, or none were counted.
fn step(statistics: &GraphStatistics, labels: &[String], shape: &PatternShape) -> Option<u64> {
    let mut most: Option<f64> = None;
    edges(shape, &mut |test| {
        let edges = matched(statistics, labels, test);
        most = most.zip(edges).map(|(a, b)| a.max(b)).or(edges);
        edges.is_some()
    })
    .then_some(())?;
    // A sum of row counts, rounded: no float converts to an integer without a cast (R-NUM-2).
    most.and_then(|m| format!("{:.0}", m.max(0.0_f64)).parse().ok())
}

/// Calls `each` with the label expression of each edge pattern of
/// `shape`, while it answers true; whether every call did.
fn edges(shape: &PatternShape, each: &mut dyn FnMut(Option<&LabelExpr>) -> bool) -> bool {
    match shape {
        PatternShape::Vertex(_) => true,
        PatternShape::Edge(test, _) => each(test.labels.as_ref()),
        PatternShape::Concat(parts) | PatternShape::Union(parts) | PatternShape::Alternation(parts) => parts.iter().all(|p| edges(p, each)),
        PatternShape::Quantified(inner, _, _) | PatternShape::Group(_, inner, _) => edges(inner, each),
    }
}

/// The edges of the label transitions an edge pattern of label expression
/// `test` matches, any edge if none is written.
fn matched(statistics: &GraphStatistics, labels: &[String], test: Option<&LabelExpr>) -> Option<f64> {
    match test {
        None | Some(LabelExpr::Wildcard) => statistics.transitions(None, None, None),
        Some(LabelExpr::Label(id)) => {
            let name = labels.get(usize::try_from(*id).ok()?)?;
            Some(statistics.transitions(None, Some(name), None).unwrap_or(0.0_f64))
        }
        Some(LabelExpr::Or(a, b)) => Some(matched(statistics, labels, Some(a))? + matched(statistics, labels, Some(b))?),
        Some(LabelExpr::Not(_) | LabelExpr::And(..)) => None,
    }
}
