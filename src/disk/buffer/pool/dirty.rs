//! \[DESIGN\] The frames made dirty, and writing them back (RULES.md D30):
//! a flush visits the frames listed as made dirty since the last flush
//! took them, not every frame, so its cost follows the pages written, not
//! the pool's size, which may be a large share of the machine's memory.
//!
//! A frame is listed once, as it goes from clean to dirty, and stays
//! listed until a flush takes it, whatever it holds meanwhile: a frame
//! whose page is evicted, or freed, is found clean then and passed over.
//! So the list holds each frame at most once, however many pages pass
//! through it, and a pool that is never flushed (a spill area's) lists
//! each frame once and never again.
//!
//! A flush clears a frame's mark before it looks whether the frame is
//! dirty, and a writer marks it dirty before it looks whether it is
//! listed, all four in one total order (`SeqCst`): either the writer sees
//! the mark cleared and lists the frame again, or the flush sees it dirty.
//! A frame held for writing that is dirty already is listed, or being
//! flushed by a flush that copies it under its latch once the writer lets
//! go, so it is not looked at again: writing a page over and over costs
//! no more than it did.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use crate::disk::buffer::{BufferPool, FileId, Frame, Key, Pin, Source};
use crate::disk::io::Durability;
use crate::error::Result;
use crate::sync::lock_builtin;

/// \[DESIGN\] Shards of the list of frames made dirty, by frame: threads
/// writing different pages seldom take one lock.
const DIRTY_SHARDS: usize = 64;

/// One shard of the list, alone on its cache lines.
#[repr(align(128))]
#[derive(Default)]
struct DirtyShard(Mutex<Vec<usize>>);

/// The frames made dirty since a flush took them, by shard.
pub(in crate::disk::buffer) struct Dirtied {
    shards: Box<[DirtyShard]>,
}

impl Default for Dirtied {
    fn default() -> Dirtied {
        Dirtied { shards: (0..DIRTY_SHARDS).map(|_| DirtyShard::default()).collect() }
    }
}

impl Dirtied {
    /// Lists frame `index`.
    fn push(&self, index: usize) {
        let at = index.checked_rem(self.shards.len()).unwrap_or_default();
        if let Some(shard) = self.shards.get(at) {
            lock_builtin(&shard.0).push(index);
        }
    }

    /// The frames listed, the list left empty.
    fn take(&self) -> Vec<usize> {
        let mut all = Vec::new();
        for shard in &self.shards {
            all.append(&mut lock_builtin(&shard.0));
        }
        all
    }
}

impl BufferPool {
    /// Marks `frame`, latched exclusively, dirty, listing it if it was
    /// clean: a dirty one is listed, or being flushed by a flush that
    /// copies it once this latch is let go.
    pub(super) fn dirty_latched(&self, frame: &Frame) {
        if !frame.dirty.load(Ordering::SeqCst) {
            self.made_dirty(frame);
        }
    }

    /// Marks `frame` dirty, and lists it unless it is listed.
    pub(super) fn made_dirty(&self, frame: &Frame) {
        frame.dirty.store(true, Ordering::SeqCst);
        if !frame.listed.load(Ordering::SeqCst) && !frame.listed.swap(true, Ordering::SeqCst) {
            self.dirtied.push(frame.index);
        }
    }

    /// Writes back every dirty page. A page is copied under its shared latch
    /// and written with no lock held; one written to again meanwhile stays
    /// dirty. A frame being filled is waited for first: the dirty page it
    /// held may be being written back, no longer marked dirty.
    ///
    /// # Errors
    ///
    /// The first error of writing a page back; that page stays dirty.
    pub fn flush(&self) -> Result<()> {
        self.flush_visiting().map(|_| ())
    }

    /// `flush`, and the frames it visited: those listed as made dirty.
    pub(in crate::disk::buffer) fn flush_visiting(&self) -> Result<usize> {
        let listed = self.dirtied.take();
        for (at, frame) in listed.iter().filter_map(|&index| self.frames.get(index)).enumerate() {
            frame.listed.store(false, Ordering::SeqCst);
            if let Err(e) = self.flush_frame(frame) {
                // This frame listed again if still dirty, and those not
                // looked at yet put back as they were, listed.
                if frame.dirty.load(Ordering::SeqCst) {
                    self.made_dirty(frame);
                }
                listed.iter().skip(at.saturating_add(1)).for_each(|&rest| self.dirtied.push(rest));
                return Err(e);
            }
        }
        Ok(listed.len())
    }

    /// Writes back the page `frame` holds, if it is dirty.
    fn flush_frame(&self, frame: &Frame) -> Result<()> {
        frame.settled();
        let Some(key) = self.pin_dirty(frame)? else { return Ok(()) };
        let pin = Pin(frame);
        frame.settled();
        let content = {
            let latch = frame.content.read().unwrap_or_else(PoisonError::into_inner);
            frame.dirty.store(false, Ordering::SeqCst);
            latch.clone()
        };
        if let Err(e) = self.write_back(key, &content) {
            frame.dirty.store(true, Ordering::SeqCst);
            return Err(e);
        }
        drop(pin);
        Ok(())
    }

    /// The page `frame` holds, pinned under its shard, if it is dirty.
    fn pin_dirty(&self, frame: &Frame) -> Result<Option<Key>> {
        let Some(key) = frame.key() else { return Ok(None) };
        let _map = self.shared_map(key)?;
        let dirty = frame.key() == Some(key) && frame.dirty.load(Ordering::SeqCst);
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
