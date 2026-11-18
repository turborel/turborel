//! \[DESIGN\] IVF changes, written through to pages (vector design
//! document R14.5: "IVF assigns to nearest centroid"). An insert of an
//! entry held counts it once more in place; else it is appended to the
//! list of the centroid nearest its stored vector. A remove counts it
//! once less, a tombstone at none. Each change is one page write, made
//! before memory changes, after memory is reserved, so a failed change
//! makes none (RULES.md D19). An entry's stored bytes and list are worked
//! out by the codec and metric before the lock is taken (R-ERR-4).

use std::sync::Arc;

use super::lists::{self, List};
use super::{F32_BYTES, Ivf, LOCATE_BYTES, Shape, State, contended, kmeans, place, place_key, unplace};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::hnsw::random::vector_hash;
use crate::engine::hnsw::{Metric, TRIES, grow};
use crate::error::Result;
use crate::sync::{lock, read, write};
use crate::vector::kernels::squared_norm;

/// Where an entry is: its list, page, slot, and count.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub list: usize,
    pub page: PageId,
    pub slot: usize,
    pub count: u32,
}

/// An entry looked for: its row, vector hash and stored bytes.
pub type Wanted<'a> = (u64, u64, &'a [u8]);

impl Shape {
    /// What an entry of `vector` stores, in `payload`: its floats' bytes,
    /// or its code; and in `stored` the vector those stand for.
    pub fn payload(&self, vector: &[f32], (payload, stored): (&mut Vec<u8>, &mut Vec<f32>)) {
        payload.clear();
        stored.clear();
        let Some(codec) = &self.codec else {
            for x in vector {
                payload.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            stored.extend_from_slice(vector);
            return;
        };
        payload.resize(codec.code_bytes(), 0);
        codec.encode(vector, payload);
        stored.resize(vector.len(), 0.0);
        codec.decode(payload, stored);
    }

    /// The list of the centroid nearest `stored`: the only one, untrained.
    pub fn assign(&self, metric: &Metric, stored: &[f32]) -> usize {
        if self.centroids.is_empty() {
            return 0;
        }
        kmeans::nearest(metric, &self.centroids, stored.len(), stored).0
    }
}

impl State {
    /// The entry `wanted` in list `list`, if held: found by the map, or by
    /// scanning the list for a key the map does not hold alone.
    pub fn find(&self, (pool, file): (&BufferPool, FileId), wanted: Wanted<'_>, list: usize, buf: &mut Vec<u8>) -> Result<Option<Found>> {
        let key = place_key(wanted.0, wanted.1);
        if let Some((at, page)) = self.locate.get(&key).copied().map(unplace) {
            lists::read_page(pool, file, page, buf)?;
            if let Some((slot, count)) = slot_of(buf, self.shape.layout.width, wanted) {
                return Ok(Some(Found { list: at, page, slot, count }));
            }
        }
        if !self.collided.contains(&key) {
            return Ok(None);
        }
        let Some(head) = self.lists.get(list).map(|l| l.head) else { return Ok(None) };
        let mut next = Some(head);
        while let Some(page) = next {
            next = lists::read_page(pool, file, page, buf)?;
            if let Some((slot, count)) = slot_of(buf, self.shape.layout.width, wanted) {
                return Ok(Some(Found { list, page, slot, count }));
            }
        }
        Ok(None)
    }

    /// Notes an entry of key `key` appended to list `list` at `page`.
    pub fn located(&mut self, key: u64, list: usize, page: PageId) {
        match place(list, page).filter(|_| !self.locate.contains_key(&key)) {
            Some(packed) => {
                self.locate.insert(key, packed);
            }
            None => {
                self.collided.insert(key);
            }
        }
    }

    /// Changes list `at`'s and the index's counts for an entry whose
    /// count was `before`, changed up if `up`: a tombstone revived, or
    /// an entry made one.
    fn counted(&mut self, at: usize, before: u32, up: bool) {
        let revived = up && before == 0;
        let buried = !up && before == 1;
        if !(revived || buried) {
            return;
        }
        let (live, dead) = (&mut self.live, &mut self.dead);
        let list = self.lists.get_mut(at);
        if revived {
            *live = live.saturating_add(1);
            *dead = dead.saturating_sub(1);
            if let Some(l) = list {
                l.live = l.live.saturating_add(1);
                l.dead = l.dead.saturating_sub(1);
            }
        } else {
            *live = live.saturating_sub(1);
            *dead = dead.saturating_add(1);
            if let Some(l) = list {
                l.live = l.live.saturating_sub(1);
                l.dead = l.dead.saturating_add(1);
            }
        }
    }
}

/// The slot and count of the entry `wanted` in a list page's entries.
pub fn slot_of(entries: &[u8], width: usize, (row, hash, payload): Wanted<'_>) -> Option<(usize, u32)> {
    entries.chunks_exact(width).enumerate().find_map(|(slot, e)| {
        let (r, count, h) = lists::fields(e)?;
        (r == row && h == hash && e.get(lists::ENTRY_HEAD..) == Some(payload)).then_some((slot, count))
    })
}

/// An entry's stored bytes, the vector they stand for, and its list,
/// worked out with no lock held from a shape, and a page buffer.
#[derive(Default)]
struct Placed {
    payload: Vec<u8>,
    stored: Vec<f32>,
    list: usize,
    buf: Vec<u8>,
}

impl Ivf {
    /// The state's shape now.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub(super) fn snapshot(&self) -> Result<Arc<Shape>> {
        Ok(Arc::clone(&read(&self.inner.state)?.shape))
    }

