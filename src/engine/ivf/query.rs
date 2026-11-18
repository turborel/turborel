//! \[DESIGN\] IVF search: the centroids measured, then the NPROBE nearest
//! lists scanned a page at a time, keeping the k nearest live entries the
//! filter takes in a bounded heap. Past the NPROBE lists, the next nearest
//! are scanned too until the filter has taken as many entries as those
//! lists held, and k, so a selective filter widens the search in step
//! with its selectivity rather than giving fewer or worse rows (vector
//! design document R9.3). A query
//! may set its own probes, the document's NEIGHBOR PARTITION PROBES
//! (§9.2). A quantized index measures codes and gives RERANK_FACTOR
//! times k candidates, re-scored exactly by the caller's `exact` (R7.1).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::write::floats_of;
use super::{Inner, Ivf, Shape, lists};
use crate::disk::pager::PageId;
use crate::engine::hnsw::{Filter, Probe, RERANK_FACTOR, Search, finish};
use crate::error::Result;
use crate::sync::lock_builtin;

/// \[DESIGN\] An automatic NPROBE scans one list in eight.
const AUTO_PROBE_SHARE: usize = 8;
/// \[DESIGN\] The buffer sets kept for searches, one a thread searching.
const POOLED: usize = 64;

/// A row and its distance, ordered by distance, then row (R9.5).
#[derive(Debug, Clone, Copy)]
struct Near {
    distance: f32,
    row: u64,
}

impl Ord for Near {
    fn cmp(&self, o: &Self) -> Ordering {
        self.distance.total_cmp(&o.distance).then(self.row.cmp(&o.row))
    }
}

impl PartialOrd for Near {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialEq for Near {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Near {}

/// One search's reusable buffers.
#[derive(Default)]
pub struct Buffers {
    page: Vec<u8>,
    vector: Vec<f32>,
    order: Vec<(f32, usize)>,
    heap: BinaryHeap<Near>,
}

impl Inner {
    /// A buffer set, kept or new.
    fn buffers(&self) -> Buffers {
        lock_builtin(&self.buffers).pop().unwrap_or_default()
    }

    /// Gives `buffers` back.
    fn give(&self, buffers: Buffers) {
        let mut kept = lock_builtin(&self.buffers);
        if kept.len() < POOLED {
            kept.push(buffers);
        }
    }

    /// The lists a search scans: its own count, else the index's, else
    /// one in eight; at least one, at most all.
    fn probes(&self, asked: Option<usize>, lists: usize) -> usize {
        let set = asked.unwrap_or_else(|| self.tuning.probes.load(Relaxed));
        let probes = if set == 0 { lists.div_ceil(AUTO_PROBE_SHARE) } else { set };
        probes.clamp(1, lists.max(1))
    }

    /// Orders `state`'s lists by their centroids' distance from `q`.
    fn order(&self, state: &Shape, q: &[f32], order: &mut Vec<(f32, usize)>) {
        order.clear();
        if state.centroids.is_empty() {
            order.push((0.0, 0));
            return;
        }
        order.extend(state.centroids.chunks_exact(self.dims).enumerate().map(|(at, c)| (self.metric.distance(q, c), at)));
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }

