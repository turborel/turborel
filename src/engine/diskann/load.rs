//! \[DESIGN\] Building a whole index from its entries: "run Vamana on a
//! dataset P and store the resulting graph on an SSD" [DiskANN, NeurIPS
//! 2019 §3.1]. Entries are numbered and written as nodes without edges as
//! they come, a seeded reservoir sample kept for k-means and the codes'
//! training, and their vectors kept in memory while they fit the build
//! memory (reserved of the budget). Then two Vamana passes in memory
//! (`vamana.rs`), or, past the memory, the merged build (`merged.rs`),
//! give the edges; the codes are trained and written, the row map is
//! loaded sorted, and the cache is made.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::Reservation;
use crate::sync::lock_builtin;

use super::DiskAnn;
use super::consolidate::SAMPLE;
use super::meta::NO_NODE;
use super::node::{LIVE, Node, corrupt};
use super::prune::Vectors;
use super::random::Random;
use super::rowmap::{hash_of, key};
use super::vamana::{self, Build};

/// What a build's memory is reserved as.
const RESERVED_AS: &str = "the build of a DiskANN index";
/// Reservations grow by at least this, 1 MiB, so a build reserves rarely.
const RESERVE_STEP: usize = 1 << 20;
/// Bytes a node costs a build beside its vector: row, count, list slot.
const NODE_OVERHEAD: usize = 32;

/// Entries read so far.
struct Loaded {
    /// Vectors while they fit memory; none once past it.
    vectors: Option<Vec<f32>>,
    rows: Vec<u64>,
    counts: Vec<u32>,
    keys: HashMap<(u64, u64), u32>,
    sample: Vec<f32>,
    seen: usize,
    reservation: Reservation,
}

