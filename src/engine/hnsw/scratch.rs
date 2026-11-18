//! \[DESIGN\] A search's reusable state: a generation-stamped visited set,
//! the bounded heaps of candidates and results, and buffers for decoded
//! vectors. A search takes one from its index's pool and gives it back,
//! so no hop and no query after the first allocates.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::Mutex;

use super::view::Hop;
use crate::sync::lock_builtin;

/// \[DESIGN\] The scratch sets a pool keeps: one a thread searching at
/// once, near enough; past them, one made for a search is dropped after.
const POOLED: usize = 64;

/// A node and its distance from what is searched for. Ordered by
/// distance, then by id, so equal distances order the same on every run.
#[derive(Debug, Clone, Copy)]
pub struct Scored {
    /// The distance.
    pub distance: f32,
    /// The node, or entry, it is of.
    pub id: u32,
}

impl Ord for Scored {
    fn cmp(&self, o: &Self) -> Ordering {
        self.distance.total_cmp(&o.distance).then(self.id.cmp(&o.id))
    }
}

impl PartialOrd for Scored {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialEq for Scored {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Scored {}

/// The ids seen in this search: a mark per id, the search's generation in
/// each it saw, so starting a search clears it by counting, not writing.
#[derive(Debug, Default)]
pub struct Visited {
    marks: Vec<u32>,
    generation: u32,
}

impl Visited {
    /// Starts a search of ids below `n`.
    pub fn begin(&mut self, n: usize) {
        if self.marks.len() < n {
            self.marks.resize(n, 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // The generation wrapped: marks of 2^32 searches ago would
            // read as this one's.
            self.marks.fill(0);
            self.generation = 1;
        }
    }

    /// Marks `id`, and whether it was not marked yet in this search. An
    /// id past those begun with, as of a node appended since, grows the
    /// marks: a search reading the graph a hop at a time sees it too.
    #[inline]
    pub fn first(&mut self, id: u32) -> bool {
        let generation = self.generation;
        let at = usize::try_from(id).unwrap_or(usize::MAX);
        if at >= self.marks.len() {
            if at == usize::MAX {
                return false;
            }
            self.marks.resize(at.saturating_add(1), 0);
        }
        let Some(mark) = self.marks.get_mut(at) else { return false };
        let first = *mark != generation;
        *mark = generation;
        first
    }
}

/// A list an insertion plans: its layer, its ids, and how many of them
/// the heuristic kept, if known.
pub type Planned = (u8, Vec<u32>, Option<usize>);

/// One search's state.
#[derive(Default)]
pub struct Scratch {
    /// The ids seen.
    pub visited: Visited,
    /// The candidates to expand, nearest first.
    pub candidates: BinaryHeap<Reverse<Scored>>,
    /// The results kept, farthest first, so the farthest is let go.
    pub results: BinaryHeap<Scored>,
    /// Sorted results, or a neighbour list being chosen.
    pub chosen: Vec<Scored>,
    /// Candidates a neighbour selection set aside.
    pub pruned: Vec<Scored>,
    /// The neighbours a selection keeps.
    pub kept: Vec<Scored>,
    /// The vector neighbours are chosen for, decoded.
    pub base: Vec<f32>,
    /// The entries of the next layer's search.
    pub entries: Vec<Scored>,
    /// A decoded vector.
    pub a: Vec<f32>,
    /// Another decoded vector.
    pub b: Vec<f32>,
    /// Ids being changed, as a node's neighbours.
    pub ids: Vec<u32>,
    /// Ids of a list being written.
    pub spare: Vec<u32>,
    /// The vectors of the candidates a selection weighs, one after another.
    pub vecs: Vec<f32>,
    /// Each candidate's place in `vecs`, as `chosen` orders them.
    pub place: Vec<usize>,
    /// Candidates with their places in `vecs`, being sorted.
    pub pairs: Vec<(Scored, usize)>,
    /// The places in `vecs` of the neighbours a selection keeps.
    pub kept_at: Vec<usize>,
    /// A list as it was read, to change it only if it still is.
    pub before: Vec<u32>,
    /// The vector being inserted, as stored, decoded.
    pub query: Vec<f32>,
    /// The code of the vector being inserted, if the graph keeps codes.
    pub code: Vec<u8>,
    /// Rows of the nodes found.
    pub rows: Vec<u64>,
    /// How many of `chosen` the last selection kept, the rest set aside,
    /// if known.
    pub known: Option<usize>,
    /// An insertion's planned lists: each layer's chosen ids, top first,
    /// and how many of them the heuristic kept.
    pub plan: Vec<Planned>,
    /// Back edges that found their list full: node and layer.
    pub over: Vec<(u32, u8)>,
    /// A direct read's buffer.
    pub owned: Vec<f32>,
    /// A shared read's copies.
    pub hop: Hop,
}

impl Scratch {
    /// Readies it for vectors of `dims` and ids below `n`.
    pub fn begin(&mut self, dims: usize, n: usize) {
        self.visited.begin(n);
        self.candidates.clear();
        self.results.clear();
        self.chosen.clear();
        self.pruned.clear();
        self.kept.clear();
        self.entries.clear();
        self.a.resize(dims, 0.0);
        self.b.resize(dims, 0.0);
        self.base.resize(dims, 0.0);
    }
}

/// Scratch sets given back, for the next searches.
#[derive(Default)]
pub struct Pool {
    kept: Mutex<Vec<Scratch>>,
}

impl Pool {
    /// A scratch set, kept or new.
    pub fn take(&self) -> Scratch {
        lock_builtin(&self.kept).pop().unwrap_or_default()
    }

    /// Gives `scratch` back, unless the pool holds enough.
    pub fn give(&self, scratch: Scratch) {
        let mut kept = lock_builtin(&self.kept);
        if kept.len() < POOLED {
            kept.push(scratch);
        }
    }

    /// Lets every kept set go, as after the index shrank.
    pub fn clear(&self) {
        lock_builtin(&self.kept).clear();
    }
}

/// Keeps `scored` among the `k` nearest of `results`, a max-heap: the
/// farthest let go once there are more than `k`.
#[inline]
pub fn keep(results: &mut BinaryHeap<Scored>, scored: Scored, k: usize) {
    if results.len() < k {
        results.push(scored);
    } else if results.peek().is_some_and(|far| scored < *far) {
        results.pop();
        results.push(scored);
    }
}
