//! \[DESIGN\] Training an IVF index (vector design document §10.4, §10.6,
//! R7.2, R10.10): a sample of its live vectors drawn, seeded k-means run,
//! its lists written anew from every live entry, its tombstones left
//! behind, all with no lock held, as the metric and codec run (R-ERR-4);
//! then, holding the index, the new state put in place if no change came
//! meanwhile, and the meta page naming it written. A bulk load trains at once on the entries given.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::lists::{self, Layout, List};
use super::open::{free_state, used};
use super::write::{Found, floats_of};
use super::{
    Config, F32_BYTES, Inner, Ivf, MAX_LISTS, MIN_LISTS, RETRAIN_GROWTH, Retired, Shape, State, TRAIN_PER_LIST, kmeans, meta_bytes,
    place_key, write_centres,
};
use crate::disk::pager::PageId;
use crate::engine::hnsw::random::{Rng, vector_hash};
use crate::engine::hnsw::{Codec, NO_PAGE, SAMPLE_MOST, grow, sample};
use crate::error::{Code, Result, SqlError};
use crate::sync::{lock, lock_builtin, read, write};

/// \[DESIGN\] Tombstones are dropped past one entry in five (R10.10's 20%).
const DEAD_SHARE: u64 = 5;

/// A training sample, the lists to train, and the codec now.
type Sampled = (Vec<f32>, usize, Option<Arc<dyn Codec>>);

/// A rebuild's buffers: an entry's stored bytes, the vector they stand
/// for, a decoded vector, a page.
struct Carry {
    payload: Vec<u8>,
    stored: Vec<f32>,
    v: Vec<f32>,
    buf: Vec<u8>,
}

/// Where a rebuild's entries come from.
enum Source<'a> {
    /// The live entries of an index's lists.
    Lists(&'a Shape),
    /// Vectors, `dims` each, and their rows.
    Flat(&'a [f32], &'a [u64]),
}

impl Inner {
    /// The lists an index of `live` rows trains: as set, else sqrt(N),
    /// at least `MIN_LISTS`.
    fn target_lists(&self, live: u64) -> usize {
        let set = self.tuning.lists.load(Relaxed);
        let auto = usize::try_from(live).unwrap_or(usize::MAX).isqrt().max(MIN_LISTS);
        if set == 0 { auto } else { set }.min(MAX_LISTS)
    }

    /// The configuration its meta page keeps.
    fn config(&self) -> Config {
        Config { dims: self.dims, lists: 0, probes: 0, sample_per_list: 1, seed: self.seed, rerank: false }
    }
}

impl Ivf {
    /// Whether maintenance is due: enough rows to train, growth past
    /// fourfold, tombstones past a fifth, or a changed count of lists.
    pub fn maintenance_due(&self) -> bool {
        let inner = &*self.inner;
        let Ok(state) = read(&inner.state) else { return false };
        let rows = u64::try_from(MIN_LISTS.saturating_mul(TRAIN_PER_LIST)).unwrap_or(u64::MAX);
        let untrained = state.trained_on == 0 && state.live >= rows;
        let grown = state.trained_on > 0 && state.live >= state.trained_on.saturating_mul(RETRAIN_GROWTH);
        let dead = state.dead > 0 && state.dead.saturating_mul(DEAD_SHARE) >= state.live.saturating_add(state.dead);
        let set = inner.tuning.lists.load(Relaxed);
        let relisted = state.trained_on > 0 && set != 0 && set != state.target;
        untrained || grown || dead || relisted
    }

