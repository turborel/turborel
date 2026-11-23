//! \[DESIGN\] Reads that hit, writing one thing shared (RULES.md D30): the
//! latch of the frame read. A table of hints, atomic words read without a
//! lock, says where each page's frame was last found; the frame is latched
//! shared without waiting, and checked to hold the page and not be filling.
//! The latch keeps the frame from eviction, which takes it exclusively
//! (`pool.rs`), so no pin is taken: an eviction takes a frame so latched
//! as busy for a moment, waited for a bounded while (`pool/claim.rs`). A
//! hit is counted on a stripe of the counters by thread, each on its own
//! cache line.
//!
//! A hint is only a guess, written as the locked way finds or fills a page:
//! one stale, or a frame latched exclusively, sends the read the locked way.
//!
//! A page held to be written or owned is pinned the same way (RULES.md
//! D854): its frame found by its hint and latched shared without waiting,
//! the pin taken under that latch once the frame is seen to hold the page,
//! and the latch let go. No shard is locked and no map searched. What
//! takes a frame from its page latches it exclusively first and then looks
//! at its pins (`Frame::taken`), so it sees a pin taken under the latch,
//! and none is taken while it holds the latch.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLockWriteGuard, TryLockError};

use super::{BufferPool, FileId, Frame, Key, PageRef, Pin, count};
use crate::error::Result;
use crate::sync::thread_stripe;

/// \[DESIGN\] Stripes of the hit counters: more than the threads that read
/// at once, so they seldom share one.
const STRIPES: usize = 64;

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

    /// The hash a page is placed by: XXH3-64 of its file and number
    /// (RULES.md D460), as its shard's.
    #[inline]
    pub(super) fn hash(key: Key) -> u64 {
        crate::hash::xxhash::Words::new(0).pair(u64::from(key.file.0), key.id)
    }

    /// The slot of the page whose hash is `hash`: its top bits.
    #[inline]
    fn slot_at(&self, hash: u64) -> Option<&AtomicU64> {
        self.slots.get(usize::try_from(hash.checked_shr(self.shift).unwrap_or_default()).unwrap_or_default())
    }

    /// The slot of `key`.
    #[inline]
    fn slot(&self, key: Key) -> Option<&AtomicU64> {
        self.slot_at(Hints::hash(key))
    }

    /// Notes `key` found in frame `index`. The slot is written only when
    /// it says otherwise: a page held again and again, as every write
    /// holds its page, is found where it was, and a store of the same
    /// frame would still take the slot's cache line from the threads
    /// reading the hints beside it.
    pub(super) fn note(&self, key: Key, index: usize) {
        let noted = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        if let Some(slot) = self.slot(key).filter(|slot| slot.load(Ordering::Relaxed) != noted) {
            slot.store(noted, Ordering::Relaxed);
        }
    }

    /// The frame the page whose hash is `hash` was last found in, if any.
    #[inline]
    pub(super) fn frame_at(&self, hash: u64) -> Option<usize> {
        let hinted = self.slot_at(hash)?.load(Ordering::Relaxed);
        usize::try_from(hinted.checked_sub(1)?).ok()
    }

    /// The frame `key` was last found in, if any.
    #[inline]
    pub(super) fn frame_of(&self, key: Key) -> Option<usize> {
        self.frame_at(Hints::hash(key))
    }

    /// Counts a hit, on the thread's stripe.
    #[inline]
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

/// Why a frame was not taken from its page.
pub(super) enum Kept {
    /// Its latch is held, for as long as one read of its page takes.
    Latched,
    /// It is pinned, for as long as its holder likes.
    Pinned,
}

impl Frame {
    /// Its content latched exclusively, its version moved on, to take the
    /// frame from the page it holds: only if no one holds its latch and,
    /// seen under the latch, no one pins it. A pin taken through a hint is
    /// taken under the latch held shared (`BufferPool::pin_hinted`), so it
    /// is seen here, and none is taken while this latch is held.
    pub(super) fn taken(&self) -> std::result::Result<RwLockWriteGuard<'_, Vec<u8>>, Kept> {
        let latch = self.try_latched().ok_or(Kept::Latched)?;
        if self.pins.load(Ordering::Acquire) > 0 {
            return Err(Kept::Pinned);
        }
        Ok(latch)
    }
}

