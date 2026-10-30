//! \[DESIGN\] What the server tells of itself (RULES.md D40, D42), through
//! the database it serves (`Database::tell`): failed logins, each, as an
//! audit of who tried to connect; and what may come in floods, failed
//! accepts and connections refused, counted and told at most once in
//! `QUIET`, so a flood does not flood the log.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use turborel::db::Database;
use turborel::diagnostics::Event;

use crate::auth::{Authenticator, Exchange, Peer, Step};

/// \[DESIGN\] How long after telling of a flood the next is told, as the
/// database's own `FilesReopened` and `CheckpointOverdue` are.
pub(crate) const QUIET: Duration = Duration::from_secs(60);

/// \[DESIGN\] Occurrences counted, told at most once in `QUIET`: the first
/// at once, the rest with the first after `QUIET` has passed.
#[derive(Default)]
pub(crate) struct Throttle {
    /// Occurrences not yet told.
    count: AtomicU64,
    /// When last told.
    told: Mutex<Option<Instant>>,
}

impl Throttle {
    /// One more, at `now`: the occurrences to tell of, this among them, if
    /// it is time to.
    pub(crate) fn one(&self, now: Instant) -> Option<u64> {
        self.count.fetch_add(1, Ordering::Relaxed);
        // Only an instant is kept, so a lock a panic poisoned is whole.
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        let due = told.is_none_or(|at| now.saturating_duration_since(at) >= QUIET);
        due.then(|| {
            *told = Some(now);
            self.count.swap(0, Ordering::Relaxed)
        })
    }
}

/// \[DESIGN\] An authenticator whose refusals are told: the user named,
/// the peer and the mechanism, and why, never what was proved by.
pub(crate) struct Told {
    pub(crate) inner: std::sync::Arc<dyn Authenticator>,
    pub(crate) db: Database,
}

impl Authenticator for Told {
    fn mechanism(&self) -> &'static str {
        self.inner.mechanism()
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        let login = Login { user: user.to_owned(), peer: peer.name(), mechanism: self.inner.mechanism() };
        Box::new(Watched { inner: self.inner.begin(user, peer), db: self.db.clone(), login })
    }
}

/// Who is logging in, to tell if refused.
struct Login {
    user: String,
    peer: String,
    mechanism: &'static str,
}

/// An exchange whose refusal is told.
struct Watched {
    inner: Box<dyn Exchange>,
    db: Database,
    login: Login,
}

impl Exchange for Watched {
    fn step(&mut self, input: &[u8]) -> Step {
        let step = self.inner.step(input);
        if let Step::Refuse(why) = &step {
            let Login { user, peer, mechanism } = &self.login;
            let (user, peer, mechanism) = (user.clone(), peer.clone(), (*mechanism).to_owned());
            self.db.tell(&Event::LoginFailed { user, peer, mechanism, message: why.clone() });
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D40: the first occurrence is told at
    /// once; those within `QUIET` after are counted, not told; the first
    /// after it is told with them all; then quiet again.
    #[test]
    fn a_flood_is_told_once_a_minute() {
        let throttle = Throttle::default();
        let start = Instant::now();
        let at = |seconds: u64| start.checked_add(Duration::from_secs(seconds)).unwrap();
        assert_eq!(throttle.one(at(0)), Some(1_u64));
        assert_eq!(throttle.one(at(1)), None);
        assert_eq!(throttle.one(at(59)), None);
        assert_eq!(throttle.one(at(60)), Some(3_u64));
        assert_eq!(throttle.one(at(61)), None);
        assert_eq!(throttle.one(at(200)), Some(2_u64));
    }

    /// Verifies: [DESIGN] RULES.md D40, D43: an authenticator told of is of
    /// its mechanism still, and steps as it does: its acceptances untold.
    #[test]
    fn an_authenticator_told_of_is_itself() {
        let told = Told { inner: std::sync::Arc::new(crate::auth::Trust), db: Database::new() };
        assert_eq!(told.mechanism(), crate::auth::TRUST);
        assert_eq!(told.begin("X", &Peer::local()).step(b""), Step::Accept("X".into()));
    }
}
