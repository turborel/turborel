//! \[DESIGN\] Eviction policies for the buffer pool (RULES.md D30): which
//! frame gives up its page when a page not in the pool is wanted. The
//! policy is an extension point; CLOCK is built in.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// \[DESIGN\] An eviction policy: an extension point (RULES.md D30).
///
/// Contract (RULES.md R-SAFE-3; checked for `Clock` by its unit tests and
/// `test/integration/buffer.rs`):
/// * It is made for a pool of a given number of frames, and only ever
///   proposes one of them.
/// * `victim` proposes only frames `evictable` accepts, and `None` when it
///   finds none. Its proposal may be stale by the time the pool acts on it;
///   the pool checks again and asks again.
/// * Methods may be called from several threads at once and take `&self`:
///   the policy guards its own state. The pool calls them holding no lock;
///   they never call back into the pool (RULES.md R-ERR-4).
/// * `accessed` may be told of a frame not used: one a read hinted, told
///   before the frame is latched and found to hold another page.
pub trait Eviction: Send + Sync {
    /// Its name, for messages.
    fn name(&self) -> &'static str;
    /// Frame `frame` was used: its page read or written.
    fn accessed(&self, frame: usize);
    /// A frame to evict, among those `evictable` accepts.
    fn victim(&self, evictable: &dyn Fn(usize) -> bool) -> Option<usize>;
}

/// \[DESIGN\] CLOCK (Corbató, 1968): an approximation of least recently
/// used. Each frame has a reference bit, set when it is used; a hand sweeps
/// the frames, clearing set bits and evicting the first frame whose bit is
/// already clear. Lock-free: a bit and the hand are atomics.
pub struct Clock {
    referenced: Vec<AtomicBool>,
    hand: AtomicUsize,
}

impl Clock {
    /// The policy for a pool of `frames` frames.
    pub fn new(frames: usize) -> Clock {
        Clock { referenced: (0..frames).map(|_| AtomicBool::new(false)).collect(), hand: AtomicUsize::new(0) }
    }
}

impl Eviction for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }
    fn accessed(&self, frame: usize) {
        // Written only when clear, so a frame used on many cores does not
        // move its cache line between them on every use.
        if let Some(bit) = self.referenced.get(frame).filter(|bit| !bit.load(Ordering::Relaxed)) {
            bit.store(true, Ordering::Relaxed);
        }
    }
    fn victim(&self, evictable: &dyn Fn(usize) -> bool) -> Option<usize> {
        let frames = self.referenced.len();
        // Two sweeps: the first may only clear bits.
        for _ in 0..frames.saturating_mul(2) {
            let frame = self.hand.fetch_add(1, Ordering::Relaxed).checked_rem(frames)?;
            let Some(bit) = self.referenced.get(frame) else { continue };
            if evictable(frame) && !bit.swap(false, Ordering::Relaxed) {
                return Some(frame);
            }
        }
        None
    }
}

// Unit tests: CLOCK's choices (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D30, CLOCK: a frame used since the hand
    /// last passed is spared once; frames `evictable` refuses are never
    /// proposed; with none evictable, or no frames, there is no victim.
    ///
    /// MC/DC of the choice (evictable and not referenced): evictable and
    /// referenced (spared); not evictable and not referenced (skipped);
    /// evictable and not referenced (chosen).
    #[test]
    fn clock_spares_used_frames_once_and_skips_refused_ones() {
        let clock = Clock::new(3);
        clock.accessed(0);
        clock.accessed(7);
        assert_eq!(clock.victim(&|_| true), Some(1), "0 was used: spared, its bit cleared");
        assert_eq!(clock.victim(&|f| f != 2), Some(0), "2 refused; 0's bit was cleared");
        assert_eq!(clock.victim(&|_| false), None);
        assert_eq!(Clock::new(0).victim(&|_| true), None);
        assert_eq!(clock.name(), "clock");
    }
}
