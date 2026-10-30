//! \[DESIGN\] The buffer pool's operations (RULES.md D30): registering
//! files, holding pages, filling frames on a miss, evicting, writing back
//! and syncing. Split from buffer.rs (RULES.md R-SIZE-2).
//!
//! The page table is sharded by page. A frame is pinned under the shard of
//! the page it holds, locked shared, so threads reading one page (a tree's
//! root) pin it at once; and claimed for another page under the shards of
//! both, locked exclusively in shard order, after checking it is still
//! unpinned, and latching it exclusively without waiting, as a read that
//! hit by its hint holds its latch but no pin (`hits.rs`); then
//! it is marked loading, and the old page's content is written back and the
//! new one read with no lock held. A dirty old page stays mapped to the
//! frame until it is written back, so a thread wanting it waits for the
//! frame rather than reading the file's older copy. A thread that waited
//! for a frame checks that it holds the page it wanted, and tries again if
//! not, so a load or a write-back that fails need only put the frame back
//! as it was.

use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

use super::{BufferPool, Clock, Counters, Eviction, FileId, Frame, Key, PageMut, PageRef, Pin, PoolCounts, SHARDS, Shard, Source, count};
use crate::disk::block::{BlockFile, BlockId};
use crate::disk::io::Durability;
use crate::disk::pager::{Extent, PageId, Pager};
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::MurmurMap;
use crate::memory::MemoryBudget;
use crate::sync::lock_builtin;

/// A shard's map of pages to frames, locked exclusively.
type Map<'a> = RwLockWriteGuard<'a, MurmurMap<Key, usize>>;

/// A frame's content, latched exclusively.
type Latch<'a> = RwLockWriteGuard<'a, Vec<u8>>;

/// `frame` latched exclusively, if no one holds its latch now.
fn try_latch(frame: &Frame) -> Option<Latch<'_>> {
    match frame.content.try_write() {
        Ok(latch) => Some(latch),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// Locks a shard's map exclusively. The pool holds it over no plug-in
/// call, so a poisoned one is used as it is.
fn write_map(map: &RwLock<MurmurMap<Key, usize>>) -> Map<'_> {
    map.write().unwrap_or_else(PoisonError::into_inner)
}

/// How a claimed frame is filled.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// With its page's content, read from its file.
    Read,
    /// With no content: a page just allocated.
    Empty,
}

/// \[DESIGN\] The shard hash: Fibonacci hashing (Knuth, *The Art of
/// Computer Programming*, vol. 3, §6.4): the multiplier is 2^64 divided by
/// the golden ratio, and the top eight bits pick one of 256 shards. The file
/// is mixed in with the FNV-1a 64-bit prime, so equal page numbers of
/// different files spread too.
const SHARD_MIX: u64 = 0x9E37_79B9_7F4A_7C15;
const SHARD_FILE_MIX: u64 = 0x0100_0000_01B3;
const SHARD_SHIFT: u32 = 56;

/// A frame claimed for a page: its index, and the page it held, if that
/// page was dirty and must be written back.
#[derive(Clone, Copy)]
struct Claim {
    frame: usize,
    write_back: Option<Key>,
}

impl BufferPool {
    /// A pool of `frames` frames, evicting by `eviction`, which must have
    /// been made for that many frames.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `frames` is 0.
    pub fn new(frames: usize, eviction: Box<dyn Eviction>) -> Result<BufferPool> {
        if frames == 0 {
            return Err(SqlError::new(Code::Api, "[DESIGN] a buffer pool needs at least one frame"));
        }
        Ok(BufferPool {
            frames: (0..frames).map(|_| Frame::empty()).collect(),
            shards: (0..SHARDS).map(|_| Shard::default()).collect(),
            empty: Mutex::new((0..frames).rev().collect()),
            sources: RwLock::new(Vec::new()),
            eviction,
            counters: Counters::default(),
            hints: super::hits::Hints::new(frames),
            _reservation: None,
        })
    }