    /// Scans list `at` of `state`, giving `sink` each live entry's
    /// distance from `q` and row, of the rows `filter` takes; the live
    /// entries it held, and those taken.
    fn scan(
        &self,
        state: &Shape,
        (q, at): (Probe<'_>, usize),
        b: &mut Buffers,
        (filter, sink): (&mut Option<Filter<'_>>, &mut dyn FnMut(Near)),
    ) -> Result<(u64, u64)> {
        let Buffers { page, vector, .. } = b;
        vector.resize(self.dims, 0.0);
        let (mut held, mut taken) = (0_u64, 0_u64);
        let mut next = state.heads.get(at).copied();
        while let Some(id) = next {
            next = lists::read_page(&self.pool, self.file, id, page)?;
            for e in page.chunks_exact(state.layout.width) {
                let Some((row, _, _)) = lists::fields(e).filter(|f| f.1 > 0) else { continue };
                held = held.saturating_add(1);
                if !filter.as_mut().is_none_or(|f| f(row)) {
                    continue;
                }
                taken = taken.saturating_add(1);
                let stored = e.get(lists::ENTRY_HEAD..).unwrap_or_default();
                match &state.codec {
                    Some(codec) => codec.decode(stored, vector),
                    None => floats_of(stored, vector),
                }
                sink(Near { distance: self.metric.between(q, (vector, lists::norm(e))), row });
            }
        }
        Ok((held, taken))
    }
}

impl Ivf {
    /// The `k` nearest rows to `q` that `search.filter` takes, scanning
    /// `search.probes` lists, or the index's NPROBE, and more while fewer
    /// than k are found.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for a vector of the wrong length or a metric giving
    /// no finite distance; the buffer pool's; a poisoned lock.
    pub fn nearest(&self, q: &[f32], k: usize, search: Search<'_>) -> Result<Vec<(f32, u64)>> {
        let inner = &*self.inner;
        inner.metric.check(q, inner.dims)?;
        let Search { probes, mut filter, exact, .. } = search;
        // The shape is taken under the lock, and its lists read after
        // (R-ERR-4): the filter, codec and metric run with no lock held.
        let state = self.snapshot()?;
        let wanted = if state.codec.is_some() && inner.tuning.rerank.load(Relaxed) { k.saturating_mul(RERANK_FACTOR) } else { k };
        let probes = inner.probes(probes, state.heads.len());
        let mut b = inner.buffers();
        inner.order(&state, q, &mut b.order);
        let mut heap = std::mem::take(&mut b.heap);
        heap.clear();
        let order = std::mem::take(&mut b.order);
        let mut scanned = Ok(());
        // The entries the first `probes` lists held, and those the filter
        // has taken: past those lists, scanning goes on until it has taken
        // as many as an unfiltered search measures.
        let (mut quota, mut taken) = (0_u64, 0_u64);
        let probe = inner.metric.probe(q);
        for (n, (_, at)) in order.iter().enumerate() {
            if n >= probes && heap.len() >= wanted && taken >= quota {
                break;
            }
            match inner.scan(&state, (probe, *at), &mut b, (&mut filter, &mut |near| keep(&mut heap, near, wanted))) {
                Ok((held, took)) => {
                    quota = if n < probes { quota.saturating_add(held) } else { quota };
                    taken = taken.saturating_add(took);
                }
                Err(error) => {
                    scanned = Err(error);
                    break;
                }
            }
        }
        drop(state);
        let mut found: Vec<(f32, u64)> = heap.drain().map(|n| (n.distance, n.row)).collect();
        (b.heap, b.order) = (heap, order);
        inner.give(b);
        scanned?;
        let kept = if exact.is_some() { k } else { wanted };
        finish(&mut found, exact, kept);
        Ok(found)
    }

    /// Every row within `radius` of `q` that `search.filter` takes, of the
    /// NPROBE nearest lists: approximate, as any search of it is (R9.1).
    ///
    /// # Errors
    ///
    /// As `nearest`.
    pub fn range(&self, q: &[f32], radius: f32, search: Search<'_>) -> Result<Vec<(f32, u64)>> {
        let inner = &*self.inner;
        inner.metric.check(q, inner.dims)?;
        let Search { probes, mut filter, exact, .. } = search;
        let state = self.snapshot()?;
        let probes = inner.probes(probes, state.heads.len());
        let mut b = inner.buffers();
        inner.order(&state, q, &mut b.order);
        let order = std::mem::take(&mut b.order);
        let mut found = Vec::new();
        let mut scanned = Ok(());
        let probe = inner.metric.probe(q);
        for (_, at) in order.iter().take(probes) {
            scanned = inner
                .scan(
                    &state,
                    (probe, *at),
                    &mut b,
                    (&mut filter, &mut |near| found.extend((near.distance <= radius).then_some((near.distance, near.row)))),
                )
                .map(|_| ());
            if scanned.is_err() {
                break;
            }
        }
        drop(state);
        b.order = order;
        inner.give(b);
        scanned?;
        let rescored = exact.is_some();
        finish(&mut found, exact, usize::MAX);
        if rescored {
            found.retain(|(d, _)| *d <= radius);
        }
        Ok(found)
    }
}

/// Keeps `near` among the `k` nearest of `heap`.
fn keep(heap: &mut BinaryHeap<Near>, near: Near, k: usize) {
    if heap.len() < k {
        heap.push(near);
    } else if heap.peek().is_some_and(|far| near < *far) {
        heap.pop();
        heap.push(near);
    }
}

impl Ivf {
    /// The row of every live entry, once an entry, read a page at a time
    /// as the iterator is consumed. It holds the shape it began in, never
    /// the state's lock, so training meanwhile frees none of the pages it
    /// reads; it gives the entries of that shape, with appends to its
    /// lists made meanwhile, as far as it has not read past them. A
    /// failure, a poisoned lock or the buffer pool's, is its last item.
    pub fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        match self.snapshot() {
            Ok(shape) => {
                let next = shape.heads.first().copied();
                Box::new(Entries { inner: Arc::clone(&self.inner), shape, list: 0, next, page: Vec::new(), at: 0, done: false })
            }
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }
}

/// An iterator of the live entries' rows of one shape.
struct Entries {
    inner: Arc<Inner>,
    shape: Arc<Shape>,
    /// The list being read.
    list: usize,
    /// The page to read next, if any in this list.
    next: Option<PageId>,
    /// The entries of the page read.
    page: Vec<u8>,
    /// The next entry of the page read.
    at: usize,
    /// Whether it has ended, or failed.
    done: bool,
}

impl Entries {
    /// The next live entry's row in the page read, if any.
    fn in_page(&mut self) -> Option<u64> {
        let width = self.shape.layout.width;
        while let Some(e) = self.page.chunks_exact(width).nth(self.at) {
            self.at = self.at.saturating_add(1);
            if let Some((row, count, _)) = lists::fields(e)
                && count > 0
            {
                return Some(row);
            }
        }
        None
    }
}

impl Iterator for Entries {
    type Item = Result<u64>;

    fn next(&mut self) -> Option<Result<u64>> {
        while !self.done {
            if let Some(row) = self.in_page() {
                return Some(Ok(row));
            }
            let Some(page) = self.next else {
                self.list = self.list.saturating_add(1);
                self.next = self.shape.heads.get(self.list).copied();
                self.done = self.next.is_none();
                continue;
            };
            self.at = 0;
            match lists::read_page(&self.inner.pool, self.inner.file, page, &mut self.page) {
                Ok(next) => self.next = next,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
        None
    }
}