impl BufferPool {
    /// The frame holding page `key`, pinned, if its hint finds it at once,
    /// holding the page and not filling: the pin taken under its latch
    /// held shared, which is then let go (RULES.md D854). The eviction
    /// policy is told once the latch is let go, no lock held (R-ERR-4).
    pub(super) fn pin_hinted(&self, key: Key) -> Option<&Frame> {
        let index = self.hints.frame_of(key)?;
        let frame = self.frames.get(index)?;
        {
            let _latch = match frame.content.try_read() {
                Ok(latch) => latch,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return None,
            };
            if frame.loading.load(Ordering::Acquire) || frame.key() != Some(key) {
                return None;
            }
            frame.pins.fetch_add(1, Ordering::AcqRel);
        }
        self.hints.hit();
        self.eviction.accessed(index);
        Some(frame)
    }

    /// Page or block `key`, whose hash is `hash` (`page_hash`), held for
    /// reading if its hint finds it at once:
    /// its frame latched shared, holding it, not filling. The eviction
    /// policy is told of the use of the frame hinted before the latch is
    /// taken, no lock held (RULES.md R-ERR-4): for a hint gone stale, of a
    /// frame not used, which its contract allows.
    /// The hash is given by a caller that places the page by it too, so it
    /// is made once a read (RULES.md D951).
    #[inline]
    pub(super) fn read_hinted_at(&self, key: Key, hash: u64) -> Option<PageRef<'_>> {
        let index = self.hints.frame_at(hash)?;
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
        Some(PageRef { content, frame, _pin: None })
    }

    /// \[DESIGN\] The hash the pool places page `id` of `file` by, for a
    /// caller that keeps its own note of the page in a place of the same
    /// hash and reads it through `read_resident_at` or `read_stamped_at`,
    /// so the page is hashed once a read (RULES.md D951).
    #[inline]
    #[must_use]
    pub fn page_hash(file: FileId, id: u64) -> u64 {
        Hints::hash(Key { file, id })
    }

    /// \[DESIGN\] Page `id` of pager file `file`, held for reading if a frame
    /// holds it now; none if none does, and nothing read from the file: for
    /// a page whose number is only a guess, which may be free (RULES.md
    /// D500, `engine/tree/near.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the pool's maps are missing a shard, which `new`
    /// prevents.
    pub fn read_resident(&self, file: FileId, id: u64) -> Result<Option<PageRef<'_>>> {
        self.read_resident_at(file, id, BufferPool::page_hash(file, id))
    }

    /// \[DESIGN\] As `read_resident`, of the page whose hash is `hash`
    /// (`page_hash`).
    ///
    /// # Errors
    ///
    /// As `read_resident`.
    pub fn read_resident_at(&self, file: FileId, id: u64, hash: u64) -> Result<Option<PageRef<'_>>> {
        let key = Key { file, id };
        if let Some(page) = self.read_hinted_at(key, hash) {
            return Ok(Some(page));
        }
        let Some(pinned) = self.pin_mapped(key)? else { return Ok(None) };
        let Some(frame) = self.settled_holding(pinned, key)? else { return Ok(None) };
        self.hints.note(key, pinned.0);
        let pin = Pin(frame);
        Ok(Some(PageRef { content: frame.content.read().unwrap_or_else(PoisonError::into_inner), frame, _pin: Some(pin) }))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::disk::buffer::{Clock, FileId};
    use crate::disk::encoding::Pipeline;
    use crate::disk::memory::MemoryFileSystem;
    use crate::disk::pager::Pager;
    use crate::error::Code;
    use crate::hash::HashFunction;
    use crate::hash::crc::{CRC32_ISCSI, Crc};

    /// Verifies: [DESIGN] RULES.md D854: a page held is pinned through its
    /// hint, and its frame then cannot be taken from it, seen pinned under
    /// its latch, so a pool of one frame refuses another page (SQLCODE
    /// -918) until the pin is let go; a hint gone stale, its frame holding
    /// another page, pins nothing; and a frame latched is not taken.
    #[test]
    fn a_page_pinned_through_its_hint_keeps_its_frame() {
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Pager::create(&MemoryFileSystem::new(), Path::new("/h"), hash, Pipeline::none(), 4096).unwrap();
        let pool = BufferPool::new(1, Box::new(Clock::new(1))).unwrap();
        let file = pool.add_pager(Arc::new(pager));
        let a = pool.new_page(file).map(|(id, _)| id).unwrap();
        let key = Key { file, id: a.0 };
        let frame = pool.pin_hinted(key).unwrap();
        assert_eq!(frame.pins.load(Ordering::Acquire), 1, "pinned, and nothing else holds it");
        assert!(matches!(frame.taken(), Err(Kept::Pinned)), "seen pinned under its latch");
        assert_eq!(pool.new_page(file).err().map(|e| e.code), Some(Code::Resources), "its frame kept");
        let held = frame.content.read().unwrap();
        assert!(matches!(frame.taken(), Err(Kept::Latched)));
        drop(held);
        drop(Pin(frame));
        assert!(frame.taken().is_ok(), "let go");
        let b = pool.new_page(file).map(|(id, _)| id).unwrap();
        assert_eq!(pool.hints.frame_of(key), Some(0), "the hint of the page evicted still names the frame");
        assert!(pool.pin_hinted(key).is_none(), "which holds another page now");
        let again = pool.pin_hinted(Key { file, id: b.0 }).unwrap();
        assert_eq!(again.index, 0);
        drop(Pin(again));
    }

    /// Verifies: [DESIGN] RULES.md D854: writers that own pages, pinned
    /// through their hints, keep their frames while readers of other
    /// pages evict through a pool far smaller than the file, and while
    /// pages are made and freed beside them. Each page names itself and
    /// counts its writes; a frame taken from an owner would show another
    /// page's name under the owner's hands, or lose a count.
    #[test]
    fn owners_pinned_through_hints_keep_their_frames_under_evictions_and_frees() {
        const PAGES: u64 = 48;
        const ROUNDS: u64 = 400;
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Pager::create(&MemoryFileSystem::new(), Path::new("/c"), hash, Pipeline::none(), 4096).unwrap();
        let pool = BufferPool::new(12, Box::new(Clock::new(12))).unwrap();
        let file = pool.add_pager(Arc::new(pager));
        let ids: Vec<_> = (0..PAGES).map(|_| pool.new_page(file).map(|(id, _)| id).unwrap()).collect();
        for id in &ids {
            *pool.write(file, *id).unwrap() = [id.0.to_le_bytes(), 0_u64.to_le_bytes()].concat();
        }
        let word = |page: &[u8], at: usize| u64::from_le_bytes(page[at..at + 8].try_into().unwrap());
        let writer = |n: u64| {
            for round in 0..ROUNDS {
                let id = ids[usize::try_from((n * 7 + round * 5) % PAGES).unwrap()];
                let owned = pool.own(file, id).unwrap();
                let before = word(&owned.read(), 8);
                std::thread::yield_now();
                let mut page = pool.write_owned(&owned).unwrap();
                assert_eq!((word(&page, 0), word(&page, 8)), (id.0, before), "the frame owned holds its page as read");
                page[8..16].copy_from_slice(&(before + 1).to_le_bytes());
            }
        };
        let reader = |n: u64| {
            for round in 0..ROUNDS * 4 {
                let id = ids[usize::try_from((n + round * 11) % PAGES).unwrap()];
                assert_eq!(word(&pool.read(file, id.0).unwrap(), 0), id.0);
            }
        };
        let churn = || {
            for _ in 0..ROUNDS {
                let made = pool.new_page(file).map(|(id, _)| id);
                // Refused while every frame is held, which costs a round.
                if let Ok(id) = made {
                    pool.free_when_released(file, id).unwrap();
                }
            }
        };
        std::thread::scope(|threads| {
            (0..4).for_each(|n| drop(threads.spawn(move || writer(n))));
            (0..3).for_each(|n| drop(threads.spawn(move || reader(n))));
            threads.spawn(churn);
        });
        let written: u64 = ids.iter().map(|id| word(&pool.read(file, id.0).unwrap(), 8)).sum();
        assert_eq!(written, 4 * ROUNDS, "no write lost");
    }

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
