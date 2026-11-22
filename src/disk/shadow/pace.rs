//! \[DESIGN\] A commit's writes spread over a span (RULES.md D812): a
//! checkpoint's job asks its commits to take about the span it gives, so
//! the work going on keeps the cores and the drive it would otherwise
//! lose all at once. After each write, the writer waits until the time
//! that share of the span is due. Asked to hurry, as a database closes,
//! it waits no more.

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// \[DESIGN\] The span the next commits are spread over, in microseconds;
/// none (`0`) when they are not, as they are by default.
#[derive(Debug, Default)]
pub(super) struct Spreading(AtomicU64);

impl Spreading {
    /// Spreads the commits from now on over `over`, or, `None`, not.
    pub(super) fn set(&self, over: Option<Duration>) {
        let micros = over.map_or(0, |over| u64::try_from(over.as_micros()).unwrap_or(u64::MAX));
        self.0.store(micros, Ordering::Release);
    }

    /// The span set, `ZERO` for none.
    fn over(&self) -> Duration {
        Duration::from_micros(self.0.load(Ordering::Acquire))
    }
}

/// \[DESIGN\] The pacing of one commit of `total` bytes, begun at `began`.
pub(super) struct Pace<'a> {
    spreading: Option<&'a Spreading>,
    began: Instant,
    total: u64,
    done: u64,
}

impl<'a> Pace<'a> {
    /// The pacing of a commit of `total` bytes, as `spreading` says.
    pub(super) fn new(spreading: &'a Spreading, total: u64) -> Pace<'a> {
        Pace { spreading: Some(spreading), began: Instant::now(), total, done: 0 }
    }

    /// No pacing: a commit made at once.
    pub(super) fn none() -> Pace<'a> {
        Pace { spreading: None, began: Instant::now(), total: 0, done: 0 }
    }

    /// Counts `bytes` written, then waits for their share of the span.
    pub(super) fn wrote(&mut self, bytes: usize) {
        self.done = self.done.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        let Some(spreading) = self.spreading else { return };
        let wait = due(self.done, self.total, spreading.over()).saturating_sub(self.began.elapsed());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }
}

/// \[DESIGN\] When `done` bytes of `total` are due, the commit spread over
/// `over`: their share of it, `over` at most, so a total counted short
/// never waits past the span.
pub(super) fn due(done: u64, total: u64, over: Duration) -> Duration {
    let share = u128::from(done.min(total)).saturating_mul(over.as_micros()).checked_div(u128::from(total)).unwrap_or_default();
    Duration::from_micros(u64::try_from(share).unwrap_or(u64::MAX))
}
