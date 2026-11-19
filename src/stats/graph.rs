//! \[DESIGN\] Statistics of a property graph (RULES.md D151), for the
//! planner's estimates of a pattern's matches and the executor's choice of
//! where a search starts: of each element table its rows, and the
//! statistics of each property that is a column alone; of each edge table
//! its out-degree and in-degree distributions; and of each label
//! transition, a source label, an edge label and a destination label, the
//! edges that make it.
//!
//! They are made of the statistics of the element tables (`stats.rs`), as
//! those are gathered: no pass of their own. An edge table's degrees at an
//! end are the frequencies of its endpoint columns' values
//! (`stats/frequencies.rs`): each vertex with an edge there is one value.
//! Of an endpoint of several columns they are those of its column with
//! the most distinct values, its maximum the least of its columns' maxima,
//! each a bound above the degrees of the whole key. A table whose
//! statistics are not gathered yet gives none, and asking for them makes
//! them due (`State::statistics`); so they are recovered as a table's are,
//! gathered again once a database reopened asks for them.

use std::sync::Arc;

use super::{Frequencies, Statistics};
use crate::catalog::{ElementKind, ElementTableDesc, EndpointDesc, GraphDesc, QualifiedName, identifier};
use crate::value::Value;

/// Of an edge table, the end its edges are counted at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// At its source: each vertex's out-degree.
    Out,
    /// At its destination: each vertex's in-degree.
    In,
}

/// \[DESIGN\] An edge table's degrees at one end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Degree {
    /// The edges of each vertex with one there, on average.
    pub average: f64,
    /// The edges of each vertex of the vertex table at that end, on
    /// average, those with none among them, if its rows are known: the
    /// fan-out of a step from a vertex of it.
    pub fan_out: Option<f64>,
    /// The most, the median and the percentiles of the edges of a vertex
    /// with one there.
    pub frequencies: Frequencies,
}

/// \[DESIGN\] A property's statistics, of a property that is a column
/// alone; none of one that is an expression.
#[derive(Debug, Clone)]
pub struct PropertyStatistics {
    /// The property's name.
    pub name: String,
    /// Its distinct non-null values.
    pub distinct: Option<f64>,
    /// The share of its values that are NULL.
    pub nulls: Option<f64>,
    /// Its least and greatest values.
    pub bounds: Option<(Value, Value)>,
}

/// \[DESIGN\] An element table's statistics.
#[derive(Debug, Clone)]
pub struct ElementStatistics {
    /// Its alias in the graph.
    pub alias: String,
    /// Of vertices or of edges.
    pub kind: ElementKind,
    /// Its rows, if its table's statistics are gathered.
    pub rows: Option<f64>,
    /// Of an edge table, its degrees at its source.
    pub out_degree: Option<Degree>,
    /// Of an edge table, its degrees at its destination.
    pub in_degree: Option<Degree>,
    /// Its properties, of all its labels, each once.
    pub properties: Vec<PropertyStatistics>,
}

/// \[DESIGN\] A label transition: the edges from a vertex of one label
/// along an edge of another to a vertex of a third.
#[derive(Debug, Clone)]
pub struct Transition {
    /// The source vertex's label.
    pub source: String,
    /// The edge's label.
    pub edge: String,
    /// The destination vertex's label.
    pub destination: String,
    /// The edges, those whose endpoint columns are not NULL.
    pub edges: f64,
}

/// What a graph's statistics are made of, of one of its tables: the
/// table's statistics and the names of its columns, in order.
pub type TableOf<'a> = &'a dyn Fn(&QualifiedName) -> Option<(Arc<dyn Statistics>, Vec<String>)>;

/// \[DESIGN\] A property graph's statistics.
#[derive(Debug, Clone)]
pub struct GraphStatistics {
    /// The graph.
    pub graph: QualifiedName,
    /// Its element tables', in the graph's order.
    pub elements: Vec<ElementStatistics>,
    /// Its label transitions, each once, of edge tables whose rows are
    /// known, in the order first met.
    pub transitions: Vec<Transition>,
}

impl GraphStatistics {
    /// Of `graph`, each of its tables' statistics as `table` gives them.
    pub fn of(graph: &GraphDesc, table: TableOf<'_>) -> GraphStatistics {
        let elements: Vec<ElementStatistics> = graph.elements.iter().map(|e| element(graph, e, table)).collect();
        let mut transitions: Vec<Transition> = Vec::new();
        for e in &graph.elements {
            let (Some(ends), Some(edges)) = (&e.endpoints, edges_of(e, table)) else { continue };
            let (Some(from), Some(to)) = (graph.element(&ends.source.vertex), graph.element(&ends.destination.vertex)) else { continue };
            let triples = from.labels.iter().flat_map(|s| e.labels.iter().flat_map(move |l| to.labels.iter().map(move |d| (s, l, d))));
            for (s, l, d) in triples {
                add(&mut transitions, (&s.name, &l.name, &d.name), edges);
            }
        }
        GraphStatistics { graph: graph.name.clone(), elements, transitions }
    }

    /// The element table of alias `alias`'s statistics.
    pub fn element(&self, alias: &str) -> Option<&ElementStatistics> {
        self.elements.iter().find(|e| e.alias == alias)
    }

    /// The rows of element table `alias`, if known.
    pub fn rows(&self, alias: &str) -> Option<f64> {
        self.element(alias)?.rows
    }

