//! \[DESIGN\] BeamSearch [DiskANN, NeurIPS 2019 §3.3]: "we fetch the
//! neighborhoods of a small number, W (say 4, 8), of the closest points
//! in L \ V in one shot, and update L to be the top L candidates in L
//! along with all the neighbors retrieved in this step." The W pages are
//! asked of the buffer pool together (`BufferPool::ahead_pages`), read by
//! the database's read-ahead job where one serves the pool. Candidates are
//! ordered by the compressed codes' distances [§3.1], or by full vectors
//! until the index has codes; each node expanded brings its full vector
//! with its edges, and the result is re-ranked by those [§3.5].
//!
//! Beyond the paper are a row filter, searched in-filter (filtered nodes are
//! traversed, only accepted ones returned, the list doubled while fewer
//! than k are found); and a radius, every node within it returned, the
//! list doubled while at least half of it was within the radius. The list
//! is also grown by the share of tombstoned nodes, which fill places in
//! it but are never returned.

use std::sync::atomic::Ordering;

use crate::disk::buffer::Ahead;
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::{lock_builtin, read};

use super::frontier::{Entry, Frontier, Visited};
use super::meta::NO_NODE;
use super::node::{Node, corrupt};
use super::{CodeTable, DiskAnn, Probe, finite};
use crate::vector::kernels::squared_norm;

/// A percentage's whole.
const PERCENT: usize = 100;
/// Doublings of the list a filtered or range search makes at most.
const MOST_ROUNDS: u32 = 16;

/// \[EXTENSION\] What a search asks for.
#[derive(Clone, Copy, Default)]
pub struct Search<'a> {
    /// k, the results wanted; ignored by a range search, which returns
    /// all it finds.
    pub results: usize,
    /// L, the candidate list; the tuning's if none, at least k.
    pub list: Option<u32>,
    /// W, the beam width; the tuning's if none.
    pub width: Option<u32>,
    /// Only rows it accepts are returned (in-filter search). It is called
    /// with no lock held, and must not call back into the index.
    pub filter: Option<&'a (dyn Fn(u64) -> bool + Sync)>,
    /// The percentage of rows the filter is expected to accept, if known:
    /// the list starts k × 100 / it long, so a selective filter still
    /// finds k.
    pub selectivity: Option<u32>,
    /// Every node within this distance, instead of the k nearest.
    pub radius: Option<f64>,
}

/// \[EXTENSION\] A result: a node, its row and full-precision distance.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The full-precision distance from the query.
    pub distance: f32,
    /// The row id.
    pub row: u64,
    /// The node.
    pub node: u32,
    /// The times (vector, row) was inserted more than removed.
    pub count: u32,
    /// The vector as stored.
    pub vector: Vec<f32>,
}

/// \[EXTENSION\] What a search did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Rounds of W reads [§4.2: "the number of times the search frontier
    /// is expanded by making W parallel disk reads"], over every round.
    pub hops: u32,
    /// Nodes expanded.
    pub expanded: u32,
    /// Nodes read from pages for expansion.
    pub reads: u32,
    /// Nodes found in the hot-node cache.
    pub cached: u32,
    /// Searches made, more than one when the list was doubled.
    pub rounds: u32,
    /// The last list size.
    pub list: u32,
}

/// A node expanded, with its full distance.
pub(super) struct Expanded {
    pub(super) distance: f32,
    pub(super) id: u32,
    pub(super) node: Node,
}

/// How candidates are ordered: by one query's code distances, and the
/// bytes of a code, or by full vectors.
type Guide<'a> = Option<(usize, Box<dyn CodeTable + 'a>)>;

impl DiskAnn {
    /// The nearest `search.results` nodes to `query` (or all within its
    /// radius), re-ranked, nearest first, with what the search did.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for a distance that is not finite, or a query of other
    /// dimensions; -918 if its visited array cannot be reserved; the
    /// buffer pool's.
    pub fn search(&self, query: &[f32], search: &Search<'_>) -> Result<(Vec<Candidate>, SearchStats)> {
        self.check_dims(query)?;
        let tuning = self.tuning()?;
        let k = search.results.max(1);
        let mut list = usize::try_from(search.list.unwrap_or(tuning.search_list)).unwrap_or(usize::MAX).max(k);
        if let Some(share) = search.selectivity.filter(|&s| s > 0) {
            list = list.max(k.saturating_mul(PERCENT).checked_div(usize::try_from(share).unwrap_or(1)).unwrap_or(list));
        }
        let width = usize::try_from(search.width.unwrap_or(tuning.beam_width)).unwrap_or(1).max(1);
        let total = usize::try_from(lock_builtin(&self.inner.alloc).next).unwrap_or(usize::MAX);
        list = self.past_dead(list);
        let mut stats = SearchStats::default();
        loop {
            let expanded = self.walk(query, (list, width), &mut stats)?;
            let (found, more) = select(expanded, search, k, list);
            if !more || list >= total || stats.rounds >= MOST_ROUNDS {
                self.inner.searches.fetch_add(1, Ordering::Relaxed);
                self.inner.hops.fetch_add(u64::from(stats.hops), Ordering::Relaxed);
                return Ok((found, stats));
            }
            list = list.saturating_mul(2).min(total);
        }
    }