    /// \[DESIGN\] A pool of `bytes` of pages of `page_size` bytes, as many
    /// frames as they fill, at least one, evicting by CLOCK; its frames'
    /// pages reserved of `budget` while it lives (RULES.md D38), as held by
    /// `what`.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget cannot grant them; -910 if `page_size` is 0.
    pub fn in_budget(budget: &Arc<MemoryBudget>, (bytes, page_size): (usize, usize), what: &str) -> Result<BufferPool> {
        let frames = bytes.checked_div(page_size).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a page has at least one byte"))?.max(1);
        let reservation = budget.reserve(frames.saturating_mul(page_size), what)?;
        let pool = BufferPool::new(frames, Box::new(Clock::new(frames)))?;
        Ok(BufferPool { _reservation: Some(reservation), ..pool })
    }

    /// Registers a pager file, read and written through the pool.
    pub fn add_pager(&self, pager: Arc<Pager>) -> FileId {
        self.add(Source::Pages(pager))
    }

    /// Registers a block file, read through the pool.
    pub fn add_blocks(&self, blocks: Arc<BlockFile>) -> FileId {
        self.add(Source::Blocks(blocks))
    }

    fn add(&self, source: Source) -> FileId {
        let mut sources = self.sources.write().unwrap_or_else(PoisonError::into_inner);
        sources.push(source);
        FileId(u32::try_from(sources.len().saturating_sub(1)).unwrap_or(u32::MAX))
    }

    /// The file registered as `file`.
    fn source(&self, file: FileId) -> Result<Source> {
        let sources = self.sources.read().unwrap_or_else(PoisonError::into_inner);
        sources
            .get(usize::try_from(file.0).unwrap_or(usize::MAX))
            .cloned()
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no file {} is registered with the buffer pool", file.0)))
    }

    /// The pager file registered as `file`.
    fn pager(&self, file: FileId) -> Result<Arc<Pager>> {
        match self.source(file)? {
            Source::Pages(pager) => Ok(pager),
            Source::Blocks(_) => {
                Err(SqlError::new(Code::Api, format!("[DESIGN] file {} is a block file: its blocks are only read", file.0)))
            }
        }
    }

    /// The most content a page of pager file `file` holds without overflow
    /// pages, for structures that size their nodes to a page.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file.
    pub fn capacity(&self, file: FileId) -> Result<usize> {
        Ok(self.pager(file)?.inline_capacity())
    }

    /// Frames in the pool.
    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    /// The counters now.
    pub fn counts(&self) -> PoolCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        let sum = |each: fn(&Shard) -> &AtomicU64| self.shards.iter().fold(0_u64, |n, s| n.saturating_add(get(each(s))));
        PoolCounts {
            hits: self.hints.hits(),
            misses: get(&c.misses),
            waits: sum(|s| &s.waits),
            evictions: get(&c.evictions),
            write_backs: get(&c.write_backs),
        }
    }

    /// The shard of `key`'s page.
    fn shard_of(key: Key) -> usize {
        // Fibonacci hashing spreads neighbouring pages over the shards.
        let mixed = key.id.wrapping_add(u64::from(key.file.0).wrapping_mul(SHARD_FILE_MIX)).wrapping_mul(SHARD_MIX);
        usize::try_from(mixed >> SHARD_SHIFT).unwrap_or_default() % SHARDS
    }

    fn shard(&self, index: usize) -> Result<&Shard> {
        self.shards.get(index).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a page table shard is missing"))
    }

    /// The map of `key`'s shard, locked exclusively.
    fn map(&self, key: Key) -> Result<Map<'_>> {
        Ok(write_map(&self.shard(Self::shard_of(key))?.frames))
    }

    /// The map of `key`'s shard, locked shared: enough to pin a frame, as
    /// a frame is claimed for another page only under the exclusive lock.
    fn shared_map(&self, key: Key) -> Result<RwLockReadGuard<'_, MurmurMap<Key, usize>>> {
        Ok(self.shard(Self::shard_of(key))?.frames.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// The maps of the shards of `a` and `b`, locked in shard order; the
    /// second is `None` when they share a shard.
    fn maps(&self, a: Key, b: Key) -> Result<(Map<'_>, Option<Map<'_>>)> {
        let (sa, sb) = (Self::shard_of(a), Self::shard_of(b));
        let (first, second) = (self.shard(sa)?, self.shard(sb)?);
        if sa == sb {
            return Ok((write_map(&first.frames), None));
        }
        if sa < sb {
            let ga = write_map(&first.frames);
            return Ok((ga, Some(write_map(&second.frames))));
        }
        let gb = write_map(&second.frames);
        Ok((write_map(&first.frames), Some(gb)))
    }
}

