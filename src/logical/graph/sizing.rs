//! \[DESIGN\] What EXPLAIN of a GRAPH_TABLE shows its searches chosen by
//! (`graph/search/choice.rs`): its element tables' rows as the binder
//! found them, and the settings of the workers. The executor chooses
//! again as it runs, of the rows then, so the two agree unless the tables
//! change between.

use crate::graph::search::SearchOptions;
use crate::graph::view::GraphSize;

/// \[DESIGN\] A GRAPH_TABLE's sizes and workers, as it was bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphSizing {
    /// Of each element table, its rows, if they were counted cheaply.
    pub rows: Vec<Option<u64>>,
    /// EXECUTOR.GRAPH_THREADS.
    pub threads: usize,
    /// GRAPH.PARALLEL_MIN_STATES.
    pub parallel_min_states: usize,
}

impl GraphSizing {
    /// The vertices and edges of element tables of which `edges` says
    /// which are edge tables, if every table's rows were counted.
    pub fn size(&self, edges: impl Iterator<Item = bool>) -> Option<GraphSize> {
        let mut size = GraphSize { vertices: 0, edges: 0 };
        for (rows, edge) in self.rows.iter().zip(edges) {
            let count = if edge { &mut size.edges } else { &mut size.vertices };
            *count = count.saturating_add((*rows)?);
        }
        Some(size)
    }

    /// The vertices of element tables `tables`, if their rows were counted.
    pub fn starts(&self, tables: &[usize]) -> Option<usize> {
        let mut n = 0_u64;
        for &t in tables {
            n = n.saturating_add(self.rows.get(t).copied().flatten()?);
        }
        usize::try_from(n).ok()
    }

    /// The options EXPLAIN's search is planned with.
    pub fn options(&self, algorithm: Option<crate::ast::PathAlgorithm>) -> SearchOptions {
        SearchOptions { threads: self.threads, parallel_min_states: self.parallel_min_states, algorithm, ..SearchOptions::default() }
    }
}
