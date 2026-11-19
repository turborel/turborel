//! \[DESIGN\] The INMEMORY NEIGHBOR GRAPH of the vector design document
//! §10.1: HNSW's layers (Malkov and Yashunin, §4) held as flat arrays, a
//! node a dense `u32`. Layer 0's lists sit in one array of fixed slots,
//! M0 ids and a degree each, so a hop reads one contiguous run; the upper
//! layers, held by few nodes, sit in blocks of M ids a layer, found by
//! each node's first block. A node's vector is its full `f32`s, or a code
//! once the index's quantizer is trained. Beside it is the squared norm
//! of the vector a search reads, its code's decoded if it keeps a code,
//! so a metric measured by norms sums only `q·v` a hop.
//!
//! A node is an entry, a (vector, row) pair, counted: inserted again it
//! counts one more, removed one less, and at none it is a tombstone, still
//! a waypoint of searches but never a result, until maintenance unlinks it.

use std::collections::HashMap;
use std::sync::Arc;

use super::measure::Codec;
use crate::vector::kernels::squared_norm;

/// No node: an empty graph's entry, a node with no upper layers.
pub const NONE: u32 = u32::MAX;

/// A node's place in the per-node arrays.
#[inline]
pub fn slot(id: u32) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}

/// The vectors of the nodes, one after another.
#[derive(Clone)]
pub enum Store {
    /// Full precision, `dims` floats a node.
    Full(Vec<f32>),
    /// Codes of a trained codec, its `code_bytes` a node.
    Coded {
        /// The codec.
        codec: Arc<dyn Codec>,
        /// The codes.
        codes: Vec<u8>,
    },
}

/// The graph.
#[derive(Clone)]
pub struct Graph {
    /// The dimensions of each vector.
    pub dims: usize,
    /// M: the most neighbours a node keeps on an upper layer.
    pub m: usize,
    /// M0: the most it keeps on layer 0.
    pub m0: usize,
    /// The nodes' vectors.
    pub store: Store,
    /// Each node's vector's squared norm, as `vector` reads the vector.
    norms: Vec<f32>,
    /// Each node's row.
    pub rows: Vec<u64>,
    /// Each node's count of inserts not removed; 0 for a tombstone.
    pub counts: Vec<u32>,
    /// Each node's top layer.
    pub levels: Vec<u8>,
    /// Each node's vector hash (`random::vector_hash`).
    pub hashes: Vec<u64>,
    /// Layer 0's lists: per node, its degree then M0 ids.
    links0: Vec<u32>,
    /// The upper layers' lists: blocks of a degree then M ids.
    upper: Vec<u32>,
    /// Each node's first block in `upper`, or `NONE`.
    upper_at: Vec<u32>,
    /// \[DESIGN\] How many of layer 0's lists hold each node, derived
    /// from the lists and never stored, so that a back edge's reselection
    /// keeps every node's last incoming edge (RULES.md D97, `shelter`).
    incoming: Vec<u32>,
    /// The node searches start from, on the top layer; `NONE` if empty.
    pub entry: u32,
    /// The top layer.
    pub top: u8,
    /// The first node of each vector hash.
    by_hash: HashMap<u64, u32>,
    /// The next node of the same hash, or `NONE`.
    same: Vec<u32>,
    /// Nodes with a count.
    pub live: usize,
    /// Tombstones.
    pub dead: usize,
    /// \[DESIGN\] Its epoch: a graph maintenance swaps in, its nodes
    /// renumbered or its store coded, has the next, so a search or change
    /// read in an older one starts again (`view`).
    pub epoch: u64,
    /// Its changes: maintenance, working on a copy, swaps it in only if
    /// none came meanwhile.
    pub changes: u64,
}

impl Graph {
    /// An empty graph of vectors of `dims`, M `m`, M0 `m0`.
    pub fn new(dims: usize, (m, m0): (usize, usize), store: Store) -> Graph {
        Graph {
            dims,
            m,
            m0,
            store,
            norms: Vec::new(),
            rows: Vec::new(),
            counts: Vec::new(),
            levels: Vec::new(),
            hashes: Vec::new(),
            links0: Vec::new(),
            upper: Vec::new(),
            upper_at: Vec::new(),
            incoming: Vec::new(),
            entry: NONE,
            top: 0,
            by_hash: HashMap::new(),
            same: Vec::new(),
            live: 0,
            dead: 0,
            epoch: 0,
            changes: 0,
        }
    }