impl BufferPool {
    /// Holds page or block `id` of `file` for reading.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not registered, or the page is not
    /// allocated; -918 if every frame is held, refused at once, never
    /// waited for (RULES.md D38); the errors of reading the page or block,
    /// or of writing back the page whose frame it takes.
    pub fn read(&self, file: FileId, id: u64) -> Result<PageRef<'_>> {
        let key = Key { file, id };
        if let Some(page) = self.read_hinted(key) {
            return Ok(page);
        }
        let frame = self.hold(key, Fill::Read)?;
        let pin = Pin(frame);
        Ok(PageRef { content: frame.content.read().unwrap_or_else(PoisonError::into_inner), _pin: Some(pin) })
    }

    /// Holds page `id` of pager file `file` for writing; it is dirty from
    /// now on.
    ///
    /// # Errors
    ///
    /// As `read`; SQLCODE -910 also if `file` is a block file.
    pub fn write(&self, file: FileId, id: PageId) -> Result<PageMut<'_>> {
        self.pager(file)?;
        let frame = self.hold(Key { file, id: id.0 }, Fill::Read)?;
        Ok(Self::held_for_writing(frame))
    }

    /// Allocates a page of pager file `file` and holds it for writing, with
    /// no content.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; SQLCODE -918
    /// if every frame is held, or its file is kept in memory of a budget
    /// with no room for the page (`Pager::reserve`); the errors of writing
    /// back the page whose frame it takes.
    pub fn new_page(&self, file: FileId) -> Result<(PageId, PageMut<'_>)> {
        let pager = self.pager(file)?;
        let id = pager.allocate(1).first().map_or(PageId(0), |e| e.start);
        match pager.reserve(id).and_then(|()| self.hold(Key { file, id: id.0 }, Fill::Empty)) {
            Ok(frame) => Ok((id, Self::held_for_writing(frame))),
            Err(e) => {
                // Never written, so nothing refers to it.
                let _ = pager.free(&[Extent { start: id, count: 1 }]);
                Err(e)
            }
        }
    }

    fn held_for_writing(frame: &Frame) -> PageMut<'_> {
        let pin = Pin(frame);
        let content = frame.content.write().unwrap_or_else(PoisonError::into_inner);
        frame.dirty.store(true, Ordering::Release);
        PageMut { content, _pin: pin }
    }

    /// Frees page `id` of pager file `file`: in the pool, and in its file.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file, or the page is
    /// held; the pager's errors of freeing it.
    pub fn free(&self, file: FileId, id: PageId) -> Result<()> {
        self.free_page(file, id, false)
    }

    /// Frees page `id` of pager file `file` as `free` does, once no one
    /// holds it: for a structure whose readers lock nothing, which may be
    /// reading the page, each for as long as one read takes, and find it no
    /// longer theirs when they look again (`engine/hash.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; the pager's
    /// errors of freeing it.
    pub fn free_when_released(&self, file: FileId, id: PageId) -> Result<()> {
        self.free_page(file, id, true)
    }

    /// `free`, waiting for the page's pins to be released if `wait`, else
    /// refusing a page held.
    fn free_page(&self, file: FileId, id: PageId, wait: bool) -> Result<()> {
        let pager = self.pager(file)?;
        let key = Key { file, id: id.0 };
        loop {
            let mut map = self.map(key)?;
            let Some(&index) = map.get(&key) else { break };
            let frame = self.frame(index)?;
            // Being written back from a frame now another page's: wait.
            if frame.key() != Some(key) {
                drop(map);
                frame.settled();
                continue;
            }
            // Held: pinned, or latched by a read that hit by its hint.
            let latch = if frame.pins.load(Ordering::Acquire) > 0 { None } else { try_latch(frame) };
            let Some(mut content) = latch else {
                if wait {
                    drop(map);
                    std::thread::yield_now();
                    continue;
                }
                return Err(SqlError::new(Code::Api, format!("[DESIGN] page {} of file {} is held, so it cannot be freed", id.0, file.0)));
            };
            map.remove(&key);
            frame.set_key(None);
            frame.dirty.store(false, Ordering::Release);
            content.clear();
            lock_builtin(&self.empty).push(index);
            break;
        }
        pager.free(&[Extent { start: id, count: 1 }])
    }

    fn frame(&self, index: usize) -> Result<&Frame> {
        self.frames
            .get(index)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the eviction policy proposed frame {index}, past the pool's frames")))
    }
}