    /// `list` grown by the share of dead nodes, which take places in the
    /// list but are never returned (FreshDiskANN keeps them traversable).
    fn past_dead(&self, list: usize) -> usize {
        let live = self.inner.live.load(Ordering::Acquire);
        let all = live.saturating_add(self.inner.dead.load(Ordering::Acquire));
        let grown = u64::try_from(list).unwrap_or(u64::MAX).saturating_mul(all).checked_div(live).unwrap_or(0);
        usize::try_from(grown).unwrap_or(usize::MAX).max(list)
    }

    /// SQLCODE -910 for a vector of other dimensions.
    pub(super) fn check_dims(&self, vector: &[f32]) -> Result<()> {
        if vector.len() != self.inner.shape.dims {
            return Err(crate::error::SqlError::new(
                crate::error::Code::Api,
                format!("[EXTENSION] a DiskANN index of {} dimensions was given a vector of {}", self.inner.shape.dims, vector.len()),
            ));
        }
        Ok(())
    }

    /// One BeamSearch from the medoid with list `list` and width `width`:
    /// every node expanded, by its full distance, nearest first.
    pub(super) fn walk(&self, query: &[f32], (list, width): (usize, usize), stats: &mut SearchStats) -> Result<Vec<Expanded>> {
        let mut visited = lock_builtin(&self.inner.visits).pop().unwrap_or_else(Visited::new);
        let result = self.walk_with(query, (list, width), stats, &mut visited);
        lock_builtin(&self.inner.visits).push(visited);
        let mut expanded = result?;
        expanded.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.id.cmp(&b.id)));
        Ok(expanded)
    }

    fn walk_with(
        &self,
        query: &[f32],
        (list, width): (usize, usize),
        stats: &mut SearchStats,
        visited: &mut Visited,
    ) -> Result<Vec<Expanded>> {
        stats.rounds = stats.rounds.saturating_add(1);
        stats.list = u32::try_from(list).unwrap_or(u32::MAX);
        let start = self.inner.medoid.load(Ordering::Acquire);
        let total = usize::try_from(lock_builtin(&self.inner.alloc).next).unwrap_or(0);
        let mut expanded = Vec::new();
        if start == NO_NODE {
            return Ok(expanded);
        }
        visited.start(total, Some(&self.inner.budget))?;
        let codes = read(&self.inner.book)?.codes.clone();
        let guide: Guide<'_> = codes.as_deref().map(|c| (c.code_bytes(), c.table(query)));
        let query = (query, squared_norm(query));
        let mut frontier = Frontier::new(list);
        let (mut batch, mut fresh, mut distances) = (Vec::new(), Vec::new(), Vec::new());
        visited.visit(start);
        self.guided(&guide, query, &[start], &mut distances)?;
        frontier.add(distances.first().copied().unwrap_or(f32::INFINITY), start);
        let mut ahead = Ahead::default();
        loop {
            frontier.take(width, &mut batch);
            if batch.is_empty() {
                return Ok(expanded);
            }
            stats.hops = stats.hops.saturating_add(1);
            fresh.clear();
            for node in self.fetch(&batch, &mut ahead, stats)? {
                let (id, node) = node;
                if node.free() {
                    continue;
                }
                fresh.extend(node.neighbours.iter().copied().filter(|&n| visited.visit(n)));
                let distance = finite(self.inner.plugins.metric.normed(query, (&node.vector, node.norm)), "metric")?;
                frontier.settle(id, distance);
                expanded.push(Expanded { distance, id, node });
            }
            stats.expanded = stats.expanded.saturating_add(u32::try_from(batch.len()).unwrap_or(0));
            self.guided(&guide, query, &fresh, &mut distances)?;
            fresh.iter().zip(&distances).for_each(|(&n, &d)| frontier.add(d, n));
        }
    }

    /// The guiding distances of `ids` from `query`, into `out`.
    fn guided(&self, guide: &Guide<'_>, query: Probe<'_>, ids: &[u32], out: &mut Vec<f32>) -> Result<()> {
        out.clear();
        let Some((width, table)) = guide else {
            out.resize(ids.len(), 0.0_f32);
            let all: Vec<usize> = (0..ids.len()).collect();
            return self.full_distances(query, ids, &all, out);
        };
        let width = *width;
        let mut copied = Vec::with_capacity(ids.len().saturating_mul(width));
        let mut missing = Vec::new();
        self.copy_codes(ids, width, &mut copied, &mut missing)?;
        for code in copied.chunks_exact(width.max(1)) {
            out.push(finite(table.distance(code), "quantizer")?);
        }
        self.full_distances(query, ids, &missing, out)
    }

    /// The codes of `ids` into `copied`, under the codes' lock only while
    /// copying, so no plug-in is called under it (R-ERR-4); zeros, and
    /// their positions in `missing`, for nodes with no code yet.
    fn copy_codes(&self, ids: &[u32], width: usize, copied: &mut Vec<u8>, missing: &mut Vec<usize>) -> Result<()> {
        let book = read(&self.inner.book)?;
        for (at, &id) in ids.iter().enumerate() {
            let Some(code) = book.code(id).filter(|c| c.len() == width) else {
                copied.resize(copied.len().saturating_add(width), 0);
                missing.push(at);
                continue;
            };
            copied.extend_from_slice(code);
        }
        Ok(())
    }

    /// Full distances of `ids` at positions `at` into `out`, reading them.
    fn full_distances(&self, query: Probe<'_>, ids: &[u32], at: &[usize], out: &mut [f32]) -> Result<()> {
        let mut node = Node::default();
        for &position in at {
            let id = ids.get(position).copied().ok_or_else(|| corrupt("a guided node is missing"))?;
            self.cached_or_read(id, &mut node)?;
            if let Some(slot) = out.get_mut(position) {
                *slot = if node.free() {
                    f32::INFINITY
                } else {
                    finite(self.inner.plugins.metric.normed(query, (&node.vector, node.norm)), "metric")?
                };
            }
        }
        Ok(())
    }

    /// Node `id` from the cache, else its page.
    fn cached_or_read(&self, id: u32, node: &mut Node) -> Result<bool> {
        if let Some(record) = read(&self.inner.cache)?.get(id) {
            self.inner.shape.decode(record, node)?;
            return Ok(true);
        }
        self.read_node(id, node)?;
        Ok(false)
    }

    /// The nodes of `batch`: cached ones at once, the rest asked of the
    /// pool together, then read.
    fn fetch(&self, batch: &[Entry], ahead: &mut Ahead, stats: &mut SearchStats) -> Result<Vec<(u32, Node)>> {
        let mut out = Vec::with_capacity(batch.len());
        let mut pages = Vec::new();
        {
            let cache = read(&self.inner.cache)?;
            for entry in batch {
                let mut node = Node::default();
                if let Some(record) = cache.get(entry.id) {
                    self.inner.shape.decode(record, &mut node)?;
                    out.push((entry.id, node));
                    stats.cached = stats.cached.saturating_add(1);
                } else {
                    pages.push(entry.id);
                }
            }
        }
        if pages.len() > 1 {
            let ids: Vec<u64> = pages.iter().map(|&id| self.locate(id).map(|(p, _)| p)).collect::<Result<_>>()?;
            self.inner.pool.ahead_pages(ahead, self.inner.file, &ids);
        }
        for id in pages {
            let mut node = Node::default();
            self.read_node(id, &mut node)?;
            stats.reads = stats.reads.saturating_add(1);
            out.push((id, node));
        }
        Ok(out)
    }

    /// The page of node `id`, for callers that read ahead.
    pub(super) fn page_of(&self, id: u32) -> Result<PageId> {
        self.locate(id).map(|(p, _)| PageId(p))
    }
}

/// The results of one walk, and whether a longer list is needed: a
/// search that found fewer than k live and accepted nodes, or a range
/// search with at least half its list within its radius, so more may be.
fn select(expanded: Vec<Expanded>, search: &Search<'_>, k: usize, list: usize) -> (Vec<Candidate>, bool) {
    let mut found = Vec::new();
    let within = |e: &Expanded| search.radius.is_none_or(|r| f64::from(e.distance) <= r);
    let inside = expanded.iter().filter(|e| within(e)).count();
    for e in expanded {
        if (search.radius.is_none() && found.len() >= k) || !within(&e) {
            break;
        }
        let accepted = e.node.live() && search.filter.is_none_or(|f| f(e.node.row));
        if accepted {
            found.push(Candidate { distance: e.distance, row: e.node.row, node: e.id, count: e.node.count, vector: e.node.vector });
        }
    }
    let more = match search.radius {
        Some(_) => inside.saturating_mul(2) >= list,
        None => found.len() < k,
    };
    (found, more)
}
