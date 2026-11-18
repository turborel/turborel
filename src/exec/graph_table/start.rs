//! \[DESIGN\] Which end a path pattern's search starts from (RULES.md
//! D117). A pattern whose matches are the same searched either way may
//! be searched from its last vertex, its pattern reversed, each edge
//! pattern's orientation reversed with it, and each path found reversed
//! again, so its rows are the same.
//!
//! Either way gives the same paths if its selector keeps every path of a
//! (start, end) partition or every shortest one (none, ALL SHORTEST,
//! SHORTEST k GROUP) and no COST is read; no limit truncates, none caps a
//! partition's paths and DETERMINISTIC_PATHS is not asked for, each of
//! which depends on the order paths are found in; each prefilter reads
//! its own element alone, and no multiset alternation keeps the branches
//! it took. A hint of [GS §16 R16.8] decides first: GRAPH_START names
//! the end the search starts from, GRAPH_DIRECTION (e, REVERSE) of an
//! edge variable of the pattern starts it from the last vertex. Else, if
//! both its end vertices are filtered and the graph's statistics are
//! known (`stats/graph.rs`), it starts from the end of fewer candidate
//! vertices times the fan-out of its first step from them: of each end,
//! the rows of the vertex tables it may bind, each by the share its
//! filter keeps, and the edges per vertex of the edge tables next to it,
//! the way the pattern leaves it.

use crate::db::State;
use crate::graph::binding::PathBinding;
use crate::graph::limits::OnLimit;
use crate::graph::program::Selector;
use crate::graph::search::SearchOptions;
use crate::graph::view::{Orientation, Way};
use crate::logical::{Cond, Expr, GraphMatch, PathShape, PatternShape, TestShape, cond_columns, map_cond_columns};
use crate::physical::estimate::{Known, selectivity};
use crate::stats::graph::{Direction, GraphStatistics};

/// Of each path pattern of `graph`, the pattern searched, and whether it
/// is reversed: from its last vertex.
pub(super) fn oriented(db: &State, graph: &GraphMatch, options: &SearchOptions) -> Vec<(PathShape, bool)> {
    let mut stats: Option<GraphStatistics> = None;
    graph
        .paths
        .iter()
        .map(|path| {
            let Some((first, last)) = ends(&path.pattern).filter(|_| reversible(graph, path, options)) else {
                return (path.clone(), false);
            };
            let hinted = hint(graph, path, (first, last));
            let from_last = hinted.unwrap_or_else(|| {
                let known = stats.get_or_insert_with(|| db.graph_statistics(&graph.graph).unwrap_or_else(|| empty(graph)));
                cheaper_from_last(db, graph, known, path, (first, last))
            });
            if from_last { (reversed(graph, path, last), true) } else { (path.clone(), false) }
        })
        .collect()
}

/// Statistics of `graph` that know nothing.
fn empty(graph: &GraphMatch) -> GraphStatistics {
    GraphStatistics { graph: graph.graph.clone(), elements: Vec::new(), transitions: Vec::new() }
}

/// `bindings` as the pattern written finds them: each reversed, if
/// found by the reversed pattern.
pub(super) fn forward(bindings: Vec<PathBinding>, reversed: bool) -> Vec<PathBinding> {
    if !reversed {
        return bindings;
    }
    bindings.into_iter().map(reverse_binding).collect()
}

/// `binding`, its elements, the variables of each, and the branches at
/// each vertex in the reverse order.
pub(super) fn reverse_binding(mut binding: PathBinding) -> PathBinding {
    binding.elements.reverse();
    binding.vars.reverse();
    binding.tags.reverse();
    binding
}

/// Whether `path` of `graph` finds the same paths searched either way.
fn reversible(graph: &GraphMatch, path: &PathShape, options: &SearchOptions) -> bool {
    let selector = matches!(path.selector, Selector::All | Selector::AllShortest | Selector::ShortestGroups(_));
    let limits = options.limits.on_limit == OnLimit::Error && options.limits.max_paths_per_partition.is_none();
    selector && path.cost.is_none() && limits && !options.deterministic && plain(graph, &path.pattern)
}

