//! \[DESIGN\] Reads that hit, writing one thing shared (RULES.md D30): the
//! latch of the frame read. A table of hints, atomic words read without a
//! lock, says where each page's frame was last found; the frame is latched
//! shared without waiting, and checked to hold the page and not be filling.
//! The latch keeps the frame from eviction, which takes it exclusively
//! (`pool.rs`), so no pin is taken. A hit is counted on a stripe of the
//! counters by thread, each on its own cache line.
//!
//! A hint is only a guess, written as the locked way finds or fills a page:
//! one stale, or a frame latched exclusively, sends the read the locked way.
//! Split from pool.rs (RULES.md R-SIZE-4).

use std::sync::TryLockError;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{BufferPool, Key, PageRef, count};
use crate::sync::thread_stripe;

/// \[DESIGN\] Stripes of the hit counters: more than the threads that read
/// at once, so they seldom share one.
const STRIPES: usize = 64;
/// The multiplier of the hints' Fibonacci hash (as the shards', `pool.rs`).
const HINT_MIX: u64 = 0x9E37_79B9_7F4A_7C15;
const HINT_FILE_MIX: u64 = 0x0100_0000_01B3;

/// A counter alone on its cache lines.
#[repr(align(128))]
#[derive(Default)]
struct Stripe(AtomicU64);

/// The hints of a pool, and its hit counters by stripe.
pub(super) struct Hints {
    /// For each hash, the frame last found for a page of it, plus one; 0
    /// for none.
    slots: Box<[AtomicU64]>,
    /// The bits of a hash kept: the slots are 2 to the 64 less this.
    shift: u32,
    stripes: Box<[Stripe]>,
}

impl Hints {
    /// Hints for a pool of `frames` frames: twice as many slots, rounded up
    /// to a power of two.
    pub(super) fn new(frames: usize) -> Hints {
        let slots = frames.saturating_mul(2).max(2).next_power_of_two();
        let shift = 64_u32.saturating_sub(slots.trailing_zeros());
        Hints { slots: (0..slots).map(|_| AtomicU64::new(0)).collect(), shift, stripes: (0..STRIPES).map(|_| Stripe::default()).collect() }
    }

    /// The slot of `key`.
    fn slot(&self, key: Key) -> Option<&AtomicU64> {
        let mixed = key.id.wrapping_add(u64::from(key.file.0).wrapping_mul(HINT_FILE_MIX)).wrapping_mul(HINT_MIX);
        self.slots.get(usize::try_from(mixed.checked_shr(self.shift).unwrap_or_default()).unwrap_or_default())
    }

    /// Notes `key` found in frame `index`.
    pub(super) fn note(&self, key: Key, index: usize) {
        if let Some(slot) = self.slot(key) {
            slot.store(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1), Ordering::Relaxed);
        }
    }

    /// The frame `key` was last found in, if any.
    fn frame_of(&self, key: Key) -> Option<usize> {
        let hinted = self.slot(key)?.load(Ordering::Relaxed);
        usize::try_from(hinted.checked_sub(1)?).ok()
    }

    /// Counts a hit, on the thread's stripe.
    pub(super) fn hit(&self) {
        if let Some(stripe) = self.stripes.get(thread_stripe(STRIPES)) {
            count(&stripe.0);
        }
    }

    /// The hits counted.
    pub(super) fn hits(&self) -> u64 {
        self.stripes.iter().fold(0_u64, |n, s| n.saturating_add(s.0.load(Ordering::Relaxed)))
    }
}

impl BufferPool {
    /// Page or block `key`, held for reading if its hint finds it at once:
    /// its frame latched shared, holding it, not filling. The eviction
    /// policy is told of the use of the frame hinted before the latch is
    /// taken, no lock held (RULES.md R-ERR-4): for a hint gone stale, of a
    /// frame not used, which its contract allows.
    pub(super) fn read_hinted(&self, key: Key) -> Option<PageRef<'_>> {
        let index = self.hints.frame_of(key)?;
        let frame = self.frames.get(index)?;
        self.eviction.accessed(index);
        let content = match frame.content.try_read() {
            Ok(content) => content,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        if frame.loading.load(Ordering::Acquire) || frame.key() != Some(key) {
            return None;
        }
        self.hints.hit();
        Some(PageRef { content, _pin: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::buffer::FileId;

    /// Verifies: [DESIGN] RULES.md D30: a hint gives the frame last noted
    /// for its page, none before; hits counted on stripes add up.
    #[test]
    fn hints_give_the_frame_last_noted() {
        let hints = Hints::new(10);
        let key = Key { file: FileId(1), id: 7 };
        assert_eq!(hints.frame_of(key), None);
        hints.note(key, 3);
        assert_eq!(hints.frame_of(key), Some(3));
        hints.note(key, 0);
        assert_eq!(hints.frame_of(key), Some(0));
        hints.hit();
        hints.hit();
        assert_eq!(hints.hits(), 2);
        assert_eq!(hints.slots.len(), 32);
    }
}
