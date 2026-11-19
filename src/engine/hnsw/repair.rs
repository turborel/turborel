//! \[DESIGN\] Deletion repair, beyond Malkov and Yashunin, who define none:
//! a tombstone stays a waypoint until maintenance, which gives each live
//! node that links to one new links, chosen by the heuristic of Algorithm
//! 4 from its live neighbours and the tombstones' live neighbours, so the
//! paths through the tombstone survive it; then the graph is made again
//! of its live nodes alone, renumbered.

use std::collections::VecDeque;
use std::sync::Arc;

use super::graph::{Graph, NONE, Store, slot};
use std::mem::take;

use super::insert::{Build, measured, select};
use super::measure::Metric;
use super::scratch::Scratch;
use super::view::{Owned, Source};

impl Graph {
    /// Gives every live node linking to a tombstone new links.
    pub fn repair(&mut self, metric: &Metric, build: &Build, s: &mut Scratch) {
        let n = u32::try_from(self.len()).unwrap_or(NONE);
        for p in 0..n {
            if !self.is_live(p) {
                continue;
            }
            let top = self.levels.get(slot(p)).copied().unwrap_or(0);
            for level in 0..=top {
                if self.links(p, level).iter().any(|e| !self.is_live(*e)) {
                    self.relink(metric, (p, level), build, s);
                }
            }
        }
    }

    /// Node `p`'s links on `level` chosen again from its live neighbours
    /// and its tombstoned neighbours' live neighbours.
    fn relink(&mut self, metric: &Metric, (p, level): (u32, u8), build: &Build, s: &mut Scratch) {
        s.visited.begin(self.len());
        s.visited.first(p);
        s.ids.clear();
        for &e in self.links(p, level) {
            let through = if self.is_live(e) { None } else { Some(self.links(e, level)) };
            for &c in std::iter::once(&e).chain(through.unwrap_or_default()) {
                if self.is_live(c) && s.visited.first(c) {
                    s.ids.push(c);
                }
            }
        }
        let mut base = take(&mut s.base);
        let mut src = Owned::new(self, take(&mut s.owned));
        // A direct read never halts: it takes no lock and sees no epoch.
        let _ = src.node(p, &mut base).and_then(|_| src.vectors(&s.ids, &mut s.vecs));
        measured(metric, &base, self.dims, s);
        let _ = select(&mut src, metric, &base, (level, build, true), s);
        s.owned = src.done();
        s.base = base;
        s.spare.clear();
        s.spare.extend(s.chosen.iter().map(|c| c.id));
        let mut ids = take(&mut s.spare);
        // A live node let go of keeps its last incoming edge (D97); one
        // with no place left is linked again by `reattach`.
        let before = self.links(p, level).to_vec();
        let (changed, _) = self.shelter((p, level, None), &before, true, &mut ids);
        self.set_links_kept(p, level, &ids, if changed { None } else { s.known });
        s.spare = ids;
    }