    /// The degrees of edge table `alias` at the end `direction` names, if
    /// known.
    pub fn degree(&self, alias: &str, direction: Direction) -> Option<Degree> {
        let e = self.element(alias)?;
        match direction {
            Direction::Out => e.out_degree,
            Direction::In => e.in_degree,
        }
    }

    /// The edges of the label transitions matching `source`, `edge` and
    /// `destination`, any label of a part not given; none if no edge
    /// table's rows are known.
    pub fn transitions(&self, source: Option<&str>, edge: Option<&str>, destination: Option<&str>) -> Option<f64> {
        let is = |want: Option<&str>, label: &str| want.is_none_or(|w| w == label);
        let matching = self.transitions.iter().filter(|t| is(source, &t.source) && is(edge, &t.edge) && is(destination, &t.destination));
        matching.map(|t| t.edges).reduce(|a, b| a + b)
    }

    /// Property `name` of element table `alias`'s statistics.
    pub fn property(&self, alias: &str, name: &str) -> Option<&PropertyStatistics> {
        self.element(alias)?.properties.iter().find(|p| p.name == name)
    }
}

/// Adds `edges` to the transition `labels`, or makes it.
fn add(transitions: &mut Vec<Transition>, (source, edge, destination): (&str, &str, &str), edges: f64) {
    if let Some(t) = transitions.iter_mut().find(|t| t.source == source && t.edge == edge && t.destination == destination) {
        t.edges += edges;
        return;
    }
    transitions.push(Transition { source: source.to_owned(), edge: edge.to_owned(), destination: destination.to_owned(), edges });
}

/// The statistics of element table `e` of `graph`.
fn element(graph: &GraphDesc, e: &ElementTableDesc, table: TableOf<'_>) -> ElementStatistics {
    let found = table(&e.table);
    let rows = found.as_ref().and_then(|(s, _)| s.rows());
    let properties = e
        .properties()
        .into_iter()
        .map(|p| {
            let column = found.as_ref().and_then(|(s, names)| Some((s, names.iter().position(|c| identifier(c) == p.expression)?)));
            PropertyStatistics {
                name: p.name.clone(),
                distinct: column.and_then(|(s, c)| s.distinct(c)),
                nulls: column.and_then(|(s, c)| s.nulls(c)),
                bounds: column.and_then(|(s, c)| s.bounds(c)),
            }
        })
        .collect();
    let degree = |end: Option<&EndpointDesc>| {
        let end = end?;
        let vertex_rows = graph.element(&end.vertex).and_then(|v| table(&v.table)).and_then(|(s, _)| s.rows());
        degree(found.as_ref()?, end, vertex_rows)
    };
    ElementStatistics {
        alias: e.alias.clone(),
        kind: e.kind,
        rows,
        out_degree: degree(e.source()),
        in_degree: degree(e.destination()),
        properties,
    }
}

/// The places of `end`'s columns among `names`, if each is there.
fn places(end: &EndpointDesc, names: &[String]) -> Option<Vec<usize>> {
    end.columns.iter().map(|c| names.iter().position(|n| n == c)).collect()
}

/// The edges of an edge table at `end`, its statistics `stats` and its
/// columns `names`: those whose endpoint columns are not NULL.
fn present((stats, names): &(Arc<dyn Statistics>, Vec<String>), end: &EndpointDesc) -> Option<f64> {
    let nulls = places(end, names)?.into_iter().filter_map(|c| stats.nulls(c)).fold(0.0_f64, f64::max);
    Some(stats.rows()? * (1.0_f64 - nulls).clamp(0.0_f64, 1.0_f64))
}

/// The edges of edge table `e` that make a transition, if its rows are
/// known: those with neither end NULL, at most.
fn edges_of(e: &ElementTableDesc, table: TableOf<'_>) -> Option<f64> {
    let found = table(&e.table)?;
    Some(present(&found, e.source()?)?.min(present(&found, e.destination()?)?))
}

/// The degrees at `end` of an edge table, its statistics and columns
/// `found`, the vertex table there of `vertex_rows` rows if known.
fn degree(found: &(Arc<dyn Statistics>, Vec<String>), end: &EndpointDesc, vertex_rows: Option<f64>) -> Option<Degree> {
    let (stats, names) = found;
    let columns = places(end, names)?;
    let edges = present(found, end)?;
    // The column of the most distinct values stands for the key.
    let widest = columns.iter().copied().filter_map(|c| Some((c, stats.distinct(c)?))).max_by(|a, b| a.1.total_cmp(&b.1));
    let (column, distinct) = widest?;
    let mut frequencies = stats.frequencies(column).unwrap_or_default();
    let least_most = columns.iter().filter_map(|&c| stats.frequencies(c)).map(|f| f.most).fold(f64::INFINITY, f64::min);
    if least_most.is_finite() {
        frequencies.most = frequencies.most.min(least_most);
    }
    let average = if distinct > 0.0_f64 { edges / distinct } else { 0.0_f64 };
    let fan_out = vertex_rows.map(|rows| if rows > 0.0_f64 { edges / rows } else { 0.0_f64 });
    Some(Degree { average, fan_out, frequencies })
}

// Unit tests: degrees of one column and of several, transitions summed
// over edge tables, and what is not known (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