    /// Does the maintenance due, and whether it did any.
    ///
    /// # Errors
    ///
    /// The buffer pool's; the quantizer's; -918 if the budget refuses the
    /// sample; the index stands as it was then.
    pub fn maintain(&self) -> Result<bool> {
        if !self.maintenance_due() {
            return Ok(false);
        }
        let inner = &*self.inner;
        let (old, changes, counts) = {
            let state = read(&inner.state)?;
            (Arc::clone(&state.shape), state.changes, (state.live, state.trained_on))
        };
        let (flat, target, codec) = self.sampled(&old, counts)?;
        let dims = inner.dims;
        let lists = target.min(flat.len().checked_div(dims.saturating_mul(TRAIN_PER_LIST)).unwrap_or(0)).max(1);
        let centroids = if lists > 1 { kmeans::train(&inner.metric, &flat, (dims, lists), inner.seed) } else { Vec::new() };
        let codec = match (codec, inner.quantizer.as_ref().filter(|_| !flat.is_empty())) {
            (Some(codec), _) => Some(codec),
            (None, Some(quantizer)) => Some(quantizer.train(&flat, dims, inner.seed)?),
            (None, None) => None,
        };
        drop(flat);
        // Built with no lock held, as the metric and codec run (R-ERR-4),
        // from the lists of the shape sampled, whose pages it keeps.
        let mut built = self.rebuild((centroids, codec), &Source::Lists(&old))?;
        built.target = target;
        let mut state = write(&inner.state)?;
        if state.changes != changes || !Arc::ptr_eq(&state.shape, &old) {
            drop(state);
            // Changed meanwhile: the new lists are let go, never named by
            // the meta page, and maintenance stays due for the next try. A
            // failed free leaks pages only.
            let _ = free_state(&inner.pool, inner.file, &built);
            return Ok(false);
        }
        self.swap(&mut state, built)?;
        Ok(true)
    }

    /// A sample of the live vectors, as stored, at most the target lists'
    /// sample and `SAMPLE_MOST`, by reservoir sampling seeded; the lists
    /// to train; the codec now. Read from `state`'s lists, of an index of
    /// `live` entries trained on `trained_on`, with no lock held.
    fn sampled(&self, state: &Shape, (live, trained_on): (u64, u64)) -> Result<Sampled> {
        let inner = &*self.inner;
        let target = inner.target_lists(live);
        let most = target.saturating_mul(inner.tuning.sample_per_list.load(Relaxed)).min(SAMPLE_MOST);
        let _held = inner.budget.reserve(most.saturating_mul(inner.dims).saturating_mul(F32_BYTES), "an IVF index's training sample")?;
        let mut rng = Rng::new(inner.seed ^ trained_on);
        let (mut flat, mut seen, mut buf, mut v) = (Vec::new(), 0_usize, Vec::new(), vec![0.0_f32; inner.dims]);
        let dims = inner.dims;
        for head in &state.heads {
            lists::walk(&inner.pool, inner.file, (*head, state.layout.width), &mut buf, &mut |_, _, e| {
                if lists::fields(e).is_none_or(|f| f.1 == 0) {
                    return Ok(());
                }
                decoded(state, e.get(lists::ENTRY_HEAD..).unwrap_or_default(), &mut v);
                seen = seen.saturating_add(1);
                if flat.len() < most.saturating_mul(dims) {
                    flat.extend_from_slice(&v);
                    return Ok(());
                }
                // Reservoir sampling (Vitter's Algorithm R): the n-th
                // vector replaces one kept with probability most / n.
                let j = rng.below(seen);
                if j < most {
                    let slot = flat.get_mut(j.saturating_mul(dims)..j.saturating_add(1).saturating_mul(dims)).unwrap_or_default();
                    slot.iter_mut().zip(&v).for_each(|(s, x)| *s = *x);
                }
                Ok(())
            })?;
        }
        Ok((flat, target, state.codec.clone()))
    }

