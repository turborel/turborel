//! \[EXTENSION\] Maintenance, beyond the paper: FreshDiskANN's delete
//! consolidation (Singh et al., 2021, Algorithm 4), and training the
//! codes of an index made by inserts.
//!
//! Consolidation, once dead nodes pass `Tuning::consolidate_percent` of
//! the nodes: each dead node D (tombstoned, or unfinished by a failed
//! insert) is marked as being freed and taken out of the row map, so no
//! insert brings it back; then each live p with an edge into D gets
//! RobustPrune(p, (N_out(p) \ D) ∪ ⋃_{d ∈ N_out(p) ∩ D} N_out(d) \ D, α, R);
//! then D's slots go to the free list. A medoid in D is replaced first by
//! the live node nearest it, found by a search.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::{lock_builtin, read};

use super::fresh::TRIES;
use super::meta::NO_NODE;
use super::node::{FREEING, LIVE, Node, TOMBSTONE};
use super::store::dec;
use super::{DiskAnn, SearchStats};

/// A percentage's whole.
const PERCENT: u64 = 100;
/// The most nodes read to train codes on.
pub(super) const SAMPLE: usize = 65_536;
/// The list of the search that finds a new medoid.
const MEDOID_LIST: usize = 32;

impl DiskAnn {
    /// Whether maintenance has work: consolidation, or codes to train.
    pub fn maintenance_due(&self) -> bool {
        self.consolidation_due() || self.training_due()
    }

    fn consolidation_due(&self) -> bool {
        let (live, dead) = (self.inner.live.load(Ordering::Acquire), self.inner.dead.load(Ordering::Acquire));
        let percent = self.tuning().map_or(0, |t| u64::from(t.consolidate_percent));
        dead > 0 && dead.saturating_mul(PERCENT) >= live.saturating_add(dead).saturating_mul(percent)
    }

    fn training_due(&self) -> bool {
        let trained = read(&self.inner.book).map_or(true, |b| b.codes.is_some());
        let at = self.tuning().map_or(u64::MAX, |t| u64::from(t.train_at));
        self.inner.plugins.quantizer.is_some() && !trained && self.inner.live.load(Ordering::Acquire) >= at
    }

    /// Does the work due; whether it did any.
    ///
    /// # Errors
    ///
    /// The quantizer's or metric's; the buffer pool's. A failed pass
    /// leaves the graph searchable and is tried again later.
    pub fn maintain(&self) -> Result<bool> {
        if self.training_due() {
            let sample = self.sample(SAMPLE)?;
            self.train(&sample)?;
            return Ok(true);
        }
        if self.consolidation_due() {
            self.consolidate()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Up to `most` live vectors, evenly spread over the nodes.
    pub(super) fn sample(&self, most: usize) -> Result<Vec<f32>> {
        let next = lock_builtin(&self.inner.alloc).next;
        let n = usize::try_from(next).unwrap_or(0);
        let step = n.checked_div(most.max(1)).unwrap_or(0).max(1);
        let mut out = Vec::new();
        let mut node = Node::default();
        for id in (0..next).step_by(step) {
            self.read_node(id, &mut node)?;
            if node.live() {
                out.extend_from_slice(&node.vector);
            }
        }
        Ok(out)
    }

    /// Consolidates every dead node now.
    ///
    /// # Errors
    ///
    /// As `maintain`.
    pub fn consolidate(&self) -> Result<()> {
        let dead = self.mark_dead()?;
        if dead.is_empty() {
            return Ok(());
        }
        if dead.contains(&self.inner.medoid.load(Ordering::Acquire)) {
            self.replace_medoid(&dead)?;
        }
        self.repair(&dead)?;
        let mut node = Node::default();
        let mut tombstones = 0_u64;
        for &d in &dead {
            self.read_node(d, &mut node)?;
            tombstones = tombstones.saturating_add(u64::from(node.flags & TOMBSTONE != 0));
            self.release(d)?;
        }
        dec(&self.inner.dead, tombstones);
        self.note()?;
        self.refresh_cache()
    }

    /// Marks every dead node as being freed and takes it out of the row
    /// map; the set marked.
    fn mark_dead(&self) -> Result<HashSet<u32>> {
        let next = lock_builtin(&self.inner.alloc).next;
        let mut dead = HashSet::new();
        let (mut node, mut stored) = (Node::default(), Vec::new());
        for id in 0..next {
            self.read_node(id, &mut node)?;
            if node.flags & TOMBSTONE == 0 && node.flags != 0 {
                continue;
            }
            let _latch = self.inner.latches.lock(PageId(u64::from(id)))?;
            let pending = &self.inner.pending;
            let marked = self.modify(id, |n| {
                // An insert registers its node before writing it, so an
                // unfinished node not registered is one a failure left.
                let unfinished = n.flags == 0 && !lock_builtin(pending).contains(&id);
                let still = n.flags & TOMBSTONE != 0 || unfinished;
                if still {
                    n.flags |= FREEING;
                }
                Ok(still)
            })?;
            if marked {
                self.inner.shape.vector_bytes(&node.vector, &mut stored);
                if let Some(key) = self.key_of(&stored, node.row, id)? {
                    self.unmap(&key, id)?;
                }
                dead.insert(id);
            }
        }
        Ok(dead)
    }

    /// Prunes again each node with an edge into `dead`, over its other
    /// edges and the dead neighbours' edges.
    fn repair(&self, dead: &HashSet<u32>) -> Result<()> {
        let next = lock_builtin(&self.inner.alloc).next;
        let (mut node, mut gone) = (Node::default(), Node::default());
        for p in 0..next {
            self.read_node(p, &mut node)?;
            if node.free() || dead.contains(&p) || !node.neighbours.iter().any(|n| dead.contains(n)) {
                continue;
            }
            for _ in 0..TRIES {
                let mut candidates: Vec<u32> = node.neighbours.iter().copied().filter(|n| !dead.contains(n)).collect();
                for d in node.neighbours.iter().filter(|n| dead.contains(n)) {
                    self.read_node(*d, &mut gone)?;
                    candidates.extend(gone.neighbours.iter().copied().filter(|n| !dead.contains(n) && *n != p));
                }
                candidates.sort_unstable();
                candidates.dedup();
                let list = self.reprune(&node.vector, p, &candidates, None)?;
                if self.replace_edges(p, &node.neighbours, &list)? {
                    break;
                }
                self.read_node(p, &mut node)?;
            }
        }
        Ok(())
    }

    /// Makes the live node nearest the dead medoid the medoid.
    fn replace_medoid(&self, dead: &HashSet<u32>) -> Result<()> {
        let old = self.inner.medoid.load(Ordering::Acquire);
        let mut node = Node::default();
        self.read_node(old, &mut node)?;
        let width = usize::try_from(self.tuning()?.beam_width).unwrap_or(1).max(1);
        let expanded = self.walk(&node.vector, (MEDOID_LIST, width), &mut SearchStats::default())?;
        let found = expanded.iter().find(|e| e.node.flags & LIVE != 0 && !dead.contains(&e.id)).map(|e| e.id);
        let next = match found {
            Some(id) => id,
            None => self.any_live(dead)?,
        };
        self.inner.medoid.store(next, Ordering::Release);
        self.note()
    }

    /// Any live node not in `dead`, else none.
    fn any_live(&self, dead: &HashSet<u32>) -> Result<u32> {
        let next = lock_builtin(&self.inner.alloc).next;
        let mut node = Node::default();
        for id in 0..next {
            self.read_node(id, &mut node)?;
            if node.live() && !dead.contains(&id) {
                return Ok(id);
            }
        }
        Ok(NO_NODE)
    }
}
