//! \[DESIGN\] A search's state: the candidate list L, kept sorted and
//! bounded [DiskANN, NeurIPS 2019 Algorithm 1, "update L to retain closest
//! L points"], and a visited array stamped by generation, so one array
//! serves query after query with no clearing. A node is visited when its
//! stamp is the current generation's.

use crate::error::Result;
use crate::memory::{MemoryBudget, Reservation};
use std::sync::Arc;

/// What the visited arrays are reserved as.
const RESERVED_AS: &str = "the visited array of a DiskANN search";

/// A visited array, reused by searches one at a time.
pub(super) struct Visited {
    stamps: Vec<u32>,
    generation: u32,
    reservation: Option<Reservation>,
}

impl Visited {
    /// An empty array.
    pub(super) fn new() -> Visited {
        Visited { stamps: Vec::new(), generation: 0, reservation: None }
    }

    /// Starts a search of nodes below `nodes`, reserving what the array
    /// grows by of `budget`.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget refuses.
    pub(super) fn start(&mut self, nodes: usize, budget: Option<&Arc<MemoryBudget>>) -> Result<()> {
        if self.stamps.len() < nodes {
            let bytes = nodes.saturating_mul(size_of::<u32>());
            if let Some(budget) = budget {
                match self.reservation.as_mut() {
                    Some(held) => held.resize(bytes)?,
                    None => self.reservation = Some(budget.reserve(bytes, RESERVED_AS)?),
                }
            }
            self.stamps.resize(nodes, 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamps.fill(0);
            self.generation = 1;
        }
        Ok(())
    }

    /// Marks `id` visited; whether it was not before. Nodes past the array
    /// (made since the search started) count as visited, so not entered.
    pub(super) fn visit(&mut self, id: u32) -> bool {
        let generation = self.generation;
        let Some(stamp) = usize::try_from(id).ok().and_then(|at| self.stamps.get_mut(at)) else { return false };
        let fresh = *stamp != generation;
        *stamp = generation;
        fresh
    }
}

/// One candidate: its guiding distance, its node, whether expanded.
#[derive(Debug, Clone, Copy)]
pub(super) struct Entry {
    pub(super) distance: f32,
    pub(super) id: u32,
    pub(super) expanded: bool,
}

/// The candidate list, nearest first, at most `limit` long.
pub(super) struct Frontier {
    pub(super) items: Vec<Entry>,
    limit: usize,
    /// No unexpanded entry is before it.
    first_open: usize,
}

impl Frontier {
    pub(super) fn new(limit: usize) -> Frontier {
        Frontier { items: Vec::with_capacity(limit.saturating_add(1)), limit: limit.max(1), first_open: 0 }
    }

    /// Adds `id` at `distance` if it is among the nearest `limit`.
    pub(super) fn add(&mut self, distance: f32, id: u32) {
        if self.items.len() >= self.limit && self.items.last().is_some_and(|e| e.distance <= distance) {
            return;
        }
        let at = self.items.partition_point(|e| (e.distance, e.id) < (distance, id));
        self.items.insert(at, Entry { distance, id, expanded: false });
        self.items.truncate(self.limit);
        self.first_open = self.first_open.min(at);
    }

    /// Up to `width` nearest unexpanded candidates, now marked expanded
    /// [DiskANN, NeurIPS 2019 §3.3: "the W closest points in L \ V"].
    pub(super) fn take(&mut self, width: usize, out: &mut Vec<Entry>) {
        out.clear();
        let mut at = self.first_open;
        while out.len() < width {
            let Some(entry) = self.items.get_mut(at) else { break };
            if !entry.expanded {
                entry.expanded = true;
                out.push(*entry);
            }
            at = at.saturating_add(1);
        }
        while self.items.get(self.first_open).is_some_and(|e| e.expanded) {
            self.first_open = self.first_open.saturating_add(1);
        }
    }

    /// Empties it for another search of list size `limit`.
    pub(super) fn reset(&mut self, limit: usize) {
        self.items.clear();
        self.limit = limit.max(1);
        self.first_open = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [DiskANN, NeurIPS 2019 Algorithm 1] The list
    /// keeps the nearest L, takes the W nearest unexpanded in order, and
    /// a visited array forgets the last search's marks without clearing.
    #[test]
    fn the_list_keeps_the_nearest_and_visits_reset_by_generation() {
        let mut f = Frontier::new(3);
        for (d, id) in [(5.0_f32, 5), (1.0_f32, 1), (3.0_f32, 3), (4.0_f32, 4), (0.5_f32, 9)] {
            f.add(d, id);
        }
        assert_eq!(f.items.iter().map(|e| e.id).collect::<Vec<_>>(), [9, 1, 3]);
        let mut out = Vec::new();
        f.take(2, &mut out);
        assert_eq!(out.iter().map(|e| e.id).collect::<Vec<_>>(), [9, 1]);
        f.add(2.0_f32, 2);
        f.take(2, &mut out);
        assert_eq!(out.iter().map(|e| e.id).collect::<Vec<_>>(), [2]);
        let mut v = Visited::new();
        v.start(4, None).unwrap();
        assert!(v.visit(2) && !v.visit(2) && !v.visit(9));
        v.start(4, None).unwrap();
        assert!(v.visit(2));
    }
}