    /// A new state of `centroids`' lists, coded by `codec`, holding the
    /// entries of `source`; its pages freed if it fails.
    fn rebuild(&self, (centroids, codec): (Vec<f32>, Option<Arc<dyn Codec>>), source: &Source<'_>) -> Result<State> {
        let inner = &*self.inner;
        let (pool, file) = (&*inner.pool, inner.file);
        let layout = Layout::new(pool, file, codec.as_ref().map_or(inner.dims.saturating_mul(F32_BYTES), |c| c.code_bytes()))?;
        let mut state = State {
            shape: Arc::new(Shape::new(centroids, codec, layout)),
            lists: Vec::new(),
            directory: Vec::new(),
            centres: PageId(NO_PAGE),
            trained_on: 0,
            target: 1,
            live: 0,
            dead: 0,
            locate: HashMap::default(),
            collided: HashSet::default(),
            changes: 0,
        };
        let filled = (|| {
            state.centres = write_centres(pool, file, &state.shape.centroids, state.shape.codec.as_ref())?;
            for _ in 0..state.shape.centroids.len().checked_div(inner.dims).unwrap_or(0).max(1) {
                state.lists.push(lists::new_list(pool, file)?);
            }
            // Its lists' first pages are fixed now; nothing shares the
            // shape yet, so it is changed in place.
            if let Some(shape) = Arc::get_mut(&mut state.shape) {
                shape.heads = state.lists.iter().map(|l| l.head).collect();
            }
            state.directory = lists::write_directory(pool, file, &state.shape.layout, &state.lists)?;
            self.fill(&mut state, source)?;
            state.trained_on = if state.shape.centroids.is_empty() { 0 } else { state.live };
            lists::rewrite_directory(pool, file, (&state.shape.layout, &state.directory), &state.lists)
        })();
        if let Err(error) = filled {
            // Nothing refers to the new pages; a failed free leaks pages only.
            let _ = free_state(pool, file, &state);
            return Err(error);
        }
        Ok(state)
    }