    /// The most neighbours a node keeps on `level`: M0 on layer 0, else M.
    pub fn most(&self, level: u8) -> usize {
        if level == 0 { self.m0 } else { self.m }
    }

    /// Its codec, if it keeps codes.
    pub fn codec(&self) -> Option<&Arc<dyn Codec>> {
        match &self.store {
            Store::Full(_) => None,
            Store::Coded { codec, .. } => Some(codec),
        }
    }

    /// Its nodes, tombstones included.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The bytes a node of top layer `level` takes, near enough, for its
    /// reservation of the memory budget.
    pub fn node_bytes(&self, level: u8) -> usize {
        let vector = match &self.store {
            Store::Full(_) => self.dims.saturating_mul(F32_BYTES),
            Store::Coded { codec, .. } => codec.code_bytes(),
        };
        let lists = self.m0.saturating_add(1).saturating_add(self.m.saturating_add(1).saturating_mul(usize::from(level)));
        vector.saturating_add(lists.saturating_mul(ID_BYTES)).saturating_add(NODE_OVERHEAD)
    }

    /// Node `id`'s vector: its own floats, or its code decoded into `buf`.
    #[inline]
    pub fn vector<'a>(&'a self, id: u32, buf: &'a mut [f32]) -> &'a [f32] {
        let at = slot(id);
        match &self.store {
            Store::Full(floats) => {
                floats.get(at.saturating_mul(self.dims)..at.saturating_add(1).saturating_mul(self.dims)).unwrap_or_default()
            }
            Store::Coded { codec, codes } => {
                let bytes = codec.code_bytes();
                if let Some(code) = codes.get(at.saturating_mul(bytes)..at.saturating_add(1).saturating_mul(bytes)) {
                    codec.decode(code, buf);
                }
                buf
            }
        }
    }

    /// Node `id`'s vector's squared norm.
    #[inline]
    pub fn norm(&self, id: u32) -> f32 {
        self.norms.get(slot(id)).copied().unwrap_or(0.0)
    }

    /// Stores its vectors as `store`, of as many nodes, as when its
    /// vectors are coded, their norms worked out again.
    pub fn recode(&mut self, store: Store) {
        self.store = store;
        self.norms.clear();
        let mut buf = self.decode_buffer();
        for id in 0..u32::try_from(self.len()).unwrap_or(NONE) {
            let norm = squared_norm(self.vector(id, &mut buf));
            self.norms.push(norm);
        }
    }

    /// The bytes of each code, if the store is coded.
    pub fn code_bytes(&self) -> Option<usize> {
        let Store::Coded { codec, .. } = &self.store else { return None };
        Some(codec.code_bytes())
    }

    /// Node `id`'s code, if the store is coded.
    pub fn code(&self, id: u32) -> Option<&[u8]> {
        let Store::Coded { codec, codes } = &self.store else { return None };
        let (at, bytes) = (slot(id), codec.code_bytes());
        codes.get(at.saturating_mul(bytes)..at.saturating_add(1).saturating_mul(bytes))
    }

    /// The span of `id`'s list on `level` in its array, degree first.
    fn span(&self, id: u32, level: u8) -> Option<(bool, usize, usize)> {
        if level == 0 {
            let width = self.m0.saturating_add(1);
            return Some((false, slot(id).saturating_mul(width), width));
        }
        if level > *self.levels.get(slot(id))? {
            return None;
        }
        let first = *self.upper_at.get(slot(id))?;
        let block = slot(first).saturating_add(usize::from(level).saturating_sub(1));
        let width = self.m.saturating_add(1);
        Some((true, block.saturating_mul(width), width))
    }

