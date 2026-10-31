//! \[DESIGN\] Claiming a frame for a page not in the pool (RULES.md D30,
//! D38): an empty one, or one the eviction policy proposes, checked again
//! under the shards of both pages, still unpinned, and latched exclusively
//! without waiting. Split from pool.rs (RULES.md R-SIZE-4).
//!
//! A frame proposed may be held: pinned, or latched by a read that hit by
//! its hint (`hits.rs`), which takes the latch and no pin. A pinned frame
//! is held for as long as its holder likes; a latched one only for as long
//! as one read of the page takes, so it is busy for a moment. So is an
//! unpinned frame the eviction policy declines to propose: CLOCK spares a
//! frame used since its hand last passed, and frames read over and over,
//! or a hand moved on by other claims meanwhile, can see it sweep the pool
//! twice and propose none. A claim tries each frame once; if none could be
//! taken and some were only busy, it sleeps a while and tries again, and
//! is refused (SQLCODE -918) when all are pinned, or the busy ones stay
//! busy past its wait: the latch may be the claiming thread's own. The
//! wait is bounded by rounds in which the pool did nothing else, no read
//! hitting, missing or evicting, so a thread that is starved of the CPU
//! while it holds a latch, on a busy server, does not get a claim refused
//! while other reads go on; and by a longer while in all, so a latch the
//! claiming thread holds is refused in the end however busy the pool. An
//! undo (`Undoing`) is not refused then but waits for a frame, a bounded
//! while too (`all_held`).

use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

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
/// \[DESIGN\] Rounds a claim waits for busy frames while the pool does
/// nothing else before it is refused: some 20 ms, far more than a read of
/// a page takes.
const CLAIM_ROUNDS: u32 = 26;
/// \[DESIGN\] The longest a claim waits for busy frames, however busy
/// the pool is meanwhile.
const CLAIM_MOST: Duration = Duration::from_millis(500);
/// \[DESIGN\] Rounds `free_when_released` waits for a page latched by a
/// read that hit by its hint: some 200 ms in all. Longer than a claim's,
/// as giving up fails the change that frees the page.
pub(super) const FREE_ROUNDS: u32 = 200;
/// \[DESIGN\] Rounds `free_when_released` waits for a page pinned: some
/// 1 s in all, as a reader holding its pin may wait for a frame for
/// another page meanwhile (up to `CLAIM_MOST`). Bounded, as the
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

/// A claim's wait for frames busy for a moment: its rounds slept, the
/// rounds in which the pool did nothing else, the pool's work last seen
/// (`BufferPool::work`), and when it began.
#[derive(Default)]
struct Busy {
    round: u32,
    stalled: u32,
    seen: Option<u64>,
    since: Option<Instant>,
}

impl Busy {
    /// Sleeps a round, the pool having done `work`, unless the wait is
    /// over: `CLAIM_ROUNDS` rounds in which the pool did nothing else, or
    /// `CLAIM_MOST` in all; whether it slept.
    fn wait(&mut self, work: u64) -> bool {
        let since = *self.since.get_or_insert_with(Instant::now);
        if self.seen.replace(work).is_some_and(|seen| seen != work) {
            self.stalled = 0;
        }
        if self.stalled >= CLAIM_ROUNDS || since.elapsed() >= CLAIM_MOST {
            return false;
        }
        self.stalled = self.stalled.saturating_add(1);
        latch_wait(&mut self.round, u32::MAX)
    }
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
        let (mut tried, mut busy, mut waited) = (Tried::default(), Busy::default(), None);
        loop {
            match self.claim_empty(key)? {
                Attempt::Taken(claim) => return Ok(Some(claim)),
                Attempt::Mapped => return Ok(None),
                Attempt::NoneEmpty | Attempt::Held | Attempt::Latched => {}
            }
            let evictable = |index: usize| {
                !tried.has(index) && self.frames.get(index).is_some_and(|f| f.pins.load(Ordering::Acquire) == 0 && f.key().is_some())
            };
            // Frames only busy, latched or declined by the policy though
            // unpinned, are waited for a bounded while; frames all held are
            // refused, or waited for by an undo alone (RULES.md D38,
            // `all_held`). Either way all are tried again.
            let Some(index) = self.eviction.victim(&evictable) else {
                let declined = (0..self.frames.len()).any(evictable);
                if !((tried.latched || declined) && busy.wait(self.work())) {
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

    /// The work the pool has done: reads that hit, missed, or evicted. A
    /// claim waiting for busy frames waits on while it grows.
    fn work(&self) -> u64 {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        self.hints.hits().wrapping_add(get(&self.counters.misses)).wrapping_add(get(&self.counters.evictions))
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
