//! \[DESIGN\] Opening an IVF index from its pages, saving its directory at
//! a checkpoint, and freeing its pages when it is dropped. Opening reads
//! the centroids, the directory, and every list once, to count its
//! entries and map each to its page; a list's last page is the end of its
//! links, whatever the directory last recorded, so appends a checkpoint
//! captured after the directory was written are found.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::lists::{self, Layout};
use super::{Config, F32_BYTES, Ivf, LOCATE_BYTES, Parts, Shape, State, place_key, read_centres, read_meta};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::hnsw::{ChainReader, Metric, NO_PAGE, Quantizer, free_pages, grow};
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::sync::read;

impl Ivf {
    /// The index whose meta page is `meta`. Its options that change in
    /// place come from `tuning`; the rest from its pages.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 for malformed pages, or a metric or quantizer not the
    /// one it was made with; -918 if the budget refuses its map; the
    /// buffer pool's.
    pub fn open(
        pool: Arc<BufferPool>,
        (file, meta): (FileId, PageId),
        tuning: &Config,
        (metric, quantizer): (Metric, Option<Arc<dyn Quantizer>>),
        budget: &Arc<MemoryBudget>,
    ) -> Result<Ivf> {
        let m = read_meta(&pool.read(file, meta.0)?)?;
        if m.metric != metric.name() || m.quantizer != quantizer.as_ref().map(|q| q.name()).unwrap_or_default() {
            return Err(super::corrupt("its metric or quantizer is not the one it was made with"));
        }
        let (centroids, codec) = read_centres(&pool, file, m.centres, (m.dims, quantizer.as_ref()))?;
        let trained = centroids.len().checked_div(m.dims).unwrap_or(0);
        if (trained != 0 || m.lists != 1) && trained != m.lists {
            return Err(super::corrupt("its centroids and lists differ in count"));
        }
        let payload = codec.as_ref().map_or(m.dims.saturating_mul(F32_BYTES), |c| c.code_bytes());
        let layout = Layout::new(&pool, file, payload)?;
        let (lists, directory) = lists::read_directory(&pool, file, m.directory, m.lists)?;
        let mut shape = Shape::new(centroids, codec, layout);
        shape.heads = lists.iter().map(|l| l.head).collect();
        let mut state = State {
            shape: Arc::new(shape),
            lists,
            directory,
            centres: m.centres,
            trained_on: m.trained_on,
            target: m.lists,
            live: 0,
            dead: 0,
            locate: HashMap::new(),
            collided: HashSet::new(),
            changes: 0,
        };
        mapped(&pool, file, &mut state)?;
        let mut memory = (budget.reserve(0, "an IVF index's centroids and map")?, 0);
        grow(&mut memory, used(&state))?;
        let config = Config { dims: m.dims, seed: m.seed, ..*tuning };
        Ok(Ivf::with(Parts { pool, file, meta, config, metric, quantizer, state, memory, budget: Arc::clone(budget) }))
    }

    /// Writes its directory's records over its pages: each list's last
    /// page and counts, as they are now.
    ///
    /// # Errors
    ///
    /// The buffer pool's; a poisoned lock.
    pub fn save(&self) -> Result<()> {
        let inner = &*self.inner;
        let state = read(&inner.state)?;
        lists::rewrite_directory(&inner.pool, inner.file, (&state.shape.layout, &state.directory), &state.lists)
    }

    /// Frees every page it holds: for an index dropped, which nothing
    /// reads again.
    ///
    /// # Errors
    ///
    /// The buffer pool's; some pages may be freed already.
    pub fn free(&self) -> Result<()> {
        let inner = &*self.inner;
        let state = read(&inner.state)?;
        free_state(&inner.pool, inner.file, &state)?;
        inner.pool.free(inner.file, inner.meta)
    }
}

/// Frees the pages of `state`'s lists, directory and centroids, if it
/// has been given them.
///
/// # Errors
///
/// The buffer pool's; some pages may be freed already.
pub fn free_state(pool: &BufferPool, file: FileId, state: &State) -> Result<()> {
    let heads: Vec<PageId> = state.lists.iter().map(|l| l.head).collect();
    free_pages_of(pool, file, (&heads, &state.directory, state.centres))
}

/// Frees the pages of the lists whose first pages are `heads`, the
/// directory's pages, and the centroids' chain from `centres`, if any.
///
/// # Errors
///
/// The buffer pool's; some pages may be freed already.
pub fn free_pages_of(pool: &BufferPool, file: FileId, (heads, directory, centres): (&[PageId], &[PageId], PageId)) -> Result<()> {
    for head in heads {
        free_pages(pool, file, &lists::pages_of(pool, file, *head)?)?;
    }
    free_pages(pool, file, directory)?;
    if centres.0 == NO_PAGE {
        return Ok(());
    }
    free_pages(pool, file, &ChainReader::new(pool, file, centres)?.pages()?)
}

/// The bytes `state` holds in memory: centroids, directory, map.
pub fn used(state: &State) -> usize {
    let map = state.locate.len().saturating_add(state.collided.len()).saturating_mul(LOCATE_BYTES);
    let lists = state.lists.len().saturating_mul(size_of::<lists::List>());
    map.saturating_add(lists).saturating_add(size_of_val(state.shape.centroids.as_slice()))
}

/// Reads every list of `state`: its last page, its counts, and each of
/// its entries' places in the map.
fn mapped(pool: &BufferPool, file: FileId, state: &mut State) -> Result<()> {
    let mut buf = Vec::new();
    for at in 0..state.lists.len() {
        let Some(head) = state.lists.get(at).map(|l| l.head) else { continue };
        let (mut live, mut dead, mut last) = (0_u64, 0_u64, head);
        let mut next = Some(head);
        while let Some(page) = next {
            next = lists::read_page(pool, file, page, &mut buf)?;
            last = page;
            for e in buf.chunks_exact(state.shape.layout.width) {
                let Some((row, count, hash)) = lists::fields(e) else { continue };
                if count > 0 {
                    live = live.saturating_add(1);
                } else {
                    dead = dead.saturating_add(1);
                }
                state.located(place_key(row, hash), at, page);
            }
        }
        if let Some(list) = state.lists.get_mut(at) {
            (list.tail, list.live, list.dead) = (last, live, dead);
        }
        state.live = state.live.saturating_add(live);
        state.dead = state.dead.saturating_add(dead);
    }
    Ok(())
}
