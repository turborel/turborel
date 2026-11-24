//! \[DESIGN\] HNSW changes and their log. Each insert and remove is
//! appended to the change log before the graph changes, under the graph's
//! exclusive lock, so the log's order is the graph's. An insert's search
//! and neighbour selection run before, with no lock held (R-ERR-4). A change whose log
//! record cannot be written makes none (RULES.md D19); once written, the
//! graph's change cannot fail, its memory reserved before.
//!
//! Opening reads the image, then replays the log over it. A record cut
//! short at the log's end, which a checkpoint taken during an append may
//! hold, is dropped and cut off; recovery's mending gives back its entry.

use std::sync::Arc;

use super::chain::{Reader as ChainReader, Tail, corrupt, put_floats, put_varint};
use std::mem::take;

use super::graph::{Graph, Store};
use super::image::{self, Shape};
use super::insert::{Build, Payload, plan, reselect};
use super::measure::Codec;
use super::random::{level_of, vector_hash};
use super::scratch::Scratch;
use super::view::{Halt, Owned, Shared, TRIES, contended};
use super::{Config, Hnsw, Inner, Parts, Quantizer, read_meta, validate};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};
use crate::sync::{lock, read, write};

use super::measure::Metric;

/// A log record of an insert: its row, then its vector.
const INSERT: u8 = 1;
/// A log record of a remove: its row, then its vector's hash.
const REMOVE: u8 = 2;
/// \[DESIGN\] The bytes the graph's reservation grows by at once, so a
/// node's insert seldom asks the budget.
pub const MEMORY_STEP: usize = 1 << 20;

/// Counts `bytes` more of `memory`, growing its reservation a step past
/// them if needed.
///
/// # Errors
///
/// SQLCODE -918 if the budget refuses.
pub fn grow(memory: &mut (Reservation, usize), bytes: usize) -> Result<()> {
    let need = memory.1.saturating_add(bytes);
    if need > memory.0.bytes() {
        memory.0.resize(need.saturating_add(MEMORY_STEP))?;
    }
    memory.1 = need;
    Ok(())
}

impl Inner {
    /// How insertions choose neighbours now.
    pub(super) fn build(&self) -> Build {
        use std::sync::atomic::Ordering::Relaxed;
        Build {
            ef_construction: self.tuning.ef_construction.load(Relaxed),
            extend: self.tuning.extend.load(Relaxed),
            keep_pruned: self.tuning.keep_pruned.load(Relaxed),
            m: self.m,
            m0: self.m.saturating_mul(2),
        }
    }

    /// Appends `record` to the change log.
    fn append(&self, record: &[u8]) -> Result<()> {
        let mut log = lock(&self.log)?;
        log.0.append(&self.pool, self.file, record)?;
        log.1 = log.1.saturating_add(1);
        Ok(())
    }

    /// Inserts `(vector, row)` into `graph`, one nothing else sees, as
    /// a load or a replay builds: counted once more if there; the bytes
    /// of the node added, if one was.
    pub(super) fn insert_owned(&self, graph: &mut Graph, (vector, hash, row): (&[f32], u64, u64)) -> Option<usize> {
        if let Some(id) = graph.find(hash, row, vector) {
            graph.count(id, true);
            return None;
        }
        let level = level_of(self.seed, row, hash, self.m);
        let build = self.build();
        let mut s = self.scratch.take();
        stored(graph.codec(), vector, &mut s);
        let (query, code) = (take(&mut s.query), take(&mut s.code));
        let mut src = Owned::new(graph, take(&mut s.owned));
        // A direct read never halts: it takes no lock and sees no epoch.
        let _ = plan(&mut src, &self.metric, (&query, level), &build, &mut s);
        s.owned = src.done();
        let payload = if graph.codec().is_some() { Payload::Code(&code) } else { Payload::Floats(vector) };
        let id = graph.publish((row, hash, level), &payload, &mut s);
        let over = take(&mut s.over);
        for &(e, at) in &over {
            let mut again = Owned::new(graph, take(&mut s.owned));
            let chosen = reselect(&mut again, &self.metric, (e, at, id), &build, &mut s);
            s.owned = again.done();
            if chosen.is_ok() {
                graph.apply((e, at, id), &mut s);
            }
        }
        (s.query, s.code, s.over) = (query, code, over);
        self.scratch.give(s);
        Some(graph.node_bytes(level))
    }