/// Whether `shape` has no multiset alternation, no group's WHERE, and no
/// prefilter reading an element other than its own.
fn plain(graph: &GraphMatch, shape: &PatternShape) -> bool {
    let own =
        |t: &TestShape| t.filter.and_then(|k| graph.prefilters.get(k)).is_none_or(|p| p.reads.iter().all(|&(var, _)| Some(var) == p.own));
    match shape {
        PatternShape::Vertex(t) | PatternShape::Edge(t, _) => own(t),
        PatternShape::Concat(parts) | PatternShape::Union(parts) => parts.iter().all(|p| plain(graph, p)),
        PatternShape::Alternation(_) | PatternShape::Group(_, _, Some(_)) => false,
        PatternShape::Quantified(body, _, _) | PatternShape::Group(_, body, None) => plain(graph, body),
    }
}

/// The first and last vertex tests of `shape`, if it is a concatenation
/// beginning and ending with one.
fn ends(shape: &PatternShape) -> Option<(&TestShape, &TestShape)> {
    let PatternShape::Concat(parts) = shape else { return None };
    let (PatternShape::Vertex(first), PatternShape::Vertex(last)) = (parts.first()?, parts.last()?) else { return None };
    Some((first, last))
}

/// The end a hint of `graph` starts `path` from: true for its last
/// vertex, false for its first; none if no hint names one.
fn hint(graph: &GraphMatch, path: &PathShape, (first, last): (&TestShape, &TestShape)) -> Option<bool> {
    let name = |t: &TestShape| graph.vars.get(t.shape).map(|v| v.name.as_str());
    if let Some((start, _)) = &graph.hints.start {
        let is = |t: &TestShape| name(t).is_some_and(|n| n.eq_ignore_ascii_case(start));
        if is(last) || is(first) {
            return Some(is(last) && !is(first));
        }
    }
    let mut edges = Vec::new();
    edge_names(graph, &path.pattern, &mut edges);
    graph.hints.reverse.iter().any(|(e, _)| edges.iter().any(|n| n.eq_ignore_ascii_case(e))).then_some(true)
}

/// The names of the edge variables of `shape`, added to `out`.
fn edge_names<'g>(graph: &'g GraphMatch, shape: &PatternShape, out: &mut Vec<&'g str>) {
    match shape {
        PatternShape::Edge(t, _) => out.extend(graph.vars.get(t.shape).map(|v| v.name.as_str())),
        PatternShape::Vertex(_) => {}
        PatternShape::Concat(parts) | PatternShape::Union(parts) | PatternShape::Alternation(parts) => {
            for p in parts {
                edge_names(graph, p, out);
            }
        }
        PatternShape::Quantified(body, _, _) | PatternShape::Group(_, body, _) => edge_names(graph, body, out),
    }
}

/// Whether searching `path` from its last vertex is estimated cheaper:
/// both ends filtered, and of fewer candidates times fan-out.
fn cheaper_from_last(db: &State, graph: &GraphMatch, stats: &GraphStatistics, path: &PathShape, ends: (&TestShape, &TestShape)) -> bool {
    let (first, last) = ends;
    if first.filter.is_none() || last.filter.is_none() {
        return false;
    }
    let forward = first_edge(&path.pattern, false);
    let backward = first_edge(&path.pattern, true);
    let cost = |end: &TestShape, edge: Option<(&TestShape, Orientation)>| {
        Some(candidates(db, graph, end)? * edge.map_or(Some(1.0_f64), |(e, o)| fan_out(graph, stats, e, o))?)
    };
    match (cost(first, forward), cost(last, backward)) {
        (Some(from_first), Some(from_last)) => from_last < from_first,
        (Some(_) | None, None) | (None, Some(_)) => false,
    }
}

/// The first edge test of `shape` and its orientation, walking it from
/// its last vertex if `backward`, its orientation then reversed.
fn first_edge(shape: &PatternShape, backward: bool) -> Option<(&TestShape, Orientation)> {
    match shape {
        PatternShape::Edge(t, o) => Some((t, if backward { o.reversed() } else { *o })),
        PatternShape::Vertex(_) | PatternShape::Union(_) | PatternShape::Alternation(_) => None,
        PatternShape::Concat(parts) => {
            if backward {
                parts.iter().rev().find_map(|p| first_edge(p, true))
            } else {
                parts.iter().find_map(|p| first_edge(p, false))
            }
        }
        PatternShape::Quantified(body, _, _) | PatternShape::Group(_, body, _) => first_edge(body, backward),
    }
}