impl DiskAnn {
    /// Builds the index, empty, from `entries`: (vector, row id) pairs,
    /// repeats counted. Nothing else may use it until this returns.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is not empty, for a vector of other dimensions
    /// or a metric that broke its contract; -918 if the budget refuses
    /// the merged build's clusters; the entries' own; the buffer pool's.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        if lock_builtin(&self.inner.alloc).next != 0 {
            return Err(SqlError::new(Code::Api, "[DESIGN] a DiskANN index is loaded only while it is empty"));
        }
        let tuning = self.tuning()?;
        let mut loaded = Loaded {
            vectors: Some(Vec::new()),
            rows: Vec::new(),
            counts: Vec::new(),
            keys: HashMap::new(),
            sample: Vec::new(),
            seen: 0,
            reservation: self.inner.budget.reserve(0, RESERVED_AS)?,
        };
        let mut random = Random::new(self.inner.seed);
        for entry in entries {
            let (vector, row) = entry?;
            self.check_dims(&vector)?;
            self.take(&mut loaded, &vector, row, (&mut random, tuning.build_memory))?;
        }
        let n = loaded.rows.len();
        if n == 0 {
            return Ok(());
        }
        let medoid = match loaded.vectors.take() {
            Some(vectors) => self.build_in_memory(&vectors, &loaded)?,
            None => self.merged(&loaded.sample, u32::try_from(n).unwrap_or(u32::MAX))?,
        };
        self.inner.medoid.store(medoid, Ordering::Release);
        self.inner.live.store(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Release);
        self.load_rowmap(&loaded)?;
        drop(loaded);
        self.note()?;
        let sample = self.sample(SAMPLE)?;
        self.train(&sample)?;
        self.refresh_cache()
    }

    /// Takes one entry: a count on a node already made, or a new node.
    fn take(&self, loaded: &mut Loaded, vector: &[f32], row: u64, (random, memory): (&mut Random, usize)) -> Result<()> {
        let mut stored = Vec::new();
        self.inner.shape.vector_bytes(vector, &mut stored);
        let identity = (hash_of(&stored), row);
        if let Some(&id) = loaded.keys.get(&identity) {
            if !self.loaded_holds(loaded, id, &stored)? {
                return Err(SqlError::new(Code::Unsupported, "[EXTENSION] two vectors of one row share a DiskANN hash"));
            }
            let count =
                loaded.counts.get_mut(usize::try_from(id).unwrap_or(usize::MAX)).ok_or_else(|| corrupt("a loaded node is missing"))?;
            *count = count.checked_add(1).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a DiskANN entry has too many references"))?;
            return Ok(());
        }
        let id = u32::try_from(loaded.rows.len())
            .ok()
            .filter(|&n| n != NO_NODE)
            .ok_or_else(|| SqlError::new(Code::Unsupported, "[EXTENSION] a DiskANN index holds at most 2^32 - 2 nodes"))?;
        loaded.keys.insert(identity, id);
        loaded.rows.push(row);
        loaded.counts.push(1);
        self.reservoir(loaded, vector, random);
        let dims = self.inner.shape.dims;
        let wanted = loaded.rows.len().saturating_mul(
            dims.saturating_mul(size_of::<f32>())
                .saturating_add(NODE_OVERHEAD)
                .saturating_add(self.inner.shape.degree.saturating_mul(size_of::<u32>())),
        );
        let mut keep = wanted <= memory;
        if keep && wanted > loaded.reservation.bytes() {
            keep = loaded.reservation.resize(wanted.saturating_add(RESERVE_STEP)).is_ok();
        }
        if let Some(vectors) = loaded.vectors.as_mut().filter(|_| keep) {
            vectors.extend_from_slice(vector);
            return Ok(());
        }
        self.spill(loaded, id, vector)
    }

    /// Whether loaded node `id` stores `stored`.
    fn loaded_holds(&self, loaded: &Loaded, id: u32, stored: &[u8]) -> Result<bool> {
        let dims = self.inner.shape.dims;
        let Some(vectors) = loaded.vectors.as_ref() else {
            let record = self.record(id)?;
            return Ok(self.inner.shape.stored_vector(&record)? == stored);
        };
        let start = usize::try_from(id).unwrap_or(usize::MAX).saturating_mul(dims);
        let mut bytes = Vec::new();
        self.inner.shape.vector_bytes(vectors.get(start..start.saturating_add(dims)).unwrap_or_default(), &mut bytes);
        Ok(bytes == stored)
    }

    /// Past the build memory, the vectors held go to their pages and the
    /// build goes merged. This and later vectors are written at once.
    fn spill(&self, loaded: &mut Loaded, id: u32, vector: &[f32]) -> Result<()> {
        if let Some(vectors) = loaded.vectors.take() {
            self.write_graph(loaded, &vectors, None)?;
            loaded.reservation.resize(0)?;
        }
        self.grow_to(id)?;
        let row = loaded.rows.last().copied().unwrap_or(0);
        self.put(id, &Node { flags: LIVE, row, count: 1, vector: vector.to_vec(), norm: 0.0, neighbours: Vec::new() })
    }

    /// A seeded reservoir sample of at most `SAMPLE` vectors (Vitter's
    /// algorithm R), for k-means.
    fn reservoir(&self, loaded: &mut Loaded, vector: &[f32], random: &mut Random) {
        loaded.seen = loaded.seen.saturating_add(1);
        let dims = self.inner.shape.dims;
        if loaded.sample.len() < SAMPLE.saturating_mul(dims) {
            loaded.sample.extend_from_slice(vector);
            return;
        }
        let pick = random.below(loaded.seen);
        if pick < SAMPLE {
            let start = pick.saturating_mul(dims);
            if let Some(slot) = loaded.sample.get_mut(start..start.saturating_add(dims)) {
                slot.copy_from_slice(vector);
            }
        }
    }

    /// Node numbers up to `id` allocated, their chunk pages made.
    fn grow_to(&self, id: u32) -> Result<()> {
        let mut alloc = lock_builtin(&self.inner.alloc);
        while alloc.next <= id {
            let next = alloc.next;
            self.ensure_chunk(next)?;
            alloc.next = next.saturating_add(1);
        }
        Ok(())
    }

    /// Writes the loaded nodes of `vectors` whole chunk pages at a time,
    /// with `graph`'s edges if given.
    fn write_graph(&self, loaded_rows: &Loaded, vectors: &[f32], graph: Option<&vamana::Graph>) -> Result<()> {
        let dims = self.inner.shape.dims;
        let n = vectors.len().checked_div(dims).unwrap_or(0);
        if n == 0 {
            return Ok(());
        }
        self.grow_to(u32::try_from(n.saturating_sub(1)).unwrap_or(u32::MAX))?;
        let shape = self.inner.shape;
        let mut node = Node::default();
        for first in (0..n).step_by(shape.per_page) {
            let mut page = vec![0_u8; shape.page_bytes()];
            for (slot, id) in (first..n.min(first.saturating_add(shape.per_page))).enumerate() {
                let id32 = u32::try_from(id).unwrap_or(u32::MAX);
                node.flags = LIVE;
                node.row = loaded_rows.rows.get(id).copied().unwrap_or(0);
                node.count = loaded_rows.counts.get(id).copied().unwrap_or(1);
                node.vector.clear();
                node.vector
                    .extend_from_slice(vectors.get(id.saturating_mul(dims)..id.saturating_add(1).saturating_mul(dims)).unwrap_or_default());
                node.neighbours.clear();
                node.neighbours.extend_from_slice(graph.map(|g| g.neighbours(id32)).unwrap_or_default());
                shape.encode(&node, page.get_mut(slot.saturating_mul(shape.record)..).unwrap_or_default())?;
            }
            let id = self.page_of(u32::try_from(first).unwrap_or(u32::MAX))?;
            *self.inner.pool.write(self.inner.file, PageId(id.0))? = page;
        }
        Ok(())
    }

    /// Two Vamana passes over vectors in memory; the medoid.
    fn build_in_memory(&self, vectors: &[f32], loaded: &Loaded) -> Result<u32> {
        let tuning = self.tuning()?;
        let build = Build {
            list: usize::try_from(tuning.build_list).unwrap_or(usize::MAX),
            alpha: tuning.alpha,
            passes: tuning.passes,
            degree: self.inner.shape.degree,
            seed: self.inner.seed,
        };
        let view = Vectors { data: vectors, dims: self.inner.shape.dims };
        let (graph, medoid) = vamana::build(view, build, &*self.inner.plugins.metric)?;
        self.write_graph(loaded, vectors, Some(&graph))?;
        Ok(medoid)
    }

    /// The row map, loaded in key order.
    fn load_rowmap(&self, loaded: &Loaded) -> Result<()> {
        let mut keys: Vec<([u8; 16], u32)> = loaded.keys.iter().map(|(&(hash, row), &id)| (key(hash, row), id)).collect();
        keys.sort_unstable();
        let mut entries = keys.into_iter().map(|(k, id)| Ok((k.to_vec(), id.to_le_bytes().to_vec())));
        self.inner.rowmap.load(&mut entries)
    }
}
