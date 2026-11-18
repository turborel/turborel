//! \[EXTENSION\] Changes to a built graph, beyond the paper, whose Vamana
//! builds once [DiskANN, NeurIPS 2019 Algorithm 3]: FreshDiskANN's insert
//! (search from the medoid with L, RobustPrune of the visited set, back
//! edges re-pruned past R) and delete by tombstone (Singh et al.,
//! FreshDiskANN, 2021, §4). A tombstoned node is still traversed, never
//! returned, until consolidation (`consolidate.rs`).
//!
//! For failure atomicity (D19) a new node is written unfinished, neither
//! live nor tombstoned, then its back edges, then its row map entry, and
//! last the write that makes it live; a failure before that last write
//! leaves a node no search returns and no `rows` finds, which
//! consolidation frees. Every other change is one record's write. So the
//! undo of an insert (a remove) and of a remove (an insert, which brings a
//! tombstone back to life) need nothing more.
//!
//! A node's edges are changed optimistically. The new list
//! is computed with no lock held, as it calls the metric (R-ERR-4); then,
//! under the node's latch, it is written only if the edges are still the
//! ones it was computed from, else computed again.

use std::sync::atomic::Ordering;

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

use super::meta::NO_NODE;
use super::node::{FREEING, LIVE, Node, TOMBSTONE};
use super::prune::{Bound, Candidate, Vectors, robust_prune};
use super::rowmap::Found;
use super::{DiskAnn, SearchStats, finite};

/// Tries of a change another writer keeps getting in before.
pub(super) const TRIES: u32 = 64;
/// \[DESIGN\] The slack of an edge list, as a ratio. A list a back edge
/// takes past R is pruned to R × 10 / 13, not to R, so the next back
/// edges are appended with no prune, as if a list could grow to 1.3 R
/// before it is pruned. With α = 1.2 RobustPrune of R + 1 candidates
/// often keeps nearly R, so pruning to R prunes again a few back edges
/// later, about five prunes of R + 1 nodes, each read, for every insert.
const SLACK: (usize, usize) = (10, 13);

/// What an attempt at counting an existing entry found.
enum Counted {
    Done,
    Changed,
}

