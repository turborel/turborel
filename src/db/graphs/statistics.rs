//! \[DESIGN\] A property graph's statistics as the planner and the
//! executor ask for them (RULES.md D117, `stats/graph.rs`): made of its
//! element tables' statistics, each asked for as `State::statistics` asks,
//! so a table's not gathered yet are made due and none wait for them.

use std::sync::Arc;

use crate::catalog::{EndpointDesc, QualifiedName};
use crate::db::State;
use crate::stats::Statistics;
use crate::stats::graph::{Degree, Direction, GraphStatistics};

impl State {
    /// \[DESIGN\] The statistics of property graph `graph`, if it exists:
    /// of each element table those known, none of one whose table's
    /// statistics are not gathered (`stats/graph.rs`).
    pub fn graph_statistics(&self, graph: &QualifiedName) -> Option<GraphStatistics> {
        let desc = self.catalog.graph(graph)?;
        Some(GraphStatistics::of(desc, &|t| self.table_statistics(t)))
    }

    /// The statistics of `table`, and the names of its columns, if known.
    fn table_statistics(&self, table: &QualifiedName) -> Option<(Arc<dyn Statistics>, Vec<String>)> {
        let names = self.catalog.table(table)?.columns.iter().map(|c| c.name.clone()).collect();
        Some((self.statistics(table)?, names))
    }

    /// \[DESIGN\] Of table `table`, if `columns`, by their places, are the
    /// endpoint columns of an edge table of a graph over it, the degrees
    /// at that end, the first graph's that knows them: the rows an
    /// equality on those columns finds, of each value, on average
    /// (`physical/indexed/fan_out.rs`).
    pub fn edge_degree(&self, table: &QualifiedName, columns: &[usize]) -> Option<Degree> {
        let desc = self.catalog.table(table)?;
        let named: Vec<&str> = columns.iter().map(|&c| desc.columns.get(c).map(|d| d.name.as_str())).collect::<Option<_>>()?;
        let is = |end: Option<&EndpointDesc>| {
            end.is_some_and(|e| e.columns.len() == named.len() && named.iter().all(|n| e.columns.iter().any(|c| c == n)))
        };
        for graph in self.catalog.graphs() {
            for e in graph.of_table(table) {
                let direction = if is(e.source()) {
                    Direction::Out
                } else if is(e.destination()) {
                    Direction::In
                } else {
                    continue;
                };
                let stats = GraphStatistics::of(graph, &|t| if t == table { self.table_statistics(t) } else { None });
                if let Some(degree) = stats.degree(&e.alias, direction) {
                    return Some(degree);
                }
            }
        }
        None
    }
}