impl BufferPool {
    /// Pins the frame holding `key`, filling one if none does; the hint of
    /// `key` the frame.
    fn hold(&self, key: Key, fill: Fill) -> Result<&Frame> {
        loop {
            if let Some(pinned) = self.pin_mapped(key)? {
                if let Some(frame) = self.settled_holding(pinned, key)? {
                    self.hints.note(key, pinned.0);
                    return Ok(frame);
                }
                // Its load failed, or it was put back: try again.
                continue;
            }
            if let Some(claim) = self.claim(key)? {
                let frame = self.fill(key, claim, fill)?;
                self.hints.note(key, claim.frame);
                return Ok(frame);
            }
        }
    }

    /// Pins the frame mapped to `key`, if one is: its index, and whether it
    /// was loading when pinned, seen under the shard's lock, as a frame
    /// starts loading, and a page mapped to a loading frame is unmapped,
    /// only under the shard's exclusive lock.
    fn pin_mapped(&self, key: Key) -> Result<Option<(usize, bool)>> {
        let map = self.shared_map(key)?;
        let Some(&index) = map.get(&key) else { return Ok(None) };
        let frame = self.frame(index)?;
        frame.pins.fetch_add(1, Ordering::AcqRel);
        Ok(Some((index, frame.loading.load(Ordering::Acquire))))
    }

    /// The frame pinned as `pin_mapped` gave it, once it is not loading, if
    /// it holds `key`; unpinned otherwise. A frame that was not loading when
    /// pinned holds `key` and cannot change page while pinned; one that was
    /// is checked, whether or not it is loading still: it may have been
    /// filled for another page while `key` was written back, or emptied.
    fn settled_holding(&self, (index, loading): (usize, bool), key: Key) -> Result<Option<&Frame>> {
        let frame = self.frame(index)?;
        // Not loading when pinned, it cannot start to while pinned.
        let waited = loading && frame.settled();
        if loading && frame.key() != Some(key) {
            drop(Pin(frame));
            return Ok(None);
        }
        let shard = self.shard(Self::shard_of(key))?;
        self.hints.hit();
        if waited {
            count(&shard.waits);
        }
        self.eviction.accessed(index);
        Ok(Some(frame))
    }