/// The vertices end test `end` of `graph` may start at: of each vertex
/// table it may bind, its rows by the share its filter keeps; none if a
/// table's statistics are not known.
fn candidates(db: &State, graph: &GraphMatch, end: &TestShape) -> Option<f64> {
    let var = graph.vars.get(end.shape)?;
    let filter = end.filter.and_then(|k| graph.prefilters.get(k)).map(|p| &p.cond);
    let cost = db.cost();
    let mut total = 0.0_f64;
    for (t, columns) in &var.tables {
        let stats = db.statistics(&graph.tables.get(*t)?.table)?;
        let rows = stats.rows()?;
        let kept = filter
            .and_then(|c| over_table(c, var.offset, columns))
            .map_or(1.0_f64, |c| selectivity(&c, Known { stats: Some(&*stats), cost: &cost }));
        total += rows * kept;
    }
    Some(total)
}

/// `cond`, over the match row, over the columns of an element table
/// instead, the variable's columns from `offset` each `columns` gives;
/// none if it reads another column, or one not a column of the table.
fn over_table(cond: &Cond, offset: usize, columns: &[Expr]) -> Option<Cond> {
    let mut read = Vec::new();
    cond_columns(cond, &mut read);
    let column = |c: usize| match columns.get(c.checked_sub(offset)?) {
        Some(Expr::Column(k)) => Some(*k),
        Some(_) | None => None,
    };
    if !read.iter().all(|&c| column(c).is_some()) {
        return None;
    }
    Some(map_cond_columns(cond.clone(), &|c| column(c).unwrap_or(c)))
}

/// The edges per vertex of a step along edge test `edge` the way
/// `orientation` allows: of each edge table it may bind, its fan-out at
/// the end it is left from, each way summed; none if one is not known.
fn fan_out(graph: &GraphMatch, stats: &GraphStatistics, edge: &TestShape, orientation: Orientation) -> Option<f64> {
    let var = graph.vars.get(edge.shape)?;
    let mut total = 0.0_f64;
    for (t, _) in &var.tables {
        let alias = &graph.tables.get(*t)?.alias;
        let at = |d: Direction| stats.degree(alias, d).and_then(|g| g.fan_out);
        if orientation.allows(Way::Right) || orientation.allows(Way::Undirected) {
            total += at(Direction::Out)?;
        }
        if orientation.allows(Way::Left) || orientation.allows(Way::Undirected) {
            total += at(Direction::In)?;
        }
    }
    Some(total)
}

/// `path` searched from its last vertex `last`: its pattern reversed, and
/// its starts the vertex tables `last` may bind.
fn reversed(graph: &GraphMatch, path: &PathShape, last: &TestShape) -> PathShape {
    let starts = graph.vars.get(last.shape).map(|v| v.tables.iter().map(|(t, _)| *t).collect()).unwrap_or_default();
    PathShape { pattern: flipped(&path.pattern), starts, ..path.clone() }
}

/// `shape` matched from its end: its parts in the reverse order, each
/// edge pattern's orientation reversed.
pub(super) fn flipped(shape: &PatternShape) -> PatternShape {
    match shape {
        PatternShape::Vertex(t) => PatternShape::Vertex(t.clone()),
        PatternShape::Edge(t, o) => PatternShape::Edge(t.clone(), o.reversed()),
        PatternShape::Concat(parts) => PatternShape::Concat(parts.iter().rev().map(flipped).collect()),
        PatternShape::Union(parts) => PatternShape::Union(parts.iter().map(flipped).collect()),
        PatternShape::Alternation(parts) => PatternShape::Alternation(parts.iter().map(flipped).collect()),
        PatternShape::Quantified(body, min, max) => PatternShape::Quantified(Box::new(flipped(body)), *min, *max),
        PatternShape::Group(r, body, f) => PatternShape::Group(*r, Box::new(flipped(body)), *f),
    }
}

// Unit tests: a pattern reversed and a path reversed back (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
