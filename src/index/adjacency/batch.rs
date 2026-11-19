//! \[DESIGN\] A frontier's lists read at once (`Adjacency::steps_many`), as
//! a vectorized Expand reads them: the vertices the compacted adjacency
//! holds, and the delta has not marked, in one pass over its arrays in
//! the order of their numbers; the others of the tree, in the order of
//! their keys, so its pages are met in order. Each vertex's steps are the
//! ones `steps_of` gives it.

use std::sync::Arc;

use super::AdjacencyIndex;
use super::csr::Csr;
use crate::error::Result;
use crate::graph::adjacency::{Direction, Step};
use crate::index::Key;

impl AdjacencyIndex {
    /// `Adjacency::steps_many`: the steps of each of `vertices` in
    /// `direction`, pushed onto `out` in their order.
    ///
    /// # Errors
    ///
    /// As `steps_of`.
    pub(super) fn steps_batch(&self, vertices: &[Key], direction: Direction, out: &mut Vec<Vec<Step>>) -> Result<()> {
        let mut lists: Vec<Vec<Step>> = vec![Vec::new(); vertices.len()];
        let mut compacted: Option<Arc<Csr>> = None;
        let mut numbered: Vec<(u32, usize)> = Vec::new();
        let mut rest: Vec<(Vec<u8>, usize)> = Vec::new();
        for (place, vertex) in vertices.iter().enumerate() {
            let prefix = match self.layout.kept.keeps(direction).then(|| self.anchored(vertex, direction)).transpose()? {
                Some(Some((prefix, false))) => prefix,
                Some(Some((prefix, true))) => {
                    rest.push((prefix, place));
                    continue;
                }
                Some(None) | None => {
                    rest.push((Vec::new(), place));
                    continue;
                }
            };
            // A list the delta marks, or of a compacted adjacency built
            // since the first was taken, is read as one alone.
            let Some(csr) = self.delta.csr(direction, &prefix)? else {
                rest.push((prefix, place));
                continue;
            };
            let held = compacted.get_or_insert_with(|| Arc::clone(&csr));
            if Arc::ptr_eq(&csr, held) {
                numbered.extend(csr.number(&prefix).map(|id| (id, place)));
            } else {
                rest.push((prefix, place));
            }
        }
        if let Some(csr) = &compacted {
            csr.steps_numbered(direction, numbered, self.layout.ends(direction).1, &mut lists)?;
        }
        rest.sort_unstable();
        for (_, place) in rest {
            if let (Some(vertex), Some(list)) = (vertices.get(place), lists.get_mut(place)) {
                self.steps_of(vertex, direction, list)?;
            }
        }
        out.extend(lists);
        Ok(())
    }
}