    /// Node `id`'s neighbours on `level`.
    #[inline]
    pub fn links(&self, id: u32, level: u8) -> &[u32] {
        let Some((up, start, width)) = self.span(id, level) else { return &[] };
        let array = if up { &self.upper } else { &self.links0 };
        let Some((degree, ids)) = array.get(start..start.saturating_add(width)).and_then(<[u32]>::split_first) else { return &[] };
        ids.get(..degree_of(*degree)).unwrap_or_default()
    }

    /// How many of node `id`'s first neighbours on `level` the heuristic
    /// kept, the rest being those it set aside, if known.
    pub fn kept(&self, id: u32, level: u8) -> Option<usize> {
        let (up, start, _) = self.span(id, level)?;
        let array = if up { &self.upper } else { &self.links0 };
        Some(slot((*array.get(start)? >> DEGREE_BITS).checked_sub(1)?))
    }

    /// Makes node `id`'s neighbours on `level` `ids`, at most the layer's
    /// most; ids past it are dropped.
    pub fn set_links(&mut self, id: u32, level: u8, ids: &[u32]) {
        self.set_links_kept(id, level, ids, None);
    }

    /// `set_links`, the first `kept` of `ids` those the heuristic kept,
    /// if known.
    pub fn set_links_kept(&mut self, id: u32, level: u8, ids: &[u32], kept: Option<usize>) {
        let Some((up, start, width)) = self.span(id, level) else { return };
        let Graph { upper, links0, incoming, .. } = self;
        let array = if up { upper } else { links0 };
        let Some((degree, slots)) = array.get_mut(start..start.saturating_add(width)).and_then(<[u32]>::split_first_mut) else { return };
        let n = ids.len().min(slots.len());
        if !up {
            slots.iter().take(degree_of(*degree)).for_each(|e| tally(incoming, *e, false));
            ids.iter().take(n).for_each(|e| tally(incoming, *e, true));
        }
        let known = kept.map_or(0, |k| u32::try_from(k.min(n).saturating_add(1)).unwrap_or(0));
        *degree = u32::try_from(n).unwrap_or(0) | known.checked_shl(DEGREE_BITS).unwrap_or(0);
        slots.iter_mut().zip(ids).for_each(|(s, i)| *s = *i);
    }

    /// How many of layer 0's lists hold node `id`.
    pub fn incoming(&self, id: u32) -> u32 {
        self.incoming.get(slot(id)).copied().unwrap_or(0)
    }

    /// \[DESIGN\] Keeps every node's last incoming edge on layer 0 when
    /// node `e`'s list `before` is chosen again as `ids`, nearest first,
    /// with `new` a candidate it did not hold (RULES.md D97). A candidate
    /// let go of that no other list leads to, a tombstone too unless
    /// `live_only`, stays, `new` first, in place of the farthest
    /// candidate chosen that another list leads to. An edge from one of
    /// the candidate's own neighbours leads from its own reach alone, so
    /// it does not count, else a pair could be cut off together. Whether
    /// `ids` changed, and the candidates no place was left for, which the
    /// caller links from elsewhere. Upper layers lead down to layer 0,
    /// which holds every node, so their lists are not held.
    pub fn shelter(
        &self,
        (e, level, new): (u32, u8, Option<u32>),
        before: &[u32],
        live_only: bool,
        ids: &mut Vec<u32>,
    ) -> (bool, Vec<u32>) {
        let mut homeless = Vec::new();
        if level != 0 {
            return (false, homeless);
        }
        let mut changed = false;
        for x in new.iter().chain(before).copied() {
            if ids.contains(&x) || self.spare_edges(x, e, before.contains(&x)) > 0 || (live_only && !self.is_live(x)) {
                continue;
            }
            if ids.len() >= self.m0 {
                let Some(at) = ids.iter().rposition(|v| self.spare_edges(*v, e, before.contains(v)) > 0) else {
                    homeless.push(x);
                    continue;
                };
                ids.remove(at);
            }
            ids.push(x);
            changed = true;
        }
        (changed, homeless)
    }

