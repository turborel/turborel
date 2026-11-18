//! \[EXTENSION\] [GS §20] How a GRAPH_TABLE's path search runs: each
//! limit its OPTIONS writes, else its setting's, 0 for none; ON LIMIT and
//! DETERMINISTIC_PATHS likewise; the workers EXECUTOR.GRAPH_THREADS lets
//! it use, and the work GRAPH.PARALLEL_MIN_STATES asks before it does; the
//! algorithm a PATH_ALGO hint names [GS §16 R16.8]; and what cancels its
//! statement.

use std::sync::Arc;

use crate::ast::PathAlgorithm;
use crate::db::State;
use crate::graph::limits::{Limits, OnLimit};
use crate::graph::search::SearchOptions;
use crate::logical::GraphOptions;
use crate::settings::{
    GRAPH_DETERMINISTIC_PATHS, GRAPH_MAX_PATH_LENGTH, GRAPH_MAX_PATHS_PER_PARTITION, GRAPH_MAX_SEARCH_STATES, GRAPH_ON_LIMIT,
    GRAPH_PARALLEL_MIN_STATES, GRAPH_THREADS, ON, TRUNCATE,
};
use crate::txn::TxId;

/// A limit: `written`, else the setting's `set`, none if 0.
fn limit(written: Option<u64>, set: usize) -> Option<usize> {
    let n = written.map_or(set, |w| usize::try_from(w).unwrap_or(usize::MAX));
    (n > 0).then_some(n)
}

/// The search options of a GRAPH_TABLE of `options` and PATH_ALGO hint
/// `algorithm`, of transaction `tx`.
pub(super) fn options(db: &Arc<State>, tx: TxId, (options, algorithm): (&GraphOptions, Option<PathAlgorithm>)) -> SearchOptions {
    let s = &db.settings;
    let on_limit = options.on_limit.unwrap_or(if s.word(GRAPH_ON_LIMIT) == TRUNCATE { OnLimit::Truncate } else { OnLimit::Error });
    let limits = Limits {
        max_path_length: limit(options.max_path_length, s.count(GRAPH_MAX_PATH_LENGTH)),
        max_paths_per_partition: limit(options.max_paths_per_partition, s.count(GRAPH_MAX_PATHS_PER_PARTITION)),
        max_search_states: limit(options.max_search_states, s.count(GRAPH_MAX_SEARCH_STATES)),
        on_limit,
    };
    SearchOptions {
        limits,
        deterministic: options.deterministic || s.word(GRAPH_DETERMINISTIC_PATHS) == ON,
        threads: s.count(GRAPH_THREADS),
        stop: db.tx.stop_of(tx),
        area: crate::graph::limits::Area::default(),
        algorithm,
        parallel_min_states: s.count(GRAPH_PARALLEL_MIN_STATES),
        step_edges: None,
    }
}