    /// Plans the insert of a node of `vector` and layer `level`
    /// with no lock held, into `s.plan`: the epoch it read and the entry
    /// it searched from, or none if it must start again.
    fn planned(&self, vector: &[f32], level: u8, s: &mut Scratch) -> Result<Option<(u64, u32)>> {
        let mut src = Shared::new(&self.graph, take(&mut s.hop))?;
        let epoch = src.epoch;
        stored(src.codec(), vector, s);
        let query = take(&mut s.query);
        let planned = plan(&mut src, &self.metric, (&query, level), &self.build(), s);
        s.query = query;
        s.hop = src.done();
        match planned {
            Ok(entry) => Ok(Some((epoch, entry))),
            Err(Halt::Moved) => Ok(None),
            Err(Halt::Failed(error)) => Err(error),
        }
    }

    /// Chooses node `e`'s list on `level` again with `new` among the
    /// candidates, as its back edge did not fit: computed with no lock
    /// held, written only if the list is still as read, else again, until
    /// it is: a back edge is the way searches reach `new`, so none is
    /// dropped. Each try lost is another writer's change of the list made,
    /// so the writers as a whole make progress. Maintenance swaps in no
    /// graph while a writer is past the fence (`fence.rs`), so the graph is
    /// not renumbered under a back edge; were it, `new` would be in the
    /// repaired graph as its copy found it.
    fn back_edge(&self, (e, level, new): (u32, u8, u32), s: &mut Scratch) -> Result<()> {
        let build = self.build();
        loop {
            let mut src = Shared::new(&self.graph, take(&mut s.hop))?;
            let epoch = src.epoch;
            let chosen = reselect(&mut src, &self.metric, (e, level, new), &build, s);
            s.hop = src.done();
            match chosen {
                Ok(()) => {}
                Err(Halt::Moved) => return Ok(()),
                Err(Halt::Failed(error)) => return Err(error),
            }
            let mut graph = write(&self.graph)?;
            if graph.epoch != epoch || graph.apply((e, level, new), s) {
                return Ok(());
            }
        }
    }
}

/// The vector `vector` stands for once stored by `codec`, in `s.query`,
/// and its code, in `s.code`. Calls the codec, so no lock may be held.
fn stored(codec: Option<&Arc<dyn Codec>>, vector: &[f32], s: &mut Scratch) {
    s.query.clear();
    s.code.clear();
    match codec {
        None => s.query.extend_from_slice(vector),
        Some(codec) => {
            s.code.resize(codec.code_bytes(), 0);
            codec.encode(vector, &mut s.code);
            s.query.resize(vector.len(), 0.0);
            codec.decode(&s.code, &mut s.query);
        }
    }
}

/// The log record of an insert of `row` under `vector`.
fn insert_record(row: u64, vector: &[f32]) -> Vec<u8> {
    let mut record = vec![INSERT];
    put_varint(&mut record, row);
    put_floats(&mut record, vector);
    record
}

