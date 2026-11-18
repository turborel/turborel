// Unit tests of path search (RULES.md R-TEST-1): the [P1] bank fixture's
// conformance tests, every restrictor and selector against a brute-force
// enumerator, the cross-iteration extension of [P2], the parallel and
// multi-source searches, the limits and cancellation.

mod bank;
mod increasing;
mod oracle;
mod runs;

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Result;
use crate::graph::binding::PathBinding;
use crate::graph::in_memory::{InMemoryGraph, Property};
use crate::graph::program::{
    Bound, ElementTest, Filter, Group, LabelExpr, PathPattern, Pattern, Quantified, Read, Restrictor, VarId, VarKind, Variable,
};
use crate::graph::search::{PathSearch, SearchOptions};
use crate::graph::view::{ElementId, LabelId, Orientation};

/// A vertex pattern of `var`, `labels` and `filter`.
fn node(var: Option<VarId>, labels: Option<LabelExpr>, filter: Option<Filter>) -> Pattern {
    Pattern::Vertex(ElementTest { var, labels, filter })
}

/// An edge pattern of `var`, `labels` and `filter`, pointing right.
fn right(var: Option<VarId>, labels: Option<LabelExpr>, filter: Option<Filter>) -> Pattern {
    Pattern::Edge(ElementTest { var, labels, filter }, Orientation::RIGHT)
}

/// `pattern` quantified `{min, max}`.
fn times(pattern: Pattern, min: u32, max: Option<u32>) -> Pattern {
    Pattern::Quantified(Box::new(Quantified { body: pattern, min, max }))
}

/// `pattern` parenthesized, with `restrictor` and `filter`.
fn group(restrictor: Restrictor, pattern: Pattern, filter: Option<Filter>) -> Pattern {
    Pattern::Group(Box::new(Group { restrictor, body: pattern, filter }))
}

/// Label `label`, or any label if it is 0.
fn label(label: LabelId) -> Option<LabelExpr> {
    (label != 0).then_some(LabelExpr::Label(label))
}

/// A prefilter reading `reads`, testing `test`.
fn filter(reads: &[Read], test: impl Fn(&Bound<'_>) -> Result<bool> + Send + Sync + 'static) -> Filter {
    Filter { reads: reads.to_vec(), test: Arc::new(test) }
}

/// Vertex variables named `vertices`, then edge variables `edges`.
fn variables(vertices: &[&str], edges: &[&str]) -> Vec<Variable> {
    let v = vertices.iter().map(|n| Variable { name: (*n).to_owned(), kind: VarKind::Vertex });
    v.chain(edges.iter().map(|n| Variable { name: (*n).to_owned(), kind: VarKind::Edge })).collect()
}

/// The paths `pattern` matches in `graph` from `starts`, to `ends` if given.
fn search(
    pattern: &PathPattern,
    graph: &InMemoryGraph,
    starts: &[ElementId],
    ends: Option<&[ElementId]>,
    options: SearchOptions,
) -> Result<Vec<PathBinding>> {
    PathSearch::new(pattern, graph, options)?.run(starts, ends)?.collect()
}

/// The paths `pattern` matches from every vertex, as names.
fn spelled(pattern: &PathPattern, graph: &InMemoryGraph, names: &HashMap<ElementId, String>) -> Result<Vec<String>> {
    let mut out: Vec<String> =
        search(pattern, graph, &graph.vertices(), None, SearchOptions::default())?.iter().map(|p| spell(p, names)).collect();
    out.sort();
    Ok(out)
}

/// `path` as its elements' names, comma separated.
fn spell(path: &PathBinding, names: &HashMap<ElementId, String>) -> String {
    path.elements.iter().map(|e| names.get(e).cloned().unwrap_or_else(|| e.to_string())).collect::<Vec<_>>().join(",")
}

/// A generator of a fixed sequence, for random graphs the same on every
/// run (RULES.md R-TEST-5): xorshift64*.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12_u32;
        self.0 ^= self.0 << 25_u32;
        self.0 ^= self.0 >> 27_u32;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next().checked_rem(n).unwrap()
    }
}

/// A random directed graph G(n, p) of one vertex table and one edge
/// table, each edge with an integer property `ts` below `range`, and
/// label 1 or 2 at random.
fn random_graph(seed: u64, n: u64, per_thousand: u64, range: u64) -> InMemoryGraph {
    let mut rng = Seeded(seed);
    let mut graph = InMemoryGraph::new();
    let vertices: Vec<ElementId> = (0..n).map(|_| graph.vertex(0, &[])).collect();
    for &from in &vertices {
        for &to in &vertices {
            if rng.below(1000) < per_thousand {
                let lab = u32::try_from(rng.below(2)).unwrap().wrapping_add(1);
                let e = graph.edge(1, (from, to), true, &[lab]);
                graph.set(e, "ts", Property::Int(i64::try_from(rng.below(range)).unwrap()));
                graph.set_cost(e, f64::from(u32::try_from(rng.below(4)).unwrap().wrapping_add(1)));
            }
        }
    }
    graph
}

/// A random sparse directed graph of `n` vertices and about `degree`
/// edges out of each, as `random_graph` labels them.
fn sparse_graph(seed: u64, n: u64, degree: u64, range: u64) -> InMemoryGraph {
    let mut rng = Seeded(seed);
    let mut graph = InMemoryGraph::new();
    let vertices: Vec<ElementId> = (0..n).map(|_| graph.vertex(0, &[])).collect();
    for &from in &vertices {
        for _ in 0..degree {
            let to = vertices[usize::try_from(rng.below(n)).unwrap()];
            let e = graph.edge(1, (from, to), true, &[1]);
            graph.set(e, "ts", Property::Int(i64::try_from(rng.below(range)).unwrap()));
        }
    }
    graph
}
