//! \[DESIGN\] Keeping an HNSW index: its image written (`save`), a bulk
//! load, its maintenance (the vector design document's §10.6 and R10.10:
//! tombstones past 20% repaired and dropped; a quantizer trained once
//! enough rows are in; a long change log folded into a new image), and its
//! pages freed when it is dropped.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, RwLockReadGuard};

use super::chain::{Reader as ChainReader, Tail, Writer as ChainWriter, free_pages};
use super::graph::{Graph, Store};
use super::log::{MEMORY_STEP, grow};
use super::random::{Rng, vector_hash};
use super::view::{TRIES, contended};
use super::{Config, Hnsw, image, meta_bytes};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::Reservation;
use crate::sync::{lock, read, write};

/// \[DESIGN\] Tombstones are dropped past one node in five (R10.10's 20%).
const DEAD_SHARE: usize = 5;
/// \[DESIGN\] The log records past which maintenance writes a new image:
/// at least this many, so a small index does not write one a change.
const LOG_LEAST: u64 = 4_096;
/// \[DESIGN\] Past a quarter of the nodes, the log is folded into an
/// image: replaying it would cost more than reading them.
const LOG_SHARE: u64 = 4;
/// \[DESIGN\] The most vectors a codec is trained on: the vector design
/// document's R7.2 default sample.
pub const SAMPLE_MOST: usize = 100_000;

impl Hnsw {
    /// Writes its image to new pages and starts an empty log, the meta
    /// page naming them written last; the old pages freed after.
    ///
    /// # Errors
    ///
    /// The buffer pool's; the old image and log stand.
    pub fn save(&self) -> Result<()> {
        let inner = &*self.inner;
        let (graph, codec) = self.codec_bytes()?;
        let mut log = lock(&inner.log)?;
        let mut image_head = lock(&inner.image)?;
        let old = [
            ChainReader::new(&inner.pool, inner.file, *image_head)?.pages()?,
            ChainReader::new(&inner.pool, inner.file, log.0.head)?.pages()?,
        ];
        let mut w = ChainWriter::new(&inner.pool, inner.file)?;
        if let Err(error) = image::write(&graph, &codec, &mut w) {
            // Unpublished pages; a failed free leaks pages only.
            let _ = w.abandon();
            return Err(error);
        }
        let (head, mut pages) = w.finish()?;
        let fresh = Tail::create(&inner.pool, inner.file).and_then(|tail| {
            pages.push(tail.head);
            let config = self.config();
            let bytes = meta_bytes(&config, (inner.metric.name(), inner.quantizer.as_ref().map(|q| q.name())), (head, tail.head));
            *inner.pool.write(inner.file, inner.meta)? = bytes;
            Ok(tail)
        });
        let tail = match fresh {
            Ok(tail) => tail,
            Err(error) => {
                // As above: the meta page still names the old ones.
                let _ = free_pages(&inner.pool, inner.file, &pages);
                return Err(error);
            }
        };
        *image_head = head;
        *log = (tail, 0);
        old.iter().try_for_each(|chain| free_pages(&inner.pool, inner.file, chain))
    }

    /// The graph, read, and its codec's bytes, written by the codec with
    /// no lock held (R-ERR-4): read again if maintenance changed the
    /// codec meanwhile.
    fn codec_bytes(&self) -> Result<(RwLockReadGuard<'_, Graph>, Vec<u8>)> {
        let inner = &*self.inner;
        for _ in 0..TRIES {
            let codec = read(&inner.graph)?.codec().cloned();
            let mut bytes = Vec::new();
            if let Some(codec) = &codec {
                codec.write(&mut bytes);
            }
            let graph = read(&inner.graph)?;
            let same = match (graph.codec(), &codec) {
                (Some(now), Some(then)) => Arc::ptr_eq(now, then),
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            };
            if same {
                return Ok((graph, bytes));
            }
        }
        Err(contended())
    }

    /// The configuration it keeps in its meta page.
    fn config(&self) -> Config {
        let inner = &*self.inner;
        Config {
            dims: inner.dims,
            m: inner.m,
            ef_construction: inner.tuning.ef_construction.load(Relaxed),
            ef_search: inner.tuning.ef_search.load(Relaxed),
            extend: inner.tuning.extend.load(Relaxed),
            keep_pruned: inner.tuning.keep_pruned.load(Relaxed),
            seed: inner.seed,
            train_rows: inner.train_rows,
            rerank: inner.tuning.rerank.load(Relaxed),
        }
    }