    /// Claims a frame for `key`: an empty one, or one the eviction policy
    /// proposes, checked again under the shards of both pages. `None` if
    /// another thread mapped `key` meanwhile.
    fn claim(&self, key: Key) -> Result<Option<Claim>> {
        loop {
            {
                let mut map = self.map(key)?;
                if map.contains_key(&key) {
                    return Ok(None);
                }
                if let Some(index) = lock_builtin(&self.empty).pop() {
                    // An empty frame's latch is held only by a read that
                    // found it holds no page, and lets go at once.
                    let frame = self.frame(index)?;
                    let latch = frame.content.write().unwrap_or_else(PoisonError::into_inner);
                    return self.take(&mut map, (index, latch), key, None).map(Some);
                }
            }
            let evictable = |index: usize| self.frames.get(index).is_some_and(|f| f.pins.load(Ordering::Acquire) == 0 && f.key().is_some());
            // Refused rather than waited for: the pool knows no statement
            // to bound a wait by, and threads each holding pages while
            // waiting for one more would wait for each other (RULES.md D38).
            let Some(index) = self.eviction.victim(&evictable) else {
                return Err(SqlError::new(
                    Code::Resources,
                    format!(
                        "[DESIGN] RULES.md D38: all {} frames of the buffer pool are held at once: it is too small for the work (STORAGE.POOL_BYTES, or WORK_MEMORY for a spill area)",
                        self.frames.len()
                    ),
                ));
            };
            let frame = self.frame(index)?;
            let Some(old) = frame.key() else { continue };
            let (mut map, mut old_map) = self.maps(key, old)?;
            if map.contains_key(&key) {
                return Ok(None);
            }
            // Still the page it held, and unpinned, now that both shards are
            // locked: pins rise only under the shard of the page held; and
            // not latched by a read that hit by its hint.
            let latch = (frame.key() == Some(old) && frame.pins.load(Ordering::Acquire) == 0).then(|| try_latch(frame)).flatten();
            if let Some(latch) = latch {
                count(&self.counters.evictions);
                let claim = self.take(&mut map, (index, latch), key, Some(old))?;
                // A dirty page stays mapped until written back (`fill`).
                if claim.write_back.is_none() {
                    old_map.as_mut().unwrap_or(&mut map).remove(&old);
                }
                return Ok(Some(claim));
            }
        }
    }

