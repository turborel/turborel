//! \[DESIGN\] An eviction policy programmed from outside the pool (RULES.md
//! D283, D284): a policy of weights and a small table of priorities,
//! which what programs it pushes now and then, and which the pool's own
//! evictor (`Weighted`) reads with no lock and no call as it chooses.
//!
//! Each frame keeps the interval its page was last used in, the one
//! before, how many intervals it was used in, and the weight the table
//! gives its page. An interval is a page brought into the pool, so a page
//! used often between misses is written once. A victim is the frame of
//! the greatest score among a sample of the frames, the score
//!
//! ```text
//! (age × RECENCY + ONCE if used in one interval alone − uses × FREQUENCY) × weight ÷ 100
//! ```
//!
//! its age taken from its last use (HISTORY 1, LRU) or the use before
//! (HISTORY 2, LRU-2, a page used once being the oldest). ONCE puts pages
//! a scan read once before those used again, as 2Q's first queue does, and
//! FREQUENCY keeps pages used often, as ARC's second list does. A weight
//! of 0 is a hint to keep the page: such a frame is chosen only when no
//! other is. A frame chosen while a policy is replaced may be weighed by
//! either, which costs a choice, never a page: the pool checks every
//! proposal itself.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{Eviction, Key};

/// The most ranges of a policy's table.
pub const RANGES: usize = 64;
/// The frames looked at for one victim when a policy names none: enough
/// to find an old page, few enough that a miss costs a bounded time.
pub const DEFAULT_SAMPLE: u32 = 32;
/// The weight of a page no range names: its score as computed.
pub const NEUTRAL: u32 = 100;
/// The pages evicted a report holds, the last.
pub const EVICTED_KEPT: usize = 256;

/// A range of pages of one file, and the weight the table gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// The file, as the pool numbers it.
    pub file: u32,
    /// The first page.
    pub first: u64,
    /// The last page.
    pub last: u64,
    /// Its weight, in hundredths: 0 a hint to keep, more than 100 sooner
    /// evicted.
    pub weight: u32,
}

/// A policy, as what programs the pool pushes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// Which use a page's age is counted from: 1 its last, 2 the one before.
    pub history: u8,
    /// The weight of a page's age.
    pub recency: u32,
    /// What each interval a page was used in takes off its score.
    pub frequency: u32,
    /// What a page used in one interval alone adds to its score.
    pub once: u32,
    /// The frames looked at for one victim; 0 for `DEFAULT_SAMPLE`.
    pub sample: u32,
    /// The table of priorities, the first range holding a page giving its
    /// weight.
    pub ranges: Vec<Range>,
}

impl Default for Policy {
    /// Least recently used, by intervals: what a pool evicts by until a
    /// policy is pushed.
    fn default() -> Policy {
        Policy { history: 1, recency: 1, frequency: 0, once: 0, sample: 0, ranges: Vec::new() }
    }
}

/// A frame as a report shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampled {
    /// Its page's file, as the pool numbers it.
    pub file: u32,
    /// Its page.
    pub page: u64,
    /// The intervals it was used in.
    pub uses: u32,
    /// The intervals since its last use.
    pub age: u64,
    /// The weight the table gives it.
    pub weight: u32,
}

/// What the pool reports of itself to what programs its policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The interval now: pages brought in since the pool was made.
    pub interval: u64,
    /// Its frames.
    pub frames: u64,
    /// The pages last evicted, file and page, the last at most
    /// `EVICTED_KEPT`, those since the last report.
    pub evicted: Vec<(u32, u64)>,
    /// Frames holding a page, a window of them moving on each report.
    pub sampled: Vec<Sampled>,
}

/// The policy as the evictors read it: each field atomic, so read with no
/// lock, and replaced whole under `writing`.
#[derive(Debug)]
pub struct Table {
    history: AtomicU32,
    recency: AtomicU32,
    frequency: AtomicU32,
    once: AtomicU32,
    sample: AtomicU32,
    ranges: Mutex<Arc<Vec<Range>>>,
    /// Bumped as each policy is pushed.
    generation: AtomicU64,
}

impl Default for Table {
    fn default() -> Table {
        let table = Table {
            history: AtomicU32::new(0),
            recency: AtomicU32::new(0),
            frequency: AtomicU32::new(0),
            once: AtomicU32::new(0),
            sample: AtomicU32::new(0),
            ranges: Mutex::new(Arc::new(Vec::new())),
            generation: AtomicU64::new(0),
        };
        table.push(&Policy::default());
        table
    }
}

