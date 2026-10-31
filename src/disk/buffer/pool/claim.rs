//! \[DESIGN\] Claiming a frame for a page not in the pool (RULES.md D30,
//! D38): an empty one, or one the eviction policy proposes, checked again
//! under the shards of both pages, still unpinned, and latched exclusively
//! without waiting. Split from pool.rs (RULES.md R-SIZE-4).
//!
//! A frame proposed may be held: pinned, or latched by a read that hit by
//! its hint (`hits.rs`), which takes the latch and no pin. A pinned frame
//! is held for as long as its holder likes; a latched one only for as long
//! as one read of the page takes, so it is busy for a moment. A claim tries
//! each frame once; if none could be taken and some were only latched, it
//! sleeps a while and tries again, for a bounded number of rounds, and is
//! refused (SQLCODE -918) when all are pinned, or the latched ones stay
//! latched past the rounds: the latch may be the claiming thread's own.
//! An undo (`Undoing`) is not refused then but waits for a frame, a
//! bounded while too (`all_held`).

use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use super::{Claim, Latch, try_latch};
use crate::disk::buffer::{BufferPool, Key, UNDO_WAITS, all_held, count};
use crate::error::Result;
use crate::hash::murmur::MurmurMap;
use crate::sync::lock_builtin;

/// \[DESIGN\] The first sleep of a thread waiting for frames latched by
/// reads that hit by their hint; each round sleeps twice the last, up to
/// `LATCH_WAIT_MOST`. Short, as such a latch is held for one read.
const LATCH_WAIT_FIRST: Duration = Duration::from_micros(10);
/// \[DESIGN\] The longest sleep of a round.
const LATCH_WAIT_MOST: Duration = Duration::from_millis(1);
/// \[DESIGN\] Rounds a claim waits for latched frames before it is
/// refused: some 20 ms in all, far more than a read of a page takes.
const CLAIM_ROUNDS: u32 = 26;
/// \[DESIGN\] Rounds `free_when_released` waits for a page latched by a
/// read that hit by its hint: some 200 ms in all. Longer than a claim's,
/// as giving up fails the change that frees the page.
pub(super) const FREE_ROUNDS: u32 = 200;
/// \[DESIGN\] Rounds `free_when_released` waits for a page pinned: some
/// 1 s in all, as a reader holding its pin may wait for a frame for
/// another page meanwhile (some 20 ms, `CLAIM_ROUNDS`). Bounded, as the
/// pin may be the caller's own, a programming error that would otherwise
/// wait for ever.
pub(super) const FREE_PIN_ROUNDS: u32 = 1000;

/// Sleeps round `round` of waiting for frames held, if fewer than
/// `rounds` were slept, and counts it; whether it slept.
pub(super) fn latch_wait(round: &mut u32, rounds: u32) -> bool {
    if *round >= rounds {
        return false;
    }
    let factor = 1_u32.checked_shl(*round).unwrap_or(u32::MAX);
    thread::sleep(LATCH_WAIT_FIRST.saturating_mul(factor).min(LATCH_WAIT_MOST));
    *round = round.saturating_add(1);
    true
}

/// The frames a claim found held in this round of proposals, so the
/// eviction policy is not asked for them again, and whether any was only
/// latched. Allocated only when a frame is found held, so a claim that
/// takes the first frame proposed allocates nothing.
#[derive(Default)]
struct Tried {
    frames: Vec<bool>,
    latched: bool,
}

impl Tried {
    /// Whether frame `index` was tried.
    fn has(&self, index: usize) -> bool {
        self.frames.get(index).copied().unwrap_or(false)
    }

    /// Notes frame `index`, of a pool of `frames`, tried: only latched if
    /// `latched`.
    fn note(&mut self, index: usize, frames: usize, latched: bool) {
        if self.frames.is_empty() {
            self.frames.resize(frames, false);
        }
        if let Some(tried) = self.frames.get_mut(index) {
            *tried = true;
        }
        self.latched |= latched;
    }
}

