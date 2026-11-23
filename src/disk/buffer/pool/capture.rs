//! \[DESIGN\] A checkpoint's capture of the pool's dirty pages, copy on
//! write (RULES.md D590).
//!
//! With writers held, a checkpoint marks each dirty page of its file it
//! may capture later (`Pager::deferrable`) as owed, and clean: it copies,
//! encodes and writes nothing then. The rest, few, are written back as
//! before. Once writers go on, each page owed is taken as it was, once:
//!
//! * by a writer, which, latching it for writing, finds it owed, and
//!   copies it first, under the latch it takes anyway;
//! * by a claim evicting it, which captures it, the page kept mapped
//!   meanwhile as a dirty one is while written back, so no read finds
//!   the file without it; and by a free, which copies it;
//! * by the checkpoint, which walks the pages owed and captures each one
//!   still owed, under its read latch, and each copy.
//!
//! Each capture is written into the batch the checkpoint sealed
//! (`Pager::captured`), beneath what the pool writes back since. Only a
//! latch changes whether a page is owed: shared for the walk, exclusive
//! for the others, so a page is taken once. An eviction counts itself in
//! flight before it lets the frame's latch go, and the walk waits for
//! those in flight before it ends, so every page owed is in the batch when
//! the checkpoint commits it. Memory is bounded by the pages written while
//! one checkpoint walks.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread;
use std::time::Duration;

use crate::disk::buffer::{BufferPool, FileId, Frame, Key, PageMut, Pin};
use crate::disk::pager::{Capturing, PageId, Pager};
use crate::error::{Code, Result, SqlError};
use crate::hash::xxhash::Xxh3Map;
use crate::sync::lock_builtin;

/// \[DESIGN\] How long the walk sleeps while evictions capture pages owed:
/// one capture's encoding and copy.
const IN_FLIGHT_WAIT: Duration = Duration::from_micros(20);

/// What a pool keeps of the capture under way, if one is.
#[derive(Default)]
pub(in crate::disk::buffer) struct Owed {
    /// Pages owed, copied as they were by a writer or a free, by page.
    copies: Mutex<Xxh3Map<Key, Vec<u8>>>,
    /// Evictions capturing a page owed now.
    in_flight: AtomicUsize,
    /// The file captured, and how.
    plan: RwLock<Option<(FileId, Arc<Capturing>)>>,
    /// The first failure of an eviction's capture.
    failure: Mutex<Option<SqlError>>,
}

/// \[DESIGN\] A checkpoint's capture under way (RULES.md D590): the pages
/// it owes, each with the frame that held it when the capture began.
#[derive(Default)]
pub struct Capture {
    pages: Vec<(usize, Key)>,
}

impl Capture {
    /// Whether it owes no page.
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}

impl BufferPool {
    /// \[DESIGN\] Begins a checkpoint's capture of pager file `file`, with
    /// its writers held: each dirty page of it that may be captured later
    /// marked owed and clean, the file made to hold the last
    /// (`Pager::cover`). The other dirty pages are left for `flush`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; the file's,
    /// if it cannot be made to hold the pages: none is owed then.
    pub fn capture(&self, file: FileId) -> Result<Capture> {
        let pager = self.pager(file)?;
        let Some(capturing) = pager.capturing().map(Arc::new) else { return Ok(Capture::default()) };
        // Set before any page is owed: a reader's miss, which the shut
        // does not hold, may evict a page owed as soon as it is marked.
        *self.owed.plan.write().unwrap_or_else(PoisonError::into_inner) = Some((file, Arc::clone(&capturing)));
        let mut capture = Capture::default();
        // The frames made dirty (`dirty.rs`), not every frame: a page owed
        // leaves the list, as it is clean, and the walk takes it; one left
        // dirty is listed again for the flush after.
        let listed = self.dirtied_taken();
        for (at, &index) in listed.iter().enumerate() {
            let Ok(frame) = self.frame(index) else { continue };
            frame.listed.store(false, Ordering::SeqCst);
            frame.settled();
            let key = match self.pin_dirty(frame) {
                Ok(Some(key)) => key,
                Ok(None) => continue,
                Err(e) => {
                    self.made_dirty(frame);
                    self.dirtied_put_back(listed.iter().skip(at.saturating_add(1)));
                    self.abandon(capture);
                    return Err(e);
                }
            };
            let pin = Pin(frame);
            frame.settled();
            let latch = frame.content.read().unwrap_or_else(PoisonError::into_inner);
            if key.file == file && pager.deferrable(PageId(key.id), latch.len(), &capturing) {
                frame.owed.store(true, Ordering::Release);
                frame.dirty.store(false, Ordering::SeqCst);
                capture.pages.push((index, key));
            } else {
                self.made_dirty(frame);
            }
            drop((latch, pin));
        }
        let last = capture.pages.iter().map(|(_, key)| key.id).max();
        if let Some(Err(e)) = last.map(|id| pager.cover(PageId(id))) {
            self.abandon(capture);
            return Err(e);
        }
        Ok(capture)
    }