    /// Adds `source`'s entries to `state`, new.
    fn fill(&self, state: &mut State, source: &Source<'_>) -> Result<()> {
        let inner = &*self.inner;
        let mut c = Carry { payload: Vec::new(), stored: Vec::new(), v: vec![0.0_f32; inner.dims], buf: Vec::new() };
        let mut page = Vec::new();
        match source {
            Source::Flat(flat, rows) => {
                for (vector, row) in flat.chunks_exact(inner.dims).zip(rows.iter()) {
                    state.shape.payload(vector, (&mut c.payload, &mut c.stored));
                    self.add(state, (*row, 1, vector_hash(vector)), (&c.payload, &c.stored), &mut c.buf)?;
                }
            }
            Source::Lists(from) => {
                let same = codecs_match(from.codec.as_ref(), state.shape.codec.as_ref());
                for head in &from.heads {
                    lists::walk(&inner.pool, inner.file, (*head, from.layout.width), &mut page, &mut |_, _, e| {
                        self.carry((state, from, same), e, &mut c)
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Adds the live entry `e` of `old`'s lists to `state`: its stored
    /// bytes kept if the two store alike (`same`), else its vector stored
    /// again by `state`'s codec.
    fn carry(&self, (state, old, same): (&mut State, &Shape, bool), e: &[u8], c: &mut Carry) -> Result<()> {
        let Some((row, count, hash)) = lists::fields(e).filter(|f| f.1 > 0) else { return Ok(()) };
        let bytes = e.get(lists::ENTRY_HEAD..).unwrap_or_default();
        if same {
            c.payload.clear();
            c.payload.extend_from_slice(bytes);
            decoded(&state.shape, bytes, &mut c.v);
            c.stored.clear();
            c.stored.extend_from_slice(&c.v);
        } else {
            decoded(old, bytes, &mut c.v);
            state.shape.payload(&c.v, (&mut c.payload, &mut c.stored));
        }
        self.add(state, (row, count, hash), (&c.payload, &c.stored), &mut c.buf)
    }

    /// Adds an entry, counted `count`, to `state`, new: counted on if it
    /// holds it, as a load of one pair twice does.
    fn add(
        &self,
        state: &mut State,
        (row, count, hash): (u64, u32, u64),
        (payload, stored): (&[u8], &[f32]),
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let inner = &*self.inner;
        let pages = (&*inner.pool, inner.file);
        let at = state.shape.assign(&inner.metric, stored);
        if let Some(Found { page, slot, .. }) = state.find(pages, (row, hash, payload), at, buf)? {
            for _ in 0..count {
                lists::count(pages.0, pages.1, (&state.shape.layout, page, slot), true)?;
            }
            return Ok(());
        }
        let mut entry = Vec::with_capacity(state.shape.layout.width);
        lists::entry(&mut entry, (row, count, hash), payload);
        let mut list: List = *state.lists.get(at).ok_or_else(|| super::corrupt("a list is missing"))?;
        let page = lists::append(pages.0, pages.1, (&state.shape.layout, &mut list), &entry)?;
        list.live = list.live.saturating_add(1);
        if let Some(slot) = state.lists.get_mut(at) {
            *slot = list;
        }
        state.live = state.live.saturating_add(1);
        state.located(place_key(row, hash), at, page);
        Ok(())
    }

    /// Makes `built` the index's state: its memory reserved, the meta page
    /// naming it written, then the old state's pages freed.
    fn swap(&self, state: &mut State, built: State) -> Result<()> {
        let inner = &*self.inner;
        let (pool, file) = (&*inner.pool, inner.file);
        let config = inner.config();
        let names = (inner.metric.name(), inner.quantizer.as_ref().map(|q| q.name()));
        let published = (|| {
            let mut memory = lock(&inner.memory)?;
            let before = memory.1;
            grow(&mut memory, used(&built))?;
            let written = pool.write(file, inner.meta).map(|mut page| *page = meta_bytes(&config, names, &built));
            memory.1 = if written.is_ok() { used(&built) } else { before };
            written
        })();
        if let Err(error) = published {
            // As in `rebuild`: the meta page still names the old state.
            let _ = free_state(pool, file, &built);
            return Err(error);
        }
        let old = std::mem::replace(state, built);
        // The meta page names the new state. The old one's pages are freed
        // once no search reads them: when the last holder of its shape,
        // maybe this, lets it go.
        let retired = Retired { pool: Arc::clone(&inner.pool), file, directory: old.directory.clone(), centres: old.centres };
        *lock_builtin(&old.shape.retired) = Some(retired);
        drop(old);
        Ok(())
    }

    /// Adds every entry of `entries` to the index, empty, which nothing
    /// else reads or writes meanwhile: trained on them at once if they are
    /// enough, else one list.
    ///
    /// # Errors
    ///
    /// As `insert`, and `maintain`; the entries' own.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        let inner = &*self.inner;
        let mut held = inner.budget.reserve(0, "an IVF index's load")?;
        let (mut flat, mut rows) = (Vec::new(), Vec::new());
        for entry in entries {
            let (vector, row) = entry?;
            inner.metric.check(&vector, inner.dims)?;
            held.resize(held.bytes().saturating_add(size_of_val(vector.as_slice())))?;
            flat.extend_from_slice(&vector);
            rows.push(row);
        }
        let target = inner.target_lists(u64::try_from(rows.len()).unwrap_or(u64::MAX));
        if rows.len() < MIN_LISTS.saturating_mul(TRAIN_PER_LIST) {
            return flat.chunks_exact(inner.dims).zip(rows).try_for_each(|(v, row)| self.insert(v, row));
        }
        let picked = sample(&flat, inner.dims, inner.seed);
        let lists = target.min(rows.len().checked_div(TRAIN_PER_LIST).unwrap_or(1)).max(1);
        let centroids = kmeans::train(&inner.metric, &picked, (inner.dims, lists), inner.seed);
        let codec = inner.quantizer.as_ref().map(|q| q.train(&picked, inner.dims, inner.seed)).transpose()?;
        drop(picked);
        let changes = read(&inner.state)?.changes;
        // Built with no lock held (R-ERR-4), then put in place.
        let mut built = self.rebuild((centroids, codec), &Source::Flat(&flat, &rows))?;
        built.target = target;
        let mut state = write(&inner.state)?;
        if state.changes != changes {
            drop(state);
            // Unpublished; a failed free leaks pages only.
            let _ = free_state(&inner.pool, inner.file, &built);
            return Err(SqlError::new(Code::Api, "[DESIGN] an IVF index was written to during its load"));
        }
        self.swap(&mut state, built)
    }
}

/// The vector an entry's stored bytes stand for in `state`, into `out`.
fn decoded(state: &Shape, bytes: &[u8], out: &mut [f32]) {
    match &state.codec {
        Some(codec) => codec.decode(bytes, out),
        None => floats_of(bytes, out),
    }
}

/// Whether two states store entries alike: neither coded, or one codec.
fn codecs_match(a: Option<&Arc<dyn Codec>>, b: Option<&Arc<dyn Codec>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (Some(_), None) | (None, Some(_)) => false,
    }
}