/// What came of trying to evict a frame proposed.
enum Attempt {
    /// Taken for the page.
    Taken(Claim),
    /// The page was mapped by another thread meanwhile.
    Mapped,
    /// Pinned, given to another page, or emptied.
    Held,
    /// Unpinned, but latched by a read that hit by its hint.
    Latched,
    /// No frame was empty: one is to be evicted.
    NoneEmpty,
}

impl BufferPool {
    /// Claims a frame for `key`: an empty one, or one the eviction policy
    /// proposes. `None` if another thread mapped `key` meanwhile.
    pub(super) fn claim(&self, key: Key) -> Result<Option<Claim>> {
        let (mut tried, mut round, mut waited) = (Tried::default(), 0_u32, None);
        loop {
            match self.claim_empty(key)? {
                Attempt::Taken(claim) => return Ok(Some(claim)),
                Attempt::Mapped => return Ok(None),
                Attempt::NoneEmpty | Attempt::Held | Attempt::Latched => {}
            }
            let evictable = |index: usize| {
                !tried.has(index) && self.frames.get(index).is_some_and(|f| f.pins.load(Ordering::Acquire) == 0 && f.key().is_some())
            };
            // Frames only latched are waited for a bounded while; frames
            // all held are refused, or waited for by an undo alone
            // (RULES.md D38, `all_held`). Either way all are tried again.
            let Some(index) = self.eviction.victim(&evictable) else {
                if !(tried.latched && latch_wait(&mut round, CLAIM_ROUNDS)) {
                    all_held(self.frames.len(), &mut waited, UNDO_WAITS)?;
                }
                tried = Tried::default();
                continue;
            };
            match self.evict(key, index)? {
                Attempt::Taken(claim) => return Ok(Some(claim)),
                Attempt::Mapped => return Ok(None),
                Attempt::Held | Attempt::NoneEmpty => tried.note(index, self.frames.len(), false),
                Attempt::Latched => tried.note(index, self.frames.len(), true),
            }
        }
    }

    /// An empty frame claimed for `key`, under its shard: `Taken`, `Mapped`
    /// if `key` is mapped already, or `NoneEmpty`.
    fn claim_empty(&self, key: Key) -> Result<Attempt> {
        let mut map = self.map(key)?;
        if map.contains_key(&key) {
            return Ok(Attempt::Mapped);
        }
        let Some(index) = lock_builtin(&self.empty).pop() else { return Ok(Attempt::NoneEmpty) };
        // An empty frame's latch is held only by a read that found it holds
        // no page, and lets go at once.
        let frame = self.frame(index)?;
        let latch = frame.content.write().unwrap_or_else(PoisonError::into_inner);
        self.take(&mut map, (index, latch), key, None).map(Attempt::Taken)
    }

    /// Tries to take frame `index`, proposed for eviction, for `key`, under
    /// the shards of both pages.
    fn evict(&self, key: Key, index: usize) -> Result<Attempt> {
        let frame = self.frame(index)?;
        let Some(old) = frame.key() else { return Ok(Attempt::Held) };
        let (mut map, mut old_map) = self.maps(key, old)?;
        if map.contains_key(&key) {
            return Ok(Attempt::Mapped);
        }
        // Still the page it held, and unpinned, now that both shards are
        // locked: pins rise only under the shard of the page held.
        if frame.key() != Some(old) || frame.pins.load(Ordering::Acquire) > 0 {
            return Ok(Attempt::Held);
        }
        let Some(latch) = try_latch(frame) else { return Ok(Attempt::Latched) };
        count(&self.counters.evictions);
        let claim = self.take(&mut map, (index, latch), key, Some(old))?;
        // A dirty page stays mapped until written back (`fill`).
        if claim.write_back.is_none() {
            old_map.as_mut().unwrap_or(&mut map).remove(&old);
        }
        Ok(Attempt::Taken(claim))
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
}