    /// \[DESIGN\] Ends `capture`, writers going on: every page it owes
    /// captured, by the walk or by whoever took it, before it returns.
    ///
    /// # Errors
    ///
    /// The first failure of a capture: the batch then lacks a page, and is
    /// not to be committed. Every page is left dirty that would otherwise
    /// be in it alone.
    pub fn capture_rest(&self, capture: Capture) -> Result<()> {
        let Some((file, capturing)) = self.owed.plan.read().unwrap_or_else(PoisonError::into_inner).clone() else { return Ok(()) };
        let pager = self.pager(file)?;
        let mut first: Result<()> = Ok(());
        for (index, key) in capture.pages {
            let walked = self.walked(pager, (index, key), &capturing);
            let copied = self.copied(pager, key, &capturing);
            first = first.and(walked).and(copied);
        }
        self.landed();
        let left: Vec<Key> = lock_builtin(&self.owed.copies).keys().copied().collect();
        for key in left {
            first = first.and(self.copied(pager, key, &capturing));
        }
        *self.owed.plan.write().unwrap_or_else(PoisonError::into_inner) = None;
        let failed = lock_builtin(&self.owed.failure).take();
        first.and(failed.map_or(Ok(()), Err))
    }

    /// \[DESIGN\] Gives `capture` up before its batch is sealed: each page
    /// still owed is dirty again, the evictions capturing one ended, and the
    /// copies are dropped.
    pub fn abandon(&self, capture: Capture) {
        for (index, key) in capture.pages {
            let Some(frame) = self.frames.get(index) else { continue };
            let _latch = frame.content.read().unwrap_or_else(PoisonError::into_inner);
            if frame.key() == Some(key) && frame.owed.swap(false, Ordering::AcqRel) {
                self.made_dirty(frame);
            }
        }
        self.landed();
        lock_builtin(&self.owed.copies).clear();
        *self.owed.plan.write().unwrap_or_else(PoisonError::into_inner) = None;
        lock_builtin(&self.owed.failure).take();
    }

    /// Waits until no eviction is capturing a page owed.
    fn landed(&self) {
        while self.owed.in_flight.load(Ordering::Acquire) > 0 {
            thread::sleep(IN_FLIGHT_WAIT);
        }
    }

    /// Captures the page frame `index` held, `key`, if it is still owed,
    /// under its read latch; a failure leaves it dirty.
    fn walked(&self, pager: &Pager, (index, key): (usize, Key), capturing: &Capturing) -> Result<()> {
        let frame = self.frame(index)?;
        let latch = frame.content.read().unwrap_or_else(PoisonError::into_inner);
        if frame.key() != Some(key) || !frame.owed.load(Ordering::Acquire) {
            return Ok(());
        }
        frame.owed.store(false, Ordering::Release);
        let captured = pager.captured(PageId(key.id), &latch, capturing);
        if captured.is_err() {
            self.made_dirty(frame);
        }
        captured
    }

    /// Captures the copy of `key` a writer or a free took, if one did.
    fn copied(&self, pager: &Pager, key: Key, capturing: &Capturing) -> Result<()> {
        let copy = lock_builtin(&self.owed.copies).remove(&key);
        copy.map_or(Ok(()), |content| pager.captured(PageId(key.id), &content, capturing))
    }

    /// `frame` latched for writing, dirty; copied first if a checkpoint's
    /// capture owes it (`capture.rs`).
    pub(in crate::disk::buffer) fn held_for_writing<'a>(&self, frame: &'a Frame) -> PageMut<'a> {
        let pin = Pin(frame);
        let content = frame.latched();
        if frame.owed.load(Ordering::Acquire) {
            self.owed_copy(frame, &content);
        }
        self.dirty_latched(frame);
        PageMut { content, _pin: pin }
    }

    /// Copies the page `frame` holds, owed, as it is, before it is changed
    /// or freed: under its exclusive latch, held by the caller.
    pub(super) fn owed_copy(&self, frame: &Frame, content: &[u8]) {
        let owed = frame.owed.swap(false, Ordering::AcqRel);
        if let Some(key) = frame.key().filter(|_| owed) {
            lock_builtin(&self.owed.copies).insert(key, content.to_vec());
        }
    }

    /// Whether the page `frame` holds, latched exclusively by a claim, is
    /// owed: then no longer, and counted in flight until `owed_now`.
    pub(super) fn owed_taken(&self, frame: &Frame) -> bool {
        let owed = frame.owed.swap(false, Ordering::AcqRel);
        if owed {
            self.owed.in_flight.fetch_add(1, Ordering::AcqRel);
        }
        owed
    }

    /// Captures page `key`, owed, as `content`, its frame claimed by an
    /// eviction (`owed_taken`); a failure noted for the walk, and the page
    /// written back instead, so the pool loses nothing.
    pub(super) fn owed_now(&self, key: Key, content: &[u8]) -> Result<()> {
        let plan = self.owed.plan.read().unwrap_or_else(PoisonError::into_inner).clone();
        let captured = match plan {
            Some((_, capturing)) => self.pager(key.file).and_then(|pager| pager.captured(PageId(key.id), content, &capturing)),
            None => Err(SqlError::new(Code::Api, "[DESIGN] RULES.md D590: a page owed with no capture under way")),
        };
        let outcome = match captured {
            Ok(()) => Ok(()),
            Err(e) => {
                lock_builtin(&self.owed.failure).get_or_insert(e);
                self.write_back(key, content)
            }
        };
        self.owed.in_flight.fetch_sub(1, Ordering::AcqRel);
        outcome
    }
}

// Unit tests: pages owed taken by writers, frees, evictions and the walk,
// each as it was, after a crash too (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