    /// Changes the options that change in place: efConstruction,
    /// efSearch, extendCandidates, keepPrunedConnections and re-ranking.
    pub fn tune(&self, config: &Config) {
        let t = &self.inner.tuning;
        t.ef_construction.store(config.ef_construction.max(1), Relaxed);
        t.ef_search.store(config.ef_search.max(1), Relaxed);
        t.extend.store(config.extend, Relaxed);
        t.keep_pruned.store(config.keep_pruned, Relaxed);
        t.rerank.store(config.rerank, Relaxed);
    }

    /// Adds every entry of `entries` to the index, empty, which nothing
    /// else reads or writes meanwhile; trains its quantizer first if
    /// there are enough; then writes its image.
    ///
    /// # Errors
    ///
    /// As `insert`; the entries' own.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        let inner = &*self.inner;
        let mut flat = Vec::new();
        let mut rows = Vec::new();
        let mut held = inner.budget.reserve(0, "an HNSW index's load")?;
        for entry in entries {
            let (vector, row) = entry?;
            inner.metric.check(&vector, inner.dims)?;
            held.resize(held.bytes().saturating_add(size_of_val(vector.as_slice())))?;
            flat.extend_from_slice(&vector);
            rows.push(row);
        }
        // Built apart, with no lock held as the metric and codec run
        // (R-ERR-4), then put in place of the empty graph.
        let mut graph = Graph::new(inner.dims, (inner.m, inner.m.saturating_mul(2)), Store::Full(Vec::new()));
        if rows.len() >= inner.train_rows {
            self.train(&mut graph, &flat)?;
        }
        for (vector, row) in flat.chunks_exact(inner.dims).zip(rows) {
            if let Some(bytes) = inner.insert_owned(&mut graph, (vector, vector_hash(vector), row)) {
                grow(&mut *lock(&inner.memory)?, bytes)?;
            }
        }
        {
            let mut now = write(&inner.graph)?;
            if now.len() > 0 {
                return Err(SqlError::new(Code::Api, "[DESIGN] an HNSW index was written to during its load"));
            }
            graph.epoch = now.epoch.wrapping_add(1);
            graph.changes = now.changes.wrapping_add(1);
            *now = graph;
            settle(&mut *lock(&inner.memory)?, &now);
        }
        drop(held);
        self.save()
    }

    /// Trains the quantizer, if it has one, on a sample of `flat`, and
    /// makes every node of `graph` keep its code.
    fn train(&self, graph: &mut Graph, flat: &[f32]) -> Result<()> {
        let inner = &*self.inner;
        let Some(quantizer) = inner.quantizer.as_ref() else { return Ok(()) };
        let codec = quantizer.train(&sample(flat, inner.dims, inner.seed), inner.dims, inner.seed)?;
        let Store::Full(floats) = &graph.store else { return Ok(()) };
        let bytes = codec.code_bytes();
        let mut encoded = vec![0_u8; floats.len().checked_div(inner.dims).unwrap_or(0).saturating_mul(bytes)];
        for (vector, code) in floats.chunks_exact(inner.dims).zip(encoded.chunks_exact_mut(bytes.max(1))) {
            codec.encode(vector, code);
        }
        graph.store = Store::Coded { codec, codes: encoded };
        Ok(())
    }

    /// Whether maintenance is due: tombstones past a fifth, a quantizer
    /// to train, or a long log.
    pub fn maintenance_due(&self) -> bool {
        let inner = &*self.inner;
        let Ok(graph) = read(&inner.graph) else { return false };
        let records = lock(&inner.log).map_or(0, |log| log.1);
        let nodes = u64::try_from(graph.len()).unwrap_or(u64::MAX);
        let dead = graph.dead > 0 && graph.dead.saturating_mul(DEAD_SHARE) >= graph.len();
        let untrained = inner.quantizer.is_some() && graph.code_bytes().is_none() && graph.live >= inner.train_rows;
        dead || untrained || records > LOG_LEAST.max(nodes.checked_div(LOG_SHARE).unwrap_or(0))
    }

    /// Does the maintenance due, and whether it did any.
    ///
    /// # Errors
    ///
    /// As `save`; the quantizer's; a poisoned lock.
    pub fn maintain(&self) -> Result<bool> {
        if !self.maintenance_due() {
            return Ok(false);
        }
        let inner = &*self.inner;
        if let Some((mut copy, changes, held)) = self.copied()? {
            let mut changed = false;
            if inner.quantizer.is_some() && copy.code_bytes().is_none() && copy.live >= inner.train_rows {
                let flat = live_vectors(&copy);
                self.train(&mut copy, &flat)?;
                changed = true;
            }
            if copy.dead > 0 && copy.dead.saturating_mul(DEAD_SHARE) >= copy.len() {
                let mut s = inner.scratch.take();
                copy.repair(&inner.metric, &inner.build(), &mut s);
                inner.scratch.give(s);
                copy = copy.compacted();
                changed = true;
            }
            let swapped = !changed || self.swap(copy, changes)?;
            drop(held);
            if !swapped {
                // Changed meanwhile: the work is let go, and maintenance
                // stays due for the next try.
                return Ok(false);
            }
        }
        self.save()?;
        Ok(true)
    }

    /// A copy of the graph, its memory reserved of the budget, for
    /// maintenance to work on with no lock held (R-ERR-4), and its count
    /// of changes, and the copy's reservation; none if it needs no repair
    /// or training, only a save.
    fn copied(&self) -> Result<Option<(Graph, u64, Reservation)>> {
        let inner = &*self.inner;
        let graph = read(&inner.graph)?;
        let untrained = inner.quantizer.is_some() && graph.code_bytes().is_none() && graph.live >= inner.train_rows;
        let dead = graph.dead > 0 && graph.dead.saturating_mul(DEAD_SHARE) >= graph.len();
        if !(untrained || dead) {
            return Ok(None);
        }
        let used = lock(&inner.memory)?.1;
        let held = inner.budget.reserve(used, "an HNSW index's maintenance copy")?;
        let copy = graph.clone();
        let changes = graph.changes;
        Ok(Some((copy, changes, held)))
    }

    /// Puts `built` in place of the graph if it has had no change since
    /// `changes`, in a new epoch: whether it did.
    fn swap(&self, built: Graph, changes: u64) -> Result<bool> {
        let inner = &*self.inner;
        let mut graph = write(&inner.graph)?;
        if graph.changes != changes {
            return Ok(false);
        }
        let mut built = built;
        built.epoch = graph.epoch.wrapping_add(1);
        built.changes = changes.wrapping_add(1);
        *graph = built;
        inner.scratch.clear();
        settle(&mut *lock(&inner.memory)?, &graph);
        Ok(true)
    }

    /// Frees every page it holds: for an index dropped, which nothing
    /// reads again.
    ///
    /// # Errors
    ///
    /// The buffer pool's; some pages may be freed already.
    pub fn free(&self) -> Result<()> {
        let inner = &*self.inner;
        let image: PageId = *lock(&inner.image)?;
        let log = lock(&inner.log)?.0.head;
        for head in [image, log] {
            free_pages(&inner.pool, inner.file, &ChainReader::new(&inner.pool, inner.file, head)?.pages()?)?;
        }
        inner.pool.free(inner.file, inner.meta)
    }
}

