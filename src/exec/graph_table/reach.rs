//! \[EXTENSION\] A path pattern's starts pruned by a reachability index
//! (RULES.md D150), where the bound form says one may be read
//! (`GraphMatch::pruning`): the ends are the vertices of the edge
//! table's vertex table its last node pattern's WHERE keeps, each tested
//! alone before the search; a start of that table is kept if the index
//! says it reaches one of them, or is one, and a start of another table
//! is kept. Every
//! match of such a pattern is a walk of the index's edges from its start
//! to its end, so a start dropped has none, and the search finds the
//! same matches of the starts kept.
//!
//! A WHERE that fails on a vertex keeps it as an end, so its error is
//! the search's to raise, if the search reaches it. An index that cannot
//! answer, its labels dropped after a delete until its maintenance,
//! prunes nothing.

use std::sync::Arc;

use super::shared::Shared;
use crate::db::State;
use crate::error::Result;
use crate::graph::indexes::Reachability;
use crate::graph::program::Bound;
use crate::graph::view::ElementId;
use crate::index::Key;
use crate::logical::PathShape;
use crate::storage::Row;
use crate::value::Value;

/// The reachability index `name`, if it is one.
fn reachability_of(db: &State, name: &str) -> Option<Arc<dyn Reachability>> {
    Arc::clone(db.index_of(name).ok()?).reachability()
}

/// The key of `row` of `columns`.
fn key_of(row: &Row, columns: &[usize]) -> Key {
    Key(columns.iter().map(|&c| row.get(c).cloned().unwrap_or(Value::Null)).collect())
}

/// `starts` of path pattern `path` of `shared`, those that cannot match
/// dropped if a reachability index says so.
///
/// # Errors
///
/// Any error reading a vertex or the index.
pub(super) fn pruned(shared: &Shared, path: &PathShape, starts: &[ElementId]) -> Result<Vec<ElementId>> {
    let Some(p) = shared.graph.pruning(path) else { return Ok(starts.to_vec()) };
    let ends_of = shared.graph.tables.get(p.edges).and_then(|t| t.ends.as_ref());
    let (Some(reach), Some(ends_of)) = (reachability_of(&shared.db, &p.index), ends_of) else { return Ok(starts.to_vec()) };
    let columns = &ends_of.source.references;
    let table = u32::try_from(p.vertices).unwrap_or(u32::MAX);
    let mut ends = Vec::new();
    for (row_id, row) in shared.store.rows(p.vertices)? {
        let bound = Bound::alone(p.end.var, ElementId::new(table, row_id));
        // An error is the search's to raise, if it reaches the vertex.
        if shared.prefilter(p.filter, &bound).unwrap_or(true) {
            ends.push(key_of(&row, columns));
        }
    }
    let mut keys = Vec::with_capacity(starts.len());
    for &start in starts.iter().filter(|s| s.table == table) {
        // A row gone has a key of no values, which the index keeps.
        keys.push(shared.store.row(start)?.map_or_else(|| Key(Vec::new()), |row| key_of(&row, columns)));
    }
    let Some(kept) = reach.reaching(&keys, &ends, p.reverse)? else { return Ok(starts.to_vec()) };
    // A start of another table matches by no edge, so it is kept.
    let mut verdicts = kept.into_iter();
    Ok(starts.iter().copied().filter(|s| s.table != table || verdicts.next().unwrap_or(true)).collect())
}