    /// Links node `x` from node `from` on layer 0, in place of the
    /// farthest of its list another list leads to, if full (D97).
    pub fn foster(&mut self, from: u32, x: u32) {
        let mut ids = self.links(from, 0).to_vec();
        if from == x || ids.contains(&x) {
            return;
        }
        if ids.len() >= self.m0 {
            let Some(at) = ids.iter().rposition(|v| self.spare_edges(*v, from, true) > 0) else { return };
            ids.remove(at);
        }
        ids.push(x);
        self.set_links(from, 0, &ids);
    }

    /// The edges on layer 0 into node `x` but from node `e`, whose list
    /// holds it if `held`, and from its own neighbours.
    fn spare_edges(&self, x: u32, e: u32, held: bool) -> u32 {
        let mutual = self.links(x, 0).iter().filter(|y| **y != e && self.links(**y, 0).contains(&x)).count();
        self.incoming(x).saturating_sub(u32::from(held)).saturating_sub(u32::try_from(mutual).unwrap_or(u32::MAX))
    }

    /// Counts again how many of layer 0's lists hold each node, as when
    /// an image is read, whose lists name nodes not yet added.
    pub fn recount(&mut self) {
        self.incoming.clear();
        self.incoming.resize(self.len(), 0);
        let width = self.m0.saturating_add(1);
        for list in self.links0.chunks_exact(width) {
            let Some((degree, ids)) = list.split_first() else { continue };
            ids.iter().take(degree_of(*degree)).for_each(|e| tally(&mut self.incoming, *e, true));
        }
    }

    /// Adds `new` to node `id`'s neighbours on `level`, if it has room:
    /// whether it had.
    pub fn push_link(&mut self, id: u32, level: u8, new: u32) -> bool {
        let Some((up, start, width)) = self.span(id, level) else { return false };
        let Graph { upper, links0, incoming, .. } = self;
        let array = if up { upper } else { links0 };
        let Some((degree, slots)) = array.get_mut(start..start.saturating_add(width)).and_then(<[u32]>::split_first_mut) else {
            return false;
        };
        let at = degree_of(*degree);
        let Some(free) = slots.get_mut(at) else { return false };
        *free = new;
        if !up {
            tally(incoming, new, true);
        }
        // The new neighbour was not weighed by the heuristic, so which of
        // the list it kept is no longer known.
        *degree = u32::try_from(at.saturating_add(1)).unwrap_or(0);
        true
    }

    /// Adds a node of `row`, its vector `vector` of hash `hash`, counted
    /// `count`, up to layer `level`, with no neighbours yet: its id.
    pub fn push(&mut self, (row, hash, count): (u64, u64, u32), level: u8, vector: &[f32]) -> u32 {
        let id = u32::try_from(self.rows.len()).unwrap_or(NONE);
        match &mut self.store {
            Store::Full(floats) => floats.extend_from_slice(vector),
            Store::Coded { codec, codes } => {
                let start = codes.len();
                codes.resize(start.saturating_add(codec.code_bytes()), 0);
                codec.encode(vector, codes.get_mut(start..).unwrap_or_default());
            }
        }
        self.add_norm(id);
        self.add(id, (row, hash, count), level);
        id
    }

    /// Adds a node whose code is `code`, for an image read.
    pub fn push_code(&mut self, (row, hash, count): (u64, u64, u32), level: u8, code: &[u8]) -> u32 {
        let id = u32::try_from(self.rows.len()).unwrap_or(NONE);
        if let Store::Coded { codes, .. } = &mut self.store {
            codes.extend_from_slice(code);
        }
        self.add_norm(id);
        self.add(id, (row, hash, count), level);
        id
    }

    /// A buffer `vector` decodes a code into: none if it keeps no codes.
    fn decode_buffer(&self) -> Vec<f32> {
        if self.code_bytes().is_some() { vec![0.0_f32; self.dims] } else { Vec::new() }
    }

    /// Keeps the squared norm of node `id`'s vector, just stored.
    fn add_norm(&mut self, id: u32) {
        let mut buf = self.decode_buffer();
        let norm = squared_norm(self.vector(id, &mut buf));
        self.norms.push(norm);
    }

