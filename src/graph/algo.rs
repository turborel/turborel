//! \[EXTENSION\] The graph algorithm library of the graph standard's §18:
//! PAGERANK, WCC, SCC, BFS_LEVELS, SSSP, TRIANGLE_COUNT,
//! LOCAL_CLUSTERING, BETWEENNESS, LOUVAIN, LABEL_PROPAGATION, K_CORE and
//! JACCARD_SIMILARITY, each its own file, on a graph of dense vertex
//! numbers in compressed sparse rows (`csr.rs`).
//!
//! The engine knows no table: a caller builds the graph of its edges,
//! each a source, a destination and perhaps a weight, and gives each run
//! a `Control` (`control.rs`), the threads it may use, the bytes it may
//! hold and what tells it to stop. Every algorithm runs on any width,
//! its work cut into morsels taken by scoped threads (`parallel.rs`), and
//! gives the same result on every width: to the bit, its sums made in
//! the same order, or exactly, of counts and labels. The SQL surface,
//! `GRAPH_ALGO.<name>(...)` and INTO PROPERTY, is `library.rs`'s.

mod betweenness;
mod components;
mod control;
mod csr;
mod jaccard;
mod kcore;
mod keys;
mod labels;
mod library;
mod louvain;
mod pagerank;
mod parallel;
mod random;
mod strong;
#[cfg(test)]
mod tests;
mod traversal;
mod triangles;

pub use betweenness::{Betweenness, betweenness};
pub use components::wcc;
pub use control::{Cancel, Charge, Control, Memory, Never};
pub use csr::{Graph, GraphBuilder, Undirected, Vertex, graph};
pub use jaccard::{Jaccard, Pair, jaccard};
pub use kcore::k_core;
pub use labels::{LabelPropagation, label_propagation};
pub use library::{ALGORITHMS, Algorithm, Arguments, Input, Kind, Output, Parameter, SCHEMA, Sizes, run};
pub use louvain::{Communities, Louvain, louvain};
pub use pagerank::{PageRank, Ranked, pagerank};
pub use strong::scc;
pub use traversal::{UNREACHED, bfs_levels, sssp};
pub use triangles::{local_clustering, triangle_count};

use crate::error::{Code, SqlError};

/// \[GS §22\] GRF-0034, of a cost, SQLSTATE 22W34 as a path search's,
/// `what` saying why.
pub(crate) fn cost_refused(what: &str) -> SqlError {
    let state = crate::error::SqlState::parse(super::limits::COST_STATE).unwrap_or_else(|| crate::error::SqlState::of(Code::Data));
    SqlError::new(Code::Data, format!("[EXTENSION] GRF-0034: {what}")).with_state(state)
}

/// The error of `what`, a loop that settles within a bound its graph
/// sets, run past it: a defect, never a graph's (RULES.md R-CF-6).
pub(crate) fn unsettled(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what} ran past the rounds its graph bounds it by"))
}

/// The error of `algorithm`'s `parameter` out of its `range`.
pub(crate) fn invalid(algorithm: &str, parameter: &str, range: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {algorithm}'s {parameter} must be {range}"))
}