impl Table {
    /// `policy` in place of the last; its ranges past `RANGES` dropped.
    pub fn push(&self, policy: &Policy) {
        self.history.store(u32::from(policy.history.clamp(1, 2)), Ordering::Relaxed);
        self.recency.store(policy.recency, Ordering::Relaxed);
        self.frequency.store(policy.frequency, Ordering::Relaxed);
        self.once.store(policy.once, Ordering::Relaxed);
        self.sample.store(policy.sample, Ordering::Relaxed);
        let ranges: Vec<Range> = policy.ranges.iter().take(RANGES).copied().collect();
        *crate::sync::lock_builtin(&self.ranges) = Arc::new(ranges);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// The policy now.
    pub fn policy(&self) -> Policy {
        let get = |a: &AtomicU32| a.load(Ordering::Relaxed);
        Policy {
            history: u8::try_from(get(&self.history)).unwrap_or(1),
            recency: get(&self.recency),
            frequency: get(&self.frequency),
            once: get(&self.once),
            sample: get(&self.sample),
            ranges: crate::sync::lock_builtin(&self.ranges).as_ref().clone(),
        }
    }

    /// How many policies were pushed.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The weight its ranges give `page` of `file`.
    fn weight(&self, file: u32, page: u64) -> u32 {
        let ranges = Arc::clone(&crate::sync::lock_builtin(&self.ranges));
        let held = ranges.iter().find(|r| r.file == file && r.first <= page && page <= r.last);
        held.map_or(NEUTRAL, |r| r.weight)
    }
}

/// What one frame keeps.
#[derive(Debug, Default)]
struct Slot {
    /// Its page's file, plus 1; 0 if it holds none.
    file: AtomicU32,
    page: AtomicU64,
    last: AtomicU64,
    before: AtomicU64,
    uses: AtomicU32,
    weight: AtomicU32,
}

/// \[DESIGN\] The evictor of a pool whose policy is pushed (`Table`): it
/// asks nothing of anyone as it chooses, and reports itself when asked
/// (`report`), off every read.
#[derive(Debug)]
pub struct Weighted {
    slots: Vec<Slot>,
    table: Arc<Table>,
    interval: AtomicU64,
    hand: AtomicUsize,
    /// Where `report` samples from next.
    window: AtomicUsize,
    /// The pages evicted, a ring of `EVICTED_KEPT`, and how many were.
    evicted: Vec<(AtomicU32, AtomicU64)>,
    evictions: AtomicUsize,
    reported: AtomicUsize,
}

impl Weighted {
    /// The evictor of a pool of `frames` frames, reading `table`.
    pub fn new(frames: usize, table: Arc<Table>) -> Weighted {
        Weighted {
            slots: (0..frames).map(|_| Slot::default()).collect(),
            table,
            interval: AtomicU64::new(1),
            hand: AtomicUsize::new(0),
            window: AtomicUsize::new(0),
            evicted: (0..EVICTED_KEPT).map(|_| (AtomicU32::new(0), AtomicU64::new(0))).collect(),
            evictions: AtomicUsize::new(0),
            reported: AtomicUsize::new(0),
        }
    }

    /// Weighs every frame again by the table, as a policy was pushed.
    pub fn reweigh(&self) {
        for slot in &self.slots {
            if let Some(file) = slot.file.load(Ordering::Relaxed).checked_sub(1) {
                slot.weight.store(self.table.weight(file, slot.page.load(Ordering::Relaxed)), Ordering::Relaxed);
            }
        }
    }

    /// Its report, `most` frames sampled, the pages evicted since the last.
    pub fn report(&self, most: usize) -> Report {
        let now = self.interval.load(Ordering::Relaxed);
        let frames = self.slots.len();
        let start = self.window.fetch_add(most, Ordering::Relaxed);
        let sampled = (0..most.min(frames))
            .filter_map(|i| self.slots.get(start.wrapping_add(i).checked_rem(frames)?))
            .filter_map(|s| {
                let file = s.file.load(Ordering::Relaxed).checked_sub(1)?;
                let age = now.saturating_sub(s.last.load(Ordering::Relaxed));
                let (page, uses, weight) =
                    (s.page.load(Ordering::Relaxed), s.uses.load(Ordering::Relaxed), s.weight.load(Ordering::Relaxed));
                Some(Sampled { file, page, uses, age, weight })
            })
            .collect();
        Report { interval: now, frames: u64::try_from(frames).unwrap_or(u64::MAX), evicted: self.evicted_since(), sampled }
    }

    /// The pages evicted since the last report, at most `EVICTED_KEPT`.
    fn evicted_since(&self) -> Vec<(u32, u64)> {
        let (now, before) = (self.evictions.load(Ordering::Acquire), self.reported.swap(0, Ordering::AcqRel));
        let from = now.saturating_sub(EVICTED_KEPT).max(before.min(now));
        self.reported.store(now, Ordering::Release);
        (from..now)
            .filter_map(|n| self.evicted.get(n.checked_rem(EVICTED_KEPT)?))
            .map(|(f, p)| (f.load(Ordering::Relaxed), p.load(Ordering::Relaxed)))
            .collect()
    }