    fn add(&mut self, id: u32, (row, hash, count): (u64, u64, u32), level: u8) {
        self.rows.push(row);
        self.counts.push(count);
        self.levels.push(level);
        self.hashes.push(hash);
        self.incoming.push(0);
        self.links0.resize(self.links0.len().saturating_add(self.m0.saturating_add(1)), 0);
        if level == 0 {
            self.upper_at.push(NONE);
        } else {
            let width = self.m.saturating_add(1);
            self.upper_at.push(u32::try_from(self.upper.len().checked_div(width).unwrap_or(0)).unwrap_or(NONE));
            self.upper.resize(self.upper.len().saturating_add(width.saturating_mul(usize::from(level))), 0);
        }
        let before = self.by_hash.insert(hash, id).unwrap_or(NONE);
        self.same.push(before);
        if count > 0 {
            self.live = self.live.saturating_add(1);
        } else {
            self.dead = self.dead.saturating_add(1);
        }
    }

    /// The nodes of vector hash `hash`, most recent first.
    pub fn of_hash(&self, hash: u64) -> impl Iterator<Item = u32> + '_ {
        let mut next = self.by_hash.get(&hash).copied().unwrap_or(NONE);
        std::iter::from_fn(move || {
            let id = next;
            next = *self.same.get(slot(id))?;
            Some(id)
        })
    }

    /// Whether node `id` holds `vector`: exactly, for full vectors; by
    /// its hash alone, for codes, whose vectors are not kept.
    pub fn holds(&self, id: u32, vector: &[f32]) -> bool {
        match &self.store {
            Store::Full(_) => {
                let mut none = [];
                self.vector(id, &mut none).iter().zip(vector).all(|(a, b)| a.to_bits() == b.to_bits())
            }
            Store::Coded { .. } => true,
        }
    }

    /// The node of `row` holding `vector`, of hash `hash`, if any.
    pub fn find(&self, hash: u64, row: u64, vector: &[f32]) -> Option<u32> {
        self.of_hash(hash).find(|id| self.rows.get(slot(*id)) == Some(&row) && self.holds(*id, vector))
    }

    /// Changes node `id`'s count by one: up if `up`, else down.
    pub fn count(&mut self, id: u32, up: bool) -> Option<()> {
        let count = self.counts.get_mut(slot(id))?;
        let before = *count;
        *count = if up { before.checked_add(1)? } else { before.checked_sub(1)? };
        let after = *count;
        if before == 0 {
            self.dead = self.dead.saturating_sub(1);
            self.live = self.live.saturating_add(1);
        } else if after == 0 {
            self.live = self.live.saturating_sub(1);
            self.dead = self.dead.saturating_add(1);
        }
        Some(())
    }

    /// Whether node `id` is live, not a tombstone.
    #[inline]
    pub fn is_live(&self, id: u32) -> bool {
        self.counts.get(slot(id)).is_some_and(|c| *c > 0)
    }
}

/// \[DESIGN\] The bits of a list's first word holding its degree, enough
/// for M0 = 2 * MAX_M; above them, how many of its first neighbours the
/// heuristic kept, plus one, or 0 if not known (`insert::choose`).
const DEGREE_BITS: u32 = 16;
/// The degree's bits.
const DEGREE_MASK: u32 = (1 << DEGREE_BITS) - 1;

/// Counts one list more holding node `id` in `incoming` if `up`, else
/// one fewer.
fn tally(incoming: &mut [u32], id: u32, up: bool) {
    if let Some(n) = incoming.get_mut(slot(id)) {
        *n = if up { n.saturating_add(1) } else { n.saturating_sub(1) };
    }
}

/// The degree a list's first word holds.
#[inline]
fn degree_of(word: u32) -> usize {
    slot(word & DEGREE_MASK)
}

/// The bytes of an `f32`.
pub const F32_BYTES: usize = 4;
/// The bytes of a node id.
const ID_BYTES: usize = 4;
/// \[DESIGN\] A node's bytes besides its vector and lists: row, count,
/// layer, hash, upper block, hash chain, incoming count, and the hash
/// map's share.
const NODE_OVERHEAD: usize = 52;