impl Hnsw {
    /// Adds one entry of `row` under `vector`. Its lists are chosen with
    /// no lock held (R-ERR-4); then the log record is appended and the
    /// node published under the graph's lock, so the log's order is the
    /// graph's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for a vector of the wrong length or a metric giving
    /// no finite distance, or an index maintenance kept renumbering; -918
    /// if the budget refuses its memory; the buffer pool's. A failed
    /// insert adds nothing.
    pub fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        let inner = &*self.inner;
        inner.metric.check(vector, inner.dims)?;
        let _writing = inner.fence.pass()?;
        let hash = vector_hash(vector);
        let mut s = inner.scratch.take();
        let done = (|| {
            for _ in 0..TRIES {
                if self.try_insert(vector, (hash, row), &mut s)? {
                    return Ok(());
                }
            }
            Err(contended())
        })();
        inner.scratch.give(s);
        done
    }

    /// One try of `insert`: whether it was done, or must start again.
    fn try_insert(&self, vector: &[f32], (hash, row): (u64, u64), s: &mut Scratch) -> Result<bool> {
        let inner = &*self.inner;
        let level = level_of(inner.seed, row, hash, inner.m);
        let held = read(&inner.graph)?.find(hash, row, vector).is_some();
        let planned = if held { None } else { inner.planned(vector, level, s)? };
        let id = {
            let mut graph = write(&inner.graph)?;
            if let Some(id) = graph.find(hash, row, vector) {
                return self.count_up(&mut graph, id, (row, vector)).map(|()| true);
            }
            let Some((epoch, entry)) = planned else { return Ok(false) };
            if graph.epoch != epoch || (entry == graph::NONE && graph.entry != graph::NONE) {
                return Ok(false);
            }
            grow(&mut *lock(&inner.memory)?, graph.node_bytes(level))?;
            inner.append(&insert_record(row, vector))?;
            let code = take(&mut s.code);
            let payload = if graph.codec().is_some() { Payload::Code(&code) } else { Payload::Floats(vector) };
            let id = graph.publish((row, hash, level), &payload, s);
            s.code = code;
            id
        };
        let over = take(&mut s.over);
        let linked = over.iter().try_for_each(|(e, at)| inner.back_edge((*e, *at, id), s));
        s.over = over;
        linked.map(|()| true)
    }

    /// Counts node `id` of `graph`, an entry of `row` under `vector`,
    /// once more, its log record appended first.
    fn count_up(&self, graph: &mut Graph, id: u32, (row, vector): (u64, &[f32])) -> Result<()> {
        if graph.counts.get(graph::slot(id)).is_some_and(|c| *c == u32::MAX) {
            return Err(SqlError::new(Code::Api, "[DESIGN] an HNSW entry is inserted too many times"));
        }
        self.inner.append(&insert_record(row, vector))?;
        graph.count(id, true);
        graph.changes = graph.changes.wrapping_add(1);
        Ok(())
    }

    /// Takes back one entry of `row` under `vector`, if there is one: its
    /// node is a tombstone once none is left.
    ///
    /// # Errors
    ///
    /// The buffer pool's; a failed remove takes back nothing.
    pub fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        let inner = &*self.inner;
        let hash = vector_hash(vector);
        let _writing = inner.fence.pass()?;
        let mut graph = write(&inner.graph)?;
        let Some(id) = graph.find(hash, row, vector).filter(|id| graph.is_live(*id)) else { return Ok(()) };
        let mut record = vec![REMOVE];
        put_varint(&mut record, row);
        record.extend_from_slice(&hash.to_le_bytes());
        inner.append(&record)?;
        graph.count(id, false);
        graph.changes = graph.changes.wrapping_add(1);
        Ok(())
    }

    /// The index whose meta page is `meta`, as it was when its pages were
    /// last written: its image read and its log replayed. Its options
    /// that change in place come from `tuning`; the rest from its pages.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 for malformed pages, or a metric or quantizer not the
    /// one it was made with; -918 if the budget refuses its graph; the
    /// buffer pool's.
    pub fn open(
        pool: Arc<BufferPool>,
        (file, meta): (FileId, PageId),
        tuning: &Config,
        (metric, quantizer): (Metric, Option<Arc<dyn Quantizer>>),
        budget: &Arc<MemoryBudget>,
    ) -> Result<Hnsw> {
        let m = read_meta(&pool.read(file, meta.0)?)?;
        if m.metric != metric.name() || m.quantizer != quantizer.as_ref().map(|q| q.name()).unwrap_or_default() {
            return Err(corrupt("its metric or quantizer is not the one it was made with"));
        }
        let config = Config { dims: m.dims, m: m.m, seed: m.seed, train_rows: m.train_rows, ..*tuning };
        validate(&config)?;
        let mut memory = (budget.reserve(0, "an HNSW index's graph")?, 0);
        let shape = Shape { dims: m.dims, m: (m.m, m.m.saturating_mul(2)), quantizer: quantizer.as_ref() };
        let graph = image::read(&mut ChainReader::new(&pool, file, m.image)?, &shape, &mut |bytes| grow(&mut memory, bytes))?;
        let log = Tail { head: m.log, last: m.log };
        let hnsw = Hnsw::with(Parts {
            pool,
            file,
            meta,
            config,
            metric,
            quantizer,
            graph,
            log,
            image: m.image,
            memory,
            budget: Arc::clone(budget),
        });
        hnsw.replay(m.log)?;
        Ok(hnsw)
    }

    /// Replays the log from `head` over the graph read: taken out of the
    /// index, which nothing else holds yet, so the metric and codec run
    /// with no lock held (R-ERR-4).
    fn replay(&self, head: PageId) -> Result<()> {
        let inner = &*self.inner;
        let mut graph = self.taken()?;
        let replayed = self.replay_into(head, &mut graph);
        *write(&inner.graph)? = graph;
        replayed
    }

    /// The graph, taken out of the index, an empty one left in its place.
    pub(super) fn taken(&self) -> Result<Graph> {
        let inner = &*self.inner;
        let empty = Graph::new(inner.dims, (inner.m, inner.m.saturating_mul(2)), Store::Full(Vec::new()));
        Ok(std::mem::replace(&mut *write(&inner.graph)?, empty))
    }

    /// `replay` over `graph`.
    fn replay_into(&self, head: PageId, graph: &mut Graph) -> Result<()> {
        let inner = &*self.inner;
        let mut r = ChainReader::new(&inner.pool, inner.file, head)?;
        let (mut mark, mut records, mut vector) = (r.mark(), 0_u64, vec![0.0_f32; inner.dims]);
        let cut = loop {
            if r.at_end()? {
                break false;
            }
            match self.replay_one(&mut r, graph, &mut vector) {
                Ok(()) => {
                    mark = r.mark();
                    records = records.saturating_add(1);
                }
                Err(error) => {
                    if !r.ended() {
                        return Err(error);
                    }
                    break true;
                }
            }
        };
        let tail = if cut {
            Tail::cut(&inner.pool, inner.file, head, (mark, &r.pages))?
        } else {
            Tail { head, last: r.pages.last().copied().unwrap_or(head) }
        };
        *lock(&inner.log)? = (tail, records);
        Ok(())
    }

    /// Replays the record `r` is at.
    fn replay_one(&self, r: &mut ChainReader<'_>, graph: &mut Graph, vector: &mut [f32]) -> Result<()> {
        let inner = &*self.inner;
        let op = r.u8()?;
        let row = r.varint()?;
        match op {
            INSERT => {
                r.floats(vector)?;
                if let Some(bytes) = inner.insert_owned(graph, (vector, vector_hash(vector), row)) {
                    grow(&mut *lock(&inner.memory)?, bytes)?;
                }
            }
            REMOVE => {
                let hash = r.u64()?;
                let found = graph.of_hash(hash).find(|id| graph.rows.get(graph::slot(*id)) == Some(&row) && graph.is_live(*id));
                if let Some(id) = found {
                    graph.count(id, false);
                }
            }
            _ => return Err(corrupt("a log record is of no known kind")),
        }
        Ok(())
    }
}

use super::graph;

/// The file and pool a log lives in, for its tests.
#[cfg(test)]
pub(super) fn pages(hnsw: &Hnsw) -> (Arc<BufferPool>, FileId) {
    (Arc::clone(&hnsw.inner.pool), hnsw.inner.file)
}