/// Counts what `graph` holds now as the reservation's use, giving back
/// what it no longer needs past a step.
fn settle(memory: &mut (Reservation, usize), graph: &Graph) {
    let used: usize = (0..graph.len()).map(|at| graph.node_bytes(graph.levels.get(at).copied().unwrap_or(0))).sum();
    memory.1 = used;
    let enough = used.saturating_add(MEMORY_STEP);
    if memory.0.bytes() > enough {
        memory.0.resize_anyway(enough);
    }
}

/// The live nodes' full vectors, one after another, for training.
fn live_vectors(graph: &Graph) -> Vec<f32> {
    let mut flat = Vec::new();
    let mut buf = vec![0.0_f32; graph.dims];
    for id in (0..u32::try_from(graph.len()).unwrap_or(u32::MAX)).filter(|id| graph.is_live(*id)) {
        flat.extend_from_slice(graph.vector(id, &mut buf));
    }
    flat
}

/// At most `SAMPLE_MOST` of `flat`'s vectors, chosen by `seed`.
pub fn sample(flat: &[f32], dims: usize, seed: u64) -> Vec<f32> {
    let n = flat.len().checked_div(dims).unwrap_or(0);
    if n <= SAMPLE_MOST {
        return flat.to_vec();
    }
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = Rng::new(seed);
    for at in 0..SAMPLE_MOST {
        let pick = at.saturating_add(rng.below(n.saturating_sub(at)));
        order.swap(at, pick);
    }
    let mut out = Vec::with_capacity(SAMPLE_MOST.saturating_mul(dims));
    for &at in order.iter().take(SAMPLE_MOST) {
        out.extend_from_slice(flat.get(at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims)).unwrap_or_default());
    }
    out
}