    /// Where `vector` goes in `shape`, in `p`: its stored bytes and list,
    /// worked out by the codec and metric with no lock held (R-ERR-4).
    fn place(&self, shape: &Shape, vector: &[f32], p: &mut Placed) {
        shape.payload(vector, (&mut p.payload, &mut p.stored));
        p.list = shape.assign(&self.inner.metric, &p.stored);
    }

    /// Adds one entry of `row` under `vector`: its place worked out with
    /// no lock held, then written under the lock if training has not
    /// replaced the shape it was worked out in meanwhile, else again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for a vector of the wrong length or a metric giving
    /// no finite distance, or an index retrained under it time after
    /// time; -918 if the budget refuses its memory; the buffer pool's. A
    /// failed insert adds nothing.
    pub fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        let inner = &*self.inner;
        inner.metric.check(vector, inner.dims)?;
        let hash = vector_hash(vector);
        let mut p = Placed::default();
        for _ in 0..TRIES {
            let shape = self.snapshot()?;
            self.place(&shape, vector, &mut p);
            let mut state = write(&inner.state)?;
            if !Arc::ptr_eq(&state.shape, &shape) {
                continue;
            }
            return self.insert_placed(&mut state, (row, hash), &mut p);
        }
        Err(contended())
    }

    /// Adds an entry of `row` and `hash`, placed as `p`, to `state`.
    fn insert_placed(&self, state: &mut State, (row, hash): (u64, u64), p: &mut Placed) -> Result<()> {
        let inner = &*self.inner;
        let pages = (&*inner.pool, inner.file);
        let at = p.list;
        let layout = state.shape.layout;
        if let Some(found) = state.find(pages, (row, hash, &p.payload), at, &mut p.buf)? {
            let before = lists::count(pages.0, pages.1, (&layout, found.page, found.slot), true)?;
            state.counted(found.list, before, true);
            state.changes = state.changes.wrapping_add(1);
            return Ok(());
        }
        grow(&mut *lock(&inner.memory)?, LOCATE_BYTES)?;
        let mut entry = Vec::with_capacity(layout.width);
        lists::entry(&mut entry, (row, 1, hash), (&p.payload, squared_norm(&p.stored)));
        let mut list: List = *state.lists.get(at).ok_or_else(|| super::corrupt("a list is missing"))?;
        let page = lists::append(pages.0, pages.1, (&layout, &mut list), &entry)?;
        list.live = list.live.saturating_add(1);
        if let Some(slot) = state.lists.get_mut(at) {
            *slot = list;
        }
        state.live = state.live.saturating_add(1);
        state.changes = state.changes.wrapping_add(1);
        state.located(place_key(row, hash), at, page);
        Ok(())
    }

    /// Takes back one entry of `row` under `vector`, if there is one.
    ///
    /// # Errors
    ///
    /// The buffer pool's; a failed remove takes back nothing; SQLCODE
    /// -910 for an index retrained under it time after time.
    pub fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        let inner = &*self.inner;
        if vector.len() != inner.dims {
            return Ok(());
        }
        let hash = vector_hash(vector);
        let mut p = Placed::default();
        for _ in 0..TRIES {
            let shape = self.snapshot()?;
            self.place(&shape, vector, &mut p);
            let mut state = write(&inner.state)?;
            if !Arc::ptr_eq(&state.shape, &shape) {
                continue;
            }
            let pages = (&*inner.pool, inner.file);
            let found = state.find(pages, (row, hash, &p.payload), p.list, &mut p.buf)?;
            let Some(found) = found.filter(|f| f.count > 0) else { return Ok(()) };
            let before = lists::count(pages.0, pages.1, (&state.shape.layout, found.page, found.slot), false)?;
            state.counted(found.list, before, false);
            state.changes = state.changes.wrapping_add(1);
            return Ok(());
        }
        Err(contended())
    }

    /// The rows with an entry under `vector`, each once: its list read
    /// with no lock held, its shape kept so its pages are not freed.
    ///
    /// # Errors
    ///
    /// The buffer pool's; a poisoned lock.
    pub fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        let inner = &*self.inner;
        if vector.len() != inner.dims {
            return Ok(Vec::new());
        }
        let hash = vector_hash(vector);
        let shape = self.snapshot()?;
        let mut p = Placed::default();
        self.place(&shape, vector, &mut p);
        let Some(head) = shape.heads.get(p.list).copied() else { return Ok(Vec::new()) };
        let mut rows = Vec::new();
        lists::walk(&inner.pool, inner.file, (head, shape.layout.width), &mut p.buf, &mut |_, _, e| {
            let matching = lists::fields(e).filter(|(_, count, h)| *count > 0 && *h == hash);
            if let Some((found, _, _)) = matching.filter(|_| e.get(lists::ENTRY_HEAD..) == Some(p.payload.as_slice())) {
                rows.push(found);
            }
            Ok(())
        })?;
        rows.sort_unstable();
        rows.dedup();
        Ok(rows)
    }

    /// Its live entries and tombstones.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn counts(&self) -> Result<(u64, u64)> {
        let state = read(&self.inner.state)?;
        Ok((state.live, state.dead))
    }

    /// Its lists, and whether it keeps codes.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn shape(&self) -> Result<(usize, bool)> {
        let state = read(&self.inner.state)?;
        Ok((state.lists.len(), state.shape.codec.is_some()))
    }
}

/// The floats of a stored full vector's bytes, into `out`.
pub fn floats_of(bytes: &[u8], out: &mut [f32]) {
    for (o, b) in out.iter_mut().zip(bytes.as_chunks::<F32_BYTES>().0) {
        *o = f32::from_bits(u32::from_le_bytes(*b));
    }
}
