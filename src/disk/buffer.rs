//! \[DESIGN\] The buffer pool (RULES.md D30): a fixed number of frames in
//! memory caching the contents of pages, and the payloads of blocks, of any
//! number of registered files. Pager files are read and written through
//! it: a written page is dirty until the pool writes it back, on `flush`,
//! `sync`, or when its frame is wanted for another page. Block files are
//! cached for reading only: blocks never change once written, and appends
//! go to the block manager directly, as real writes.
//!
//! A page is used through a guard: `read` gives a shared latch, `write` an
//! exclusive one; both keep the frame from eviction while held. A thread
//! must not latch a page it already holds.
//!
//! A read that hits writes one thing shared: its frame's latch
//! (`hits.rs`). It finds the frame through a table of hints, read without
//! a lock, and checks under the latch that the frame holds the page; the
//! latch itself keeps the frame, as an eviction takes it exclusively. A
//! hint wrong, or a frame busy, sends the read the locked way: the page
//! table's shard, and a pin. Which frame is evicted is
//! an extension point (`Eviction`); CLOCK is built in.
//!
//! No lock is held during I/O, nor during a call into the eviction policy
//! (RULES.md R-ERR-4): a frame being filled or emptied is marked loading,
//! and threads wanting its page wait for it on a condition variable.

mod eviction;
mod hits;
mod pool;

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub use eviction::{Clock, Eviction};

use super::block::BlockFile;
use super::pager::Pager;
use crate::hash::murmur::MurmurMap;
use crate::memory::Reservation;
use crate::sync::lock_builtin;

/// A file registered with a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// A page of a pager file, or a block of a block file, in a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// Its file.
    pub file: FileId,
    /// Its page number or block id.
    pub id: u64,
}

/// A file the pool caches.
#[derive(Clone)]
enum Source {
    Pages(Arc<Pager>),
    Blocks(Arc<BlockFile>),
}

/// The counters of a pool, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolCounts {
    /// Pages or blocks found in the pool.
    pub hits: u64,
    /// Pages or blocks read into it.
    pub misses: u64,
    /// Of the hits, those that waited for another thread to read the page.
    pub waits: u64,
    /// Frames given up to another page.
    pub evictions: u64,
    /// Dirty pages written back.
    pub write_backs: u64,
}

/// The counters of the rarer events; hits are counted by thread
/// (`hits.rs`), waits per shard.
#[derive(Debug, Default)]
struct Counters {
    misses: AtomicU64,
    evictions: AtomicU64,
    write_backs: AtomicU64,
}

/// \[DESIGN\] Shards of the page table: threads hitting different pages
/// lock different shards. Fewer let threads collide; more no longer
/// stay in a thread's cache (`bench/buffer.rs` compares them).
const SHARDS: usize = 256;

/// One shard of the page table, with its own hit counters, alone on its
/// cache lines so shards used by different threads never share one.
#[repr(align(128))]
#[derive(Default)]
struct Shard {
    frames: RwLock<MurmurMap<Key, usize>>,
    waits: AtomicU64,
}

/// Adds 1 to `counter`; it would wrap only after 2^64, which no pool reaches.
fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// One frame: the page it holds, its pins, whether it is dirty, whether it
/// is being filled or emptied, and its content behind the page's latch.
/// Its page changes only under the page table's shard lock and the latch
/// held exclusively, so either keeps it as it is.
struct Frame {
    /// The page's file, plus one; 0 for none.
    key_file: AtomicU64,
    /// The page's number or block id.
    key_id: AtomicU64,
    pins: AtomicUsize,
    dirty: AtomicBool,
    /// Set while the frame is filled or emptied: wait, then check it holds
    /// the page wanted. Read without a lock on every hit.
    loading: AtomicBool,
    /// Guards waiting for `loading` to clear.
    waiting: Mutex<()>,
    changed: Condvar,
    content: RwLock<Vec<u8>>,
}

impl Frame {
    fn empty() -> Frame {
        Frame {
            key_file: AtomicU64::new(0),
            key_id: AtomicU64::new(0),
            pins: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
            loading: AtomicBool::new(false),
            waiting: Mutex::new(()),
            changed: Condvar::new(),
            content: RwLock::new(Vec::new()),
        }
    }

    /// Marks the frame filled or emptied, and wakes those waiting for it.
    fn ready(&self) {
        let _waiting = lock_builtin(&self.waiting);
        self.loading.store(false, Ordering::Release);
        self.changed.notify_all();
    }

    /// Waits until the frame is not loading; whether it had to wait.
    fn settled(&self) -> bool {
        if !self.loading.load(Ordering::Acquire) {
            return false;
        }
        let mut waiting = lock_builtin(&self.waiting);
        while self.loading.load(Ordering::Acquire) {
            waiting = self.changed.wait(waiting).unwrap_or_else(PoisonError::into_inner);
        }
        true
    }

    /// The key of the page it holds.
    fn key(&self) -> Option<Key> {
        let file = self.key_file.load(Ordering::Acquire).checked_sub(1)?;
        Some(Key { file: FileId(u32::try_from(file).ok()?), id: self.key_id.load(Ordering::Acquire) })
    }

    /// Makes `key` the page it holds, under its shard's lock and its latch.
    fn set_key(&self, key: Option<Key>) {
        self.key_id.store(key.map_or(0, |k| k.id), Ordering::Release);
        self.key_file.store(key.map_or(0, |k| u64::from(k.file.0).saturating_add(1)), Ordering::Release);
    }
}

/// \[DESIGN\] A buffer pool. It may be shared between threads.
pub struct BufferPool {
    frames: Vec<Frame>,
    /// The page table, by shard; a frame is pinned, and evicted, only under
    /// the shard of the page it holds.
    shards: Vec<Shard>,
    /// Frames holding no page. Taken after a shard's lock, never before.
    empty: Mutex<Vec<usize>>,
    sources: RwLock<Vec<Source>>,
    eviction: Box<dyn Eviction>,
    counters: Counters,
    /// Where each page's frame was last found, by the page's hash
    /// (`hits.rs`): read without a lock.
    hints: hits::Hints,
    /// Its share of the database's memory budget, if it was made in one,
    /// held while it lives and given back as it is dropped.
    _reservation: Option<Reservation>,
}

/// A pin on a frame, released when dropped.
struct Pin<'a>(&'a Frame);

impl Drop for Pin<'_> {
    fn drop(&mut self) {
        self.0.pins.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A page held for reading: its content, under a shared latch. The latch is
/// declared, and so released, before the pin: a frame is never evicted
/// while latched.
pub struct PageRef<'a> {
    content: RwLockReadGuard<'a, Vec<u8>>,
    /// None for a page found by its hint: the latch keeps the frame.
    _pin: Option<Pin<'a>>,
}

/// A page held for writing: its content, under an exclusive latch, released
/// before the pin as for `PageRef`. The page is dirty from the moment it is
/// held.
pub struct PageMut<'a> {
    content: RwLockWriteGuard<'a, Vec<u8>>,
    _pin: Pin<'a>,
}

impl Deref for PageRef<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.content
    }
}

impl Deref for PageMut<'_> {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.content
    }
}

impl DerefMut for PageMut<'_> {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.content
    }
}