impl DiskAnn {
    /// Adds one reference to (`vector`, `row`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for other dimensions, a count past 2^32 - 1, a metric
    /// that broke its contract, or an entry other writers kept changing;
    /// the buffer pool's. A failed call adds no reference.
    pub fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        self.check_dims(vector)?;
        let mut stored = Vec::new();
        self.inner.shape.vector_bytes(vector, &mut stored);
        for _ in 0..TRIES {
            let done = match self.find(&stored, row)? {
                Found::Node(id) => matches!(self.count_up(id, &stored)?, Counted::Done),
                Found::Absent(key) => self.insert_new(vector, row, key)?,
            };
            if done {
                return Ok(());
            }
            std::thread::yield_now();
        }
        Err(contended())
    }

    /// One more reference to node `id`, if it still holds `stored`: a
    /// tombstone or unfinished node comes back to life.
    fn count_up(&self, id: u32, stored: &[u8]) -> Result<Counted> {
        let _latch = self.inner.latches.lock(PageId(u64::from(id)))?;
        let shape = self.inner.shape;
        let mut was = 0_u8;
        let mut bytes = Vec::new();
        let changed = self.modify(id, |n| {
            shape.vector_bytes(&n.vector, &mut bytes);
            if n.free() || n.flags & FREEING != 0 || bytes != stored {
                return Ok(false);
            }
            was = n.flags;
            n.count = if n.live() {
                n.count.checked_add(1).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a DiskANN entry has too many references"))?
            } else {
                1
            };
            n.flags = LIVE;
            Ok(true)
        })?;
        if !changed {
            return Ok(Counted::Changed);
        }
        if was & TOMBSTONE != 0 {
            self.kill(-1);
            self.note()?;
        } else if was & LIVE == 0 {
            self.inner.live.fetch_add(1, Ordering::AcqRel);
            self.note()?;
        }
        Ok(Counted::Done)
    }

    /// A new node of (`vector`, `row`); false if another writer mapped
    /// the same entry first, to be counted instead.
    fn insert_new(&self, vector: &[f32], row: u64, key: [u8; 16]) -> Result<bool> {
        let id = self.allocate()?;
        let built = self.build_node(id, vector, row, key);
        lock_builtin(&self.inner.pending).remove(&id);
        built
    }

    fn build_node(&self, id: u32, vector: &[f32], row: u64, key: [u8; 16]) -> Result<bool> {
        let first = self.inner.medoid.compare_exchange(NO_NODE, id, Ordering::AcqRel, Ordering::Acquire).is_ok();
        let kept = if first { Vec::new() } else { self.candidates_for(id, vector)? };
        let node = Node { flags: 0, row, count: 1, vector: vector.to_vec(), neighbours: kept.clone() };
        if let Err(error) = self.put(id, &node) {
            // The node was never written, so its slot goes back to the free
            // list. Should that fail too, it is leaked; the first error is
            // the one told.
            let _ = self.release(id);
            return Err(error);
        }
        self.set_code(id, vector)?;
        for &j in &kept {
            self.link(j, id, vector)?;
        }
        if !self.map(&key, id)? {
            return Ok(false);
        }
        self.modify(id, |n| {
            n.flags = LIVE;
            Ok(true)
        })?;
        self.inner.live.fetch_add(1, Ordering::AcqRel);
        self.note()?;
        Ok(true)
    }

    /// RobustPrune of the nodes a search for `vector` visits.
    fn candidates_for(&self, id: u32, vector: &[f32]) -> Result<Vec<u32>> {
        let tuning = self.tuning()?;
        let list = usize::try_from(tuning.build_list).unwrap_or(usize::MAX);
        let width = usize::try_from(tuning.beam_width).unwrap_or(1).max(1);
        let expanded = self.walk(vector, (list, width), &mut SearchStats::default())?;
        if expanded.is_empty() {
            // The medoid was freed, or its first insert failed, so this
            // node starts the graph anew, as the first insert does. Should
            // another insert have taken it over meanwhile, its node will do.
            let old = self.inner.medoid.load(Ordering::Acquire);
            let _ = self.inner.medoid.compare_exchange(old, id, Ordering::AcqRel, Ordering::Acquire);
            return Ok(Vec::new());
        }
        let mut data = Vec::with_capacity(expanded.len().saturating_mul(self.inner.shape.dims));
        let mut candidates = Vec::with_capacity(expanded.len());
        for (row, e) in expanded.iter().enumerate() {
            data.extend_from_slice(&e.node.vector);
            candidates.push(Candidate { distance: e.distance, id: e.id, row });
        }
        let vectors = Vectors { data: &data, dims: self.inner.shape.dims };
        let bound = Bound { alpha: tuning.alpha, degree: self.inner.shape.degree };
        robust_prune(id, &mut candidates, vectors, bound, &*self.inner.plugins.metric)
    }

    /// Adds the back edge (j, x), pruning N_out(j) ∪ {x} if past R
    /// [DiskANN, NeurIPS 2019 Algorithm 3].
    pub(super) fn link(&self, j: u32, x: u32, x_vector: &[f32]) -> Result<()> {
        let mut seen = Node::default();
        for _ in 0..TRIES {
            self.read_node(j, &mut seen)?;
            if seen.free() || seen.neighbours.contains(&x) {
                return Ok(());
            }
            let mut extra = seen.neighbours.clone();
            extra.push(x);
            let degree = self.inner.shape.degree;
            let list = if extra.len() <= degree {
                extra
            } else {
                let slack = degree.saturating_mul(SLACK.0).checked_div(SLACK.1).unwrap_or(degree).max(1);
                self.reprune_within(&seen.vector, j, &extra, (Some((x, x_vector)), slack))?
            };
            if self.replace_edges(j, &seen.neighbours, &list)? {
                return Ok(());
            }
            std::thread::yield_now();
        }
        Err(contended())
    }

    /// Writes `list` as node `j`'s edges if they are still `was`.
    pub(super) fn replace_edges(&self, j: u32, was: &[u32], list: &[u32]) -> Result<bool> {
        let _latch = self.inner.latches.lock(PageId(u64::from(j)))?;
        self.modify(j, |n| {
            if n.free() || n.neighbours != was {
                return Ok(false);
            }
            n.neighbours = list.to_vec();
            Ok(true)
        })
    }

    /// RobustPrune(p, `ids`, α, R) of node `p` at `from`, reading each
    /// candidate's vector, but `known`'s, from its record.
    pub(super) fn reprune(&self, from: &[f32], p: u32, ids: &[u32], known: Option<(u32, &[f32])>) -> Result<Vec<u32>> {
        self.reprune_within(from, p, ids, (known, self.inner.shape.degree))
    }

    /// As `reprune`, keeping at most `degree` edges.
    fn reprune_within(&self, from: &[f32], p: u32, ids: &[u32], (known, degree): (Option<(u32, &[f32])>, usize)) -> Result<Vec<u32>> {
        let dims = self.inner.shape.dims;
        let mut data = Vec::with_capacity(ids.len().saturating_mul(dims));
        let mut candidates = Vec::with_capacity(ids.len());
        let mut node = Node::default();
        for &id in ids {
            if let Some((_, v)) = known.filter(|k| k.0 == id) {
                data.extend_from_slice(v);
            } else {
                self.read_node(id, &mut node)?;
                if node.free() {
                    continue;
                }
                data.extend_from_slice(&node.vector);
            }
            let row = candidates.len();
            let start = row.saturating_mul(dims);
            let v = data.get(start..start.saturating_add(dims)).unwrap_or_default();
            candidates.push(Candidate { distance: finite(self.inner.plugins.metric.distance(from, v), "metric")?, id, row });
        }
        let alpha = self.tuning()?.alpha;
        let bound = Bound { alpha, degree };
        robust_prune(p, &mut candidates, Vectors { data: &data, dims }, bound, &*self.inner.plugins.metric)
    }

    /// Takes back one reference to (`vector`, `row`); none held, nothing.
    /// The last makes its node a tombstone.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for other dimensions; the buffer pool's. A failed call
    /// takes back nothing.
    pub fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        self.check_dims(vector)?;
        let mut stored = Vec::new();
        self.inner.shape.vector_bytes(vector, &mut stored);
        let Found::Node(id) = self.find(&stored, row)? else { return Ok(()) };
        let _latch = self.inner.latches.lock(PageId(u64::from(id)))?;
        let mut buried = false;
        self.modify(id, |n| {
            if !n.live() || n.row != row {
                return Ok(false);
            }
            n.count = n.count.saturating_sub(1);
            if n.count == 0 {
                n.flags = TOMBSTONE;
                buried = true;
            }
            Ok(true)
        })?;
        if buried {
            self.kill(1);
            self.note()?;
        }
        Ok(())
    }

    /// The rows with a live entry of `vector`, each once.
    ///
    /// # Errors
    ///
    /// As `remove`.
    pub fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        self.check_dims(vector)?;
        let mut stored = Vec::new();
        self.inner.shape.vector_bytes(vector, &mut stored);
        let mut node = Node::default();
        let mut rows = Vec::new();
        for (row, id) in self.entries_of(&stored)? {
            self.read_node(id, &mut node)?;
            if node.live() && node.row == row && !rows.contains(&row) {
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

fn contended() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a DiskANN entry kept changing under other writers")
}
