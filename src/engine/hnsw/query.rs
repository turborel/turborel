//! \[DESIGN\] Reading an HNSW index: the k nearest rows, the rows within a
//! radius, and the rows of one vector. Results are (distance, row) pairs,
//! nearest first, ties by row (vector design document R9.5). A quantized
//! index measures codes, so it gives RERANK_FACTOR times k candidates
//! (R7.1), re-scored exactly by the caller's `exact` if given.

use std::sync::atomic::Ordering::Relaxed;

use std::mem::take;
use std::sync::Arc;

use super::graph::slot;
use super::random::vector_hash;
use super::scratch::Scratch;
use super::search::{knn, range};
use super::view::{Halt, Shared, Source, Step, TRIES, contended};
use super::{Hnsw, Inner};
use crate::error::Result;
use crate::sync::read;

/// \[DESIGN\] The candidates a quantized search gives per row asked for,
/// re-scored at full precision: the vector design document's R7.1
/// RERANK_FACTOR default.
pub const RERANK_FACTOR: usize = 4;

/// Whether a row may be a result.
pub type Filter<'a> = &'a mut dyn FnMut(u64) -> bool;
/// A row's exact distance, if it has one.
pub type Exact<'a> = &'a mut dyn FnMut(u64) -> Option<f32>;

/// One search's requests beyond its vector.
#[derive(Default)]
pub struct Search<'a> {
    /// efSearch for this query, as `WITH TARGET ACCURACY PARAMETERS
    /// (EFSEARCH n)` gives it; else the index's.
    pub ef: Option<usize>,
    /// The partitions probed, as `NEIGHBOR PARTITION PROBES n` gives
    /// them, for an IVF index; else the index's.
    pub probes: Option<usize>,
    /// Whether a row may be a result: a row filter, as a WHERE clause's.
    pub filter: Option<Filter<'a>>,
    /// A row's exact distance, for re-ranking candidates measured by codes.
    pub exact: Option<Exact<'a>>,
}

/// Sorts `found` nearest first, ties by row, re-scored by `exact` if
/// given, and keeps the first `k`.
pub fn finish(found: &mut Vec<(f32, u64)>, exact: Option<Exact<'_>>, k: usize) {
    if let Some(exact) = exact {
        found.retain_mut(|(d, row)| match exact(*row) {
            Some(e) => {
                *d = e;
                true
            }
            None => false,
        });
    }
    found.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    found.truncate(k);
}

impl Hnsw {
    /// The `k` nearest rows to `q` that `search.filter` takes. The graph
    /// is read a hop at a time, the metric, codec and filter run with no
    /// lock held (R-ERR-4), so a filter may use the index itself.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for a vector of the wrong length or a metric giving
    /// no finite distance, or an index maintenance kept renumbering; a
    /// poisoned lock.
    pub fn nearest(&self, q: &[f32], k: usize, search: Search<'_>) -> Result<Vec<(f32, u64)>> {
        let inner = &*self.inner;
        inner.metric.check(q, inner.dims)?;
        let ef = search.ef.unwrap_or_else(|| inner.tuning.ef_search.load(Relaxed)).max(1);
        let Search { mut filter, exact, .. } = search;
        let rerank = inner.tuning.rerank.load(Relaxed);
        let mut wanted = k;
        let mut found = self.searched(&mut |src, s| {
            wanted = if src.codec().is_some() && rerank { k.saturating_mul(RERANK_FACTOR) } else { k };
            knn(src, (&inner.metric, q), (wanted, ef), s, &mut |_, row| takes(row, &mut filter))
        })?;
        let keep = if exact.is_some() { k } else { wanted };
        finish(&mut found, exact, keep);
        Ok(found)
    }

    /// Every row within `radius` of `q` that `search.filter` takes, as
    /// far as the graph's expansion reaches (R9.1): approximate, as any
    /// search of it is.
    ///
    /// # Errors
    ///
    /// As `nearest`.
    pub fn range(&self, q: &[f32], radius: f32, search: Search<'_>) -> Result<Vec<(f32, u64)>> {
        let inner = &*self.inner;
        inner.metric.check(q, inner.dims)?;
        let ef = search.ef.unwrap_or_else(|| inner.tuning.ef_search.load(Relaxed)).max(1);
        let Search { mut filter, exact, .. } = search;
        let mut found =
            self.searched(&mut |src, s| range(src, (&inner.metric, q), (radius, ef), s, &mut |_, row| takes(row, &mut filter)))?;
        let rescored = exact.is_some();
        finish(&mut found, exact, usize::MAX);
        if rescored {
            found.retain(|(d, _)| *d <= radius);
        }
        Ok(found)
    }