    /// The graph of its live nodes alone, renumbered in order, links to
    /// tombstones dropped; its entry the old one if live, else a live node
    /// of the highest layer.
    pub fn compacted(&self) -> Graph {
        let store = match &self.store {
            Store::Full(_) => Store::Full(Vec::with_capacity(self.live.saturating_mul(self.dims))),
            Store::Coded { codec, .. } => Store::Coded { codec: Arc::clone(codec), codes: Vec::new() },
        };
        let mut out = Graph::new(self.dims, (self.m, self.m0), store);
        let mut renumbered = vec![NONE; self.len()];
        let mut buf = vec![0.0_f32; self.dims];
        for (old, new) in (0..u32::try_from(self.len()).unwrap_or(NONE)).filter(|id| self.is_live(*id)).zip(0..NONE) {
            let at = slot(old);
            let row = (
                self.rows.get(at).copied().unwrap_or(0),
                self.hashes.get(at).copied().unwrap_or(0),
                self.counts.get(at).copied().unwrap_or(0),
            );
            let level = self.levels.get(at).copied().unwrap_or(0);
            match self.code(old) {
                Some(code) => out.push_code(row, level, code),
                None => out.push(row, level, self.vector(old, &mut buf)),
            };
            if let Some(mark) = renumbered.get_mut(at) {
                *mark = new;
            }
        }
        let mut ids = Vec::new();
        for (old, new) in renumbered.iter().enumerate().filter(|(_, new)| **new != NONE) {
            let old = u32::try_from(old).unwrap_or(NONE);
            for level in 0..=self.levels.get(slot(old)).copied().unwrap_or(0) {
                ids.clear();
                ids.extend(self.links(old, level).iter().filter_map(|e| renumbered.get(slot(*e)).copied().filter(|n| *n != NONE)));
                out.set_links(*new, level, &ids);
            }
        }
        out.entry = renumbered.get(slot(self.entry)).copied().filter(|n| *n != NONE).or_else(|| out.highest()).unwrap_or(NONE);
        out.top = out.levels.get(slot(out.entry)).copied().unwrap_or(0);
        out.reattach();
        out
    }

    /// \[DESIGN\] Links each node no path from the entry reaches on layer
    /// 0 from one that is reached (RULES.md D97): one of its own
    /// neighbours, else the entry, in place of the last of that list
    /// another list also holds, if full. A tombstone's repair may leave
    /// such a node, as its lists drop the tombstones' edges, and two nodes
    /// let go of by others' lists may hold only each other. Round after
    /// round, as a node linked again leads to others.
    fn reattach(&mut self) {
        let mut seen = Vec::new();
        let mut queue = VecDeque::new();
        for _ in 0..REATTACH_ROUNDS {
            self.reached(&mut seen, &mut queue);
            let mut moved = false;
            for u in (0..u32::try_from(self.len()).unwrap_or(NONE)).filter(|u| !seen.get(slot(*u)).copied().unwrap_or(true)) {
                moved |= self.adopt(u, &seen);
            }
            if !moved {
                return;
            }
        }
    }

    /// Marks in `seen` each node a path from the entry reaches on layer 0.
    fn reached(&self, seen: &mut Vec<bool>, queue: &mut VecDeque<u32>) {
        seen.clear();
        seen.resize(self.len(), false);
        queue.clear();
        queue.push_back(self.entry);
        while let Some(id) = queue.pop_front() {
            let Some(at) = seen.get_mut(slot(id)).filter(|s| !**s) else { continue };
            *at = true;
            queue.extend(self.links(id, 0).iter().copied());
        }
    }

    /// Links node `u` from a node `seen` marks: whether one had a place.
    fn adopt(&mut self, u: u32, seen: &[bool]) -> bool {
        let parents: Vec<u32> = self.links(u, 0).iter().copied().chain(std::iter::once(self.entry)).collect();
        for y in parents.into_iter().filter(|y| seen.get(slot(*y)).copied().unwrap_or(false)) {
            let mut ids = self.links(y, 0).to_vec();
            if ids.len() >= self.m0 {
                let Some(at) = ids.iter().rposition(|v| self.incoming(*v) > 1) else { continue };
                ids.remove(at);
            }
            ids.push(u);
            self.set_links(y, 0, &ids);
            return true;
        }
        false
    }

    /// A node of the highest layer, the first such.
    fn highest(&self) -> Option<u32> {
        let (at, _) = self.levels.iter().enumerate().max_by_key(|(at, level)| (**level, std::cmp::Reverse(*at)))?;
        u32::try_from(at).ok()
    }
}

/// \[DESIGN\] The most rounds `Graph::reattach` links nodes again in:
/// a node cut off is linked in the round after the one that reaches a
/// neighbour of it, and chains of more than a few are not met, so 16
/// bounds the work; a node left over is linked at the next maintenance.
const REATTACH_ROUNDS: usize = 16;
