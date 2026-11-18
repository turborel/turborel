//! \[DESIGN\] What EXPLAIN of a GRAPH_TABLE shows its searches chosen by:
//! the rows of each of its stored element tables, as the storage counts
//! them, and the settings of the workers (`logical::GraphSizing`).

use super::Context;
use crate::logical::{ElementRead, ElementSource, GraphSizing};
use crate::settings::{GRAPH_PARALLEL_MIN_STATES, GRAPH_THREADS};

/// The sizing of a GRAPH_TABLE of element tables `tables`.
pub(super) fn of(cx: Context<'_>, tables: &[ElementSource]) -> GraphSizing {
    let rows = tables
        .iter()
        .map(|t| match t.read {
            ElementRead::Stored => cx.db.storage_of(&t.table).ok().and_then(|s| u64::try_from(s.row_count()).ok()),
            ElementRead::Held(_) => None,
        })
        .collect();
    GraphSizing { rows, threads: cx.db.settings.count(GRAPH_THREADS), parallel_min_states: cx.db.settings.count(GRAPH_PARALLEL_MIN_STATES) }
}
