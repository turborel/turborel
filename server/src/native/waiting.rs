//! \[DESIGN\] How long a worker waits on its client for the next request,
//! in slices (RULES.md D839). Any worker waits `lent::WAITED` for a fast
//! client. A connection's home then waits on, as long as `HOME_WAITED`, a
//! slice of `lent::LOOKED` at a time, looking between slices whether the
//! operator ended the connection or the server stopped, so a client that
//! thinks between its requests is read by the thread that runs them and
//! never by the runtime's, which would cost two moves between threads
//! and two of its socket between the runtime and the thread at each
//! request. The wait ends sooner than SERVER.IDLE_TIMEOUT_MS, and
//! SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS while a transaction is active,
//! would end the connection, so the task that takes the client back ends
//! it as it would have.

use std::time::{Duration, Instant};

use super::lent::{LOOKED, WAITED};

/// \[DESIGN\] How long a home waits on its client before the connection's
/// task does. A client that thinks longer pays the two moves between
/// threads once a second at most, a few tens of microseconds, under a
/// ten-thousandth of its time; a home waiting so is woken once each
/// `LOOKED`, fifty times in the second, each a couple of microseconds.
/// Waiting longer gains nothing measurable and keeps a thread from the
/// pool longer. Fixed, not a setting (RULES.md D36): the shape of a
/// hand-over between threads, which no work of the server's tunes.
pub(super) const HOME_WAITED: Duration = Duration::from_secs(1);

/// A worker's wait on its client, as it goes.
pub(super) struct Waiting {
    began: Instant,
    /// Whether the worker is the connection's home.
    homed: bool,
    /// How long the home may wait, read once `WAITED` has passed, so a
    /// fast client's request reads no setting.
    bound: Option<Duration>,
}

impl Waiting {
    /// A wait begun at `began`, of a home if `homed`.
    pub(super) fn new(homed: bool, began: Instant) -> Waiting {
        Waiting { began, homed, bound: None }
    }

    /// Whether `WAITED` has passed, so the client is slow and the worker
    /// gives up its permit to run statements.
    pub(super) fn slow(&self) -> bool {
        self.began.elapsed() >= WAITED
    }

    /// How long the next read waits: what is left of `WAITED`, then a
    /// home's slices; none once the wait is over, the bound reached or
    /// the connection's task to take the client back (`leaving`). The
    /// bound is asked of `bound` once, as the client turns slow.
    pub(super) fn slice(&mut self, leaving: impl FnOnce() -> bool, bound: impl FnOnce() -> Duration) -> Option<Duration> {
        let gone = self.began.elapsed();
        if gone < WAITED {
            return Some(WAITED.saturating_sub(gone));
        }
        if !self.homed || leaving() {
            return None;
        }
        let bound = *self.bound.get_or_insert_with(bound);
        let left = bound.saturating_sub(gone);
        (!left.is_zero()).then(|| left.min(LOOKED))
    }
}

/// The longest a home waits: `HOME_WAITED`, or less as the limits that
/// apply, SERVER.IDLE_TIMEOUT_MS's and, while a transaction is active,
/// SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS's, say; none of either is no
/// limit.
pub(super) fn bounded(idle: Option<Duration>, in_transaction: Option<Duration>) -> Duration {
    [idle, in_transaction].into_iter().flatten().fold(HOME_WAITED, Duration::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D839: a home waits `HOME_WAITED`, or
    /// less as either idle limit says, the shorter of the two if both
    /// apply; a limit longer than `HOME_WAITED` changes nothing.
    #[test]
    fn a_home_waits_no_longer_than_an_idle_limit() {
        let ms = Duration::from_millis;
        assert_eq!(bounded(None, None), HOME_WAITED);
        assert_eq!(bounded(Some(ms(200)), None), ms(200));
        assert_eq!(bounded(None, Some(ms(300))), ms(300));
        assert_eq!(bounded(Some(ms(200)), Some(ms(50))), ms(50));
        assert_eq!(bounded(Some(HOME_WAITED.saturating_mul(5)), None), HOME_WAITED);
    }

    /// A wait of a home if `homed`, begun `ago`.
    fn begun(homed: bool, ago: Duration) -> Waiting {
        Waiting::new(homed, Instant::now().checked_sub(ago).unwrap())
    }

    /// Verifies: [DESIGN] RULES.md D42, D839: within `WAITED` any worker
    /// waits what is left of it; past it a home waits on in slices of
    /// `LOOKED` at most, to its bound, and any other worker waits no more.
    /// MC/DC pairs of the end of a home's wait, from a slice given: `homed`
    /// false ends it; `leaving` true ends it.
    #[test]
    fn a_home_waits_on_in_slices_and_another_worker_does_not() {
        let fresh = begun(false, Duration::ZERO).slice(|| false, || HOME_WAITED);
        assert!(fresh.is_some_and(|s| s <= WAITED), "{fresh:?}");
        let mut home = begun(true, WAITED);
        assert_eq!(home.slice(|| false, || HOME_WAITED), Some(LOOKED));
        assert_eq!(home.slice(|| false, || Duration::ZERO), Some(LOOKED), "the bound kept, not asked again");
        assert_eq!(begun(false, WAITED).slice(|| false, || HOME_WAITED), None, "away from home");
        assert_eq!(begun(true, WAITED).slice(|| true, || HOME_WAITED), None, "the task takes the client back");
    }

    /// Verifies: [DESIGN] RULES.md D839: a home's last slice ends at its
    /// bound, and a wait past it is over.
    #[test]
    fn a_homes_wait_ends_at_its_bound() {
        let ms = Duration::from_millis;
        let near = begun(true, ms(95)).slice(|| false, || ms(100));
        assert!(near.is_some_and(|s| s <= ms(5)), "{near:?}");
        assert_eq!(begun(true, ms(120)).slice(|| false, || ms(100)), None);
    }

    /// Verifies: [DESIGN] RULES.md D839: a wait is slow, its permit given
    /// up, once `WAITED` has passed since it began.
    #[test]
    fn a_wait_is_slow_once_waited_has_passed() {
        let past = Waiting::new(true, Instant::now().checked_sub(WAITED).unwrap());
        assert!(past.slow());
        let ahead = Waiting::new(true, Instant::now().checked_add(Duration::from_secs(60)).unwrap());
        assert!(!ahead.slow());
    }
}