    /// The score of `slot`, and whether its weight hints to keep it.
    fn score(slot: &Slot, (now, policy): (u64, &Scoring)) -> (bool, u64) {
        let stamp = if policy.history == 2 { slot.before.load(Ordering::Relaxed) } else { slot.last.load(Ordering::Relaxed) };
        let age = now.saturating_sub(stamp);
        let uses = slot.uses.load(Ordering::Relaxed);
        let once = if uses <= 1 { u64::from(policy.once) } else { 0 };
        let kept = u64::from(uses).saturating_mul(u64::from(policy.frequency));
        let raw = age.saturating_mul(u64::from(policy.recency)).saturating_add(once).saturating_sub(kept);
        let weight = slot.weight.load(Ordering::Relaxed);
        (weight == 0, raw.saturating_mul(u64::from(weight)).checked_div(u64::from(NEUTRAL)).unwrap_or(raw))
    }
}

/// The parts of the policy one choice reads, read once for it.
struct Scoring {
    history: u32,
    recency: u32,
    frequency: u32,
    once: u32,
}

impl Eviction for Arc<Weighted> {
    fn name(&self) -> &'static str {
        "weighted"
    }

    fn accessed(&self, frame: usize) {
        let Some(slot) = self.slots.get(frame) else { return };
        let now = self.interval.load(Ordering::Relaxed);
        let last = slot.last.load(Ordering::Relaxed);
        // Written once an interval, so a page used on many cores between
        // misses does not move its cache line between them on every use.
        if last != now && slot.last.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            slot.before.store(last, Ordering::Relaxed);
            slot.uses.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn placed(&self, frame: usize, key: Key) {
        let Some(slot) = self.slots.get(frame) else { return };
        if let Some(old) = slot.file.load(Ordering::Relaxed).checked_sub(1) {
            let n = self.evictions.fetch_add(1, Ordering::AcqRel);
            if let Some((f, p)) = n.checked_rem(EVICTED_KEPT).and_then(|at| self.evicted.get(at)) {
                f.store(old, Ordering::Relaxed);
                p.store(slot.page.load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
        // An interval begins with each page brought in.
        let now = self.interval.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        slot.file.store(key.file.0.saturating_add(1), Ordering::Relaxed);
        slot.page.store(key.id, Ordering::Relaxed);
        slot.weight.store(self.table.weight(key.file.0, key.id), Ordering::Relaxed);
        slot.before.store(0, Ordering::Relaxed);
        slot.uses.store(1, Ordering::Relaxed);
        slot.last.store(now, Ordering::Relaxed);
    }

    fn victim(&self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        let frames = self.slots.len();
        let get = |a: &AtomicU32| a.load(Ordering::Relaxed);
        let t = &self.table;
        let scoring = Scoring { history: get(&t.history), recency: get(&t.recency), frequency: get(&t.frequency), once: get(&t.once) };
        let sample = match get(&t.sample) {
            0 => DEFAULT_SAMPLE,
            n => n,
        };
        let looked = usize::try_from(sample).unwrap_or(usize::MAX).min(frames);
        let start = self.hand.fetch_add(looked, Ordering::Relaxed);
        let now = self.interval.load(Ordering::Relaxed);
        let mut best: Option<(bool, u64, usize)> = None;
        for frame in (0..looked).filter_map(|i| start.wrapping_add(i).checked_rem(frames)) {
            let Some(slot) = self.slots.get(frame).filter(|_| evictable(frame)) else { continue };
            let (keep, score) = Weighted::score(slot, (now, &scoring));
            let better = best.is_none_or(|(k, s, _)| (k && !keep) || (k == keep && score > s));
            if better {
                best = Some((keep, score, frame));
            }
        }
        best.map(|(_, _, frame)| frame)
    }
}

/// What makes the evictor of a pool of so many frames.
pub type MakeEviction = Arc<dyn Fn(usize) -> Box<dyn Eviction> + Send + Sync>;

/// \[DESIGN\] What a database's pools are made to evict by (RULES.md
/// D283): CLOCK, unless a host plug-in's policy was set before the pool
/// is made.
#[derive(Default)]
pub struct EvictionSlot(std::sync::RwLock<Option<MakeEviction>>);

impl std::fmt::Debug for EvictionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvictionSlot").finish_non_exhaustive()
    }
}

impl EvictionSlot {
    /// Pools made after evict by what `make` makes.
    pub fn set(&self, make: MakeEviction) {
        *self.0.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(make);
    }

    /// The evictor of a pool of `frames` frames.
    pub fn make(&self, frames: usize) -> Box<dyn Eviction> {
        let held = self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let Some(make) = held else { return Box::new(super::Clock::new(frames)) };
        make(frames)
    }
}

// Unit tests: the evictor's choices by its policy (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