    /// Maps frame `index`, latched exclusively, to `key` in `map`, its
    /// shard's, pinned and loading; `old` is the page it held, written back
    /// only if dirty.
    fn take(&self, map: &mut MurmurMap<Key, usize>, (index, latch): (usize, Latch<'_>), key: Key, old: Option<Key>) -> Result<Claim> {
        let frame = self.frame(index)?;
        map.insert(key, index);
        frame.pins.fetch_add(1, Ordering::AcqRel);
        frame.set_key(Some(key));
        frame.loading.store(true, Ordering::Release);
        drop(latch);
        let dirty = frame.dirty.swap(false, Ordering::AcqRel);
        Ok(Claim { frame: index, write_back: old.filter(|_| dirty) })
    }

    /// Writes back what `claim`'s frame held if it must, then fills it for
    /// `key`. If either fails, the frame is put back as it was (or emptied)
    /// and the error returned.
    fn fill(&self, key: Key, claim: Claim, fill: Fill) -> Result<&Frame> {
        let frame = self.frame(claim.frame)?;
        let old_content = mem::take(&mut *frame.content.write().unwrap_or_else(PoisonError::into_inner));
        if let Some(old) = claim.write_back {
            if let Err(e) = self.write_back(old, &old_content) {
                self.restore(claim.frame, key, old, old_content);
                return Err(e);
            }
            // Written back: a thread wanting it now reads it from the file.
            if let Ok(mut map) = self.map(old)
                && map.get(&old) == Some(&claim.frame)
            {
                map.remove(&old);
            }
        }
        let mut content = Vec::new();
        if fill == Fill::Read {
            count(&self.counters.misses);
            if let Err(e) = self.read_into(key, &mut content) {
                self.restore_empty(claim.frame, key);
                return Err(e);
            }
        }
        *frame.content.write().unwrap_or_else(PoisonError::into_inner) = content;
        frame.dirty.store(fill == Fill::Empty, Ordering::Release);
        frame.ready();
        self.eviction.accessed(claim.frame);
        Ok(frame)
    }

    /// Reads `key` from its file into `content`.
    fn read_into(&self, key: Key, content: &mut Vec<u8>) -> Result<()> {
        match self.source(key.file)? {
            Source::Pages(pager) => pager.read(PageId(key.id), content),
            Source::Blocks(blocks) => blocks.read_into(BlockId(key.id), content).map(|_| ()),
        }
    }

    /// Writes `content` back as page `key`.
    fn write_back(&self, key: Key, content: &[u8]) -> Result<()> {
        self.pager(key.file)?.write(PageId(key.id), content)?;
        count(&self.counters.write_backs);
        Ok(())
    }

    /// Puts frame `index` back holding `old`, dirty, after writing it back
    /// failed; `key`, which claimed it, is unmapped, and `old`, still
    /// mapped, mapped to it again.
    fn restore(&self, index: usize, key: Key, old: Key, content: Vec<u8>) {
        let Some(frame) = self.frames.get(index) else { return };
        if let Ok((mut map, mut old_map)) = self.maps(key, old) {
            map.remove(&key);
            old_map.as_mut().unwrap_or(&mut map).insert(old, index);
            let mut latch = frame.content.write().unwrap_or_else(PoisonError::into_inner);
            frame.set_key(Some(old));
            *latch = content;
        }
        frame.dirty.store(true, Ordering::Release);
        frame.ready();
        drop(Pin(frame));
    }

    /// Empties frame `index` after reading `key` into it failed.
    fn restore_empty(&self, index: usize, key: Key) {
        let Some(frame) = self.frames.get(index) else { return };
        if let Ok(mut map) = self.map(key) {
            map.remove(&key);
            let _latch = frame.content.write().unwrap_or_else(PoisonError::into_inner);
            frame.set_key(None);
            lock_builtin(&self.empty).push(index);
        }
        frame.ready();
        drop(Pin(frame));
    }
}

impl BufferPool {
    /// Writes back every dirty page. A page is copied under its shared latch
    /// and written with no lock held; one written to again meanwhile stays
    /// dirty. A frame being filled is waited for first: the dirty page it
    /// held may be being written back, no longer marked dirty.
    ///
    /// # Errors
    ///
    /// The first error of writing a page back; that page stays dirty.
    pub fn flush(&self) -> Result<()> {
        for frame in &self.frames {
            frame.settled();
            let Some(key) = self.pin_dirty(frame)? else { continue };
            let pin = Pin(frame);
            frame.settled();
            let content = {
                let latch = frame.content.read().unwrap_or_else(PoisonError::into_inner);
                frame.dirty.store(false, Ordering::Release);
                latch.clone()
            };
            if let Err(e) = self.write_back(key, &content) {
                frame.dirty.store(true, Ordering::Release);
                return Err(e);
            }
            drop(pin);
        }
        Ok(())
    }

    /// The page `frame` holds, pinned under its shard, if it is dirty.
    fn pin_dirty(&self, frame: &Frame) -> Result<Option<Key>> {
        let Some(key) = frame.key() else { return Ok(None) };
        let _map = self.shared_map(key)?;
        let dirty = frame.key() == Some(key) && frame.dirty.load(Ordering::Acquire);
        if dirty {
            frame.pins.fetch_add(1, Ordering::AcqRel);
        }
        Ok(dirty.then_some(key))
    }

    /// Writes back every dirty page, then syncs `file` as `durability` says.
    ///
    /// # Errors
    ///
    /// As `flush`; SQLCODE -910 if `file` is not registered; the errors of
    /// the file's sync.
    pub fn sync(&self, file: FileId, durability: Durability) -> Result<()> {
        self.flush()?;
        match self.source(file)? {
            Source::Pages(pager) => pager.sync(durability),
            Source::Blocks(blocks) => blocks.sync(durability),
        }
    }
}