    /// The (distance, row) pairs of the nodes `run` leaves in
    /// `s.chosen`, run over the shared graph, again if maintenance
    /// renumbered it meanwhile.
    fn searched(&self, run: &mut Run<'_>) -> Result<Vec<(f32, u64)>> {
        let inner = &*self.inner;
        let mut s = inner.scratch.take();
        let mut outcome = None;
        for _ in 0..TRIES {
            outcome = self.search_once(run, &mut s).transpose();
            if outcome.is_some() {
                break;
            }
        }
        inner.scratch.give(s);
        outcome.unwrap_or_else(|| Err(contended()))
    }

    /// One try of `searched`: none if maintenance renumbered the graph.
    fn search_once(&self, run: &mut Run<'_>, s: &mut Scratch) -> Result<Option<Vec<(f32, u64)>>> {
        let mut src = Shared::new(&self.inner.graph, take(&mut s.hop))?;
        let (mut ids, mut rows) = (take(&mut s.ids), take(&mut s.rows));
        let done = run(&mut src, s).and_then(|()| {
            ids.clear();
            ids.extend(s.chosen.iter().map(|c| c.id));
            src.rows(&ids, &mut rows)
        });
        s.hop = src.done();
        let found = match done {
            Ok(()) => Ok(Some(s.chosen.iter().zip(&rows).map(|(c, row)| (c.distance, *row)).collect())),
            Err(Halt::Moved) => Ok(None),
            Err(Halt::Failed(error)) => Err(error),
        };
        (s.ids, s.rows) = (ids, rows);
        found
    }

    /// The rows with an entry under `vector`, each once.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        let graph = read(&self.inner.graph)?;
        let mut rows: Vec<u64> = graph
            .of_hash(vector_hash(vector))
            .filter(|id| graph.is_live(*id) && graph.holds(*id, vector))
            .filter_map(|id| graph.rows.get(slot(id)).copied())
            .collect();
        rows.sort_unstable();
        rows.dedup();
        Ok(rows)
    }

    /// The row of every live node, once a node, copied `ENTRIES_CHUNK`
    /// nodes at a time under the read lock, never held while the iterator
    /// is consumed. A node maintenance renumbers meanwhile, compacting the
    /// graph, may be given twice or missed; nodes appended meanwhile are
    /// given if not yet passed. A poisoned lock is its last item.
    pub fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        Box::new(Entries { inner: Arc::clone(&self.inner), next: 0, rows: Vec::new(), done: false })
    }

    /// Its live nodes and tombstones.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn counts(&self) -> Result<(usize, usize)> {
        let graph = read(&self.inner.graph)?;
        Ok((graph.live, graph.dead))
    }

    /// Whether it keeps codes, its quantizer trained.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn quantized(&self) -> Result<bool> {
        Ok(read(&self.inner.graph)?.code_bytes().is_some())
    }
}

/// A search run over the shared graph, its results left in `s.chosen`.
type Run<'a> = dyn FnMut(&mut Shared<'_>, &mut Scratch) -> Step<()> + 'a;

/// Whether a node of row `row`, if live, may be a result: live, its row
/// taken by `filter`.
fn takes(row: Option<u64>, filter: &mut Option<Filter<'_>>) -> bool {
    row.is_some_and(|row| filter.as_mut().is_none_or(|f| f(row)))
}

/// \[DESIGN\] The nodes an `entries` iterator reads at once under the lock:
/// enough that the lock is seldom taken, few enough that a writer waits little.
const ENTRIES_CHUNK: usize = 1_024;

/// An iterator of the live nodes' rows.
struct Entries {
    inner: Arc<Inner>,
    /// The next node to read.
    next: usize,
    /// Rows read and not yet given, last first.
    rows: Vec<u64>,
    /// Whether it has ended, or failed.
    done: bool,
}

impl Iterator for Entries {
    type Item = Result<u64>;

    fn next(&mut self) -> Option<Result<u64>> {
        loop {
            if let Some(row) = self.rows.pop() {
                return Some(Ok(row));
            }
            if self.done {
                return None;
            }
            let graph = match read(&self.inner.graph) {
                Ok(graph) => graph,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            };
            let end = self.next.saturating_add(ENTRIES_CHUNK).min(graph.len());
            self.rows.extend(
                (self.next..end)
                    .filter_map(|at| u32::try_from(at).ok())
                    .filter(|id| graph.is_live(*id))
                    .filter_map(|id| graph.rows.get(slot(id)).copied()),
            );
            self.done = end <= self.next;
            self.next = end;
            self.rows.reverse();
        }
    }
}
