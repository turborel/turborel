//! \[DESIGN\] Logins (RULES.md D42, D43): the checks of authentication,
//! slow on purpose (a password's PBKDF2, D26), run on blocking threads of
//! their own, at most SERVER.MAX_LOGINS at once, apart from the workers
//! statements run on: so clients that never log in cannot keep the
//! statements of those who did waiting.
//!
//! And a client whose logins fail is slowed, by where it connects from,
//! never by the user it names: a user locked out after failures would let
//! anyone lock out anyone. After a failure from an address, its next login
//! waits 100 ms, doubled with each failure after, up to SERVER.LOGIN_DELAY_MS;
//! logins of one address waiting take turns, one a delay, so connections in
//! parallel guess no faster. A login accepted forgets the address's
//! failures, and so do ten minutes without one. An IPv6 address counts by
//! its /64, the least a site is given; clients of a Unix socket count as
//! one address.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use turborel::db::Database;
use turborel::error::Result;
use turborel_protocol::broken;

use crate::auth::{Origin, Peer};
use crate::limit::Limit;
use crate::settings::{LOGIN_DELAY_MS, MAX_LOGINS, count, millis};

/// \[DESIGN\] The wait after an address's first failed login, doubled with
/// each after.
pub const FIRST_DELAY: Duration = Duration::from_millis(100);

/// \[DESIGN\] How long an address's failures are kept after its last.
pub const FORGET_AFTER: Duration = Duration::from_secs(600);

/// \[DESIGN\] The addresses whose failures are kept at most: past them, the
/// one longest without a failure is forgotten, so a flood of addresses
/// holds no more memory than this.
pub const MOST_ADDRESSES: usize = 10_000;

/// The bits of an IPv6 address that name its site: its /64.
const SITE_BITS: u32 = 64;

/// Where logins come from, as their failures are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    /// An address, an IPv6 one's site only.
    Address(IpAddr),
    /// A Unix socket's clients.
    Local,
}

impl Source {
    fn of(peer: &Peer) -> Source {
        match peer.origin {
            Origin::Tcp(address) => match address.ip().to_canonical() {
                IpAddr::V4(v4) => Source::Address(IpAddr::V4(v4)),
                IpAddr::V6(v6) => {
                    let site = u128::from(v6).checked_shr(SITE_BITS).unwrap_or(0).checked_shl(SITE_BITS).unwrap_or(0);
                    Source::Address(IpAddr::V6(Ipv6Addr::from(site)))
                }
            },
            Origin::Local => Source::Local,
        }
    }
}

/// An address's failed logins: how many in a row, and when the next of
/// its logins may be checked.
#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    next: Instant,
}

/// The wait after `failures` failures in a row: `FIRST_DELAY`, doubled
/// with each after the first, at most `most`.
fn delay(failures: u32, most: Duration) -> Duration {
    let doubled = 1_u32.checked_shl(failures.saturating_sub(1)).unwrap_or(u32::MAX);
    FIRST_DELAY.saturating_mul(doubled).min(most)
}

/// Room for one more address among `failed`, as of `now`: those forgotten
/// dropped, and if none was, the one longest without a failure.
fn make_room(failed: &mut HashMap<Source, Failures>, now: Instant) {
    failed.retain(|_, f| now.saturating_duration_since(f.next) <= FORGET_AFTER);
    if failed.len() < MOST_ADDRESSES {
        return;
    }
    if let Some(oldest) = failed.iter().min_by_key(|(_, f)| f.next).map(|(s, _)| *s) {
        failed.remove(&oldest);
    }
}

/// \[DESIGN\] What checks logins: on blocking threads of their own, at
/// most SERVER.MAX_LOGINS at once; their failures counted by address.
#[derive(Default)]
pub struct Logins {
    limit: Arc<Limit>,
    failed: Mutex<HashMap<Source, Failures>>,
}

impl Logins {
    fn failures(&self) -> std::sync::MutexGuard<'_, HashMap<Source, Failures>> {
        // Each entry is replaced whole, so a lock a panic poisoned is whole.
        self.failed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// When a login from `peer` may be checked, as of `now`, if it waits:
    /// its turn taken, so the next waits a delay more.
    fn turn(&self, peer: &Peer, now: Instant, most: Duration) -> Option<Instant> {
        let mut failed = self.failures();
        let source = Source::of(peer);
        let kept = *failed.get(&source)?;
        if now.saturating_duration_since(kept.next) > FORGET_AFTER {
            failed.remove(&source);
            return None;
        }
        let at = kept.next.max(now);
        failed.insert(source, Failures { next: at.checked_add(delay(kept.count, most)).unwrap_or(at), ..kept });
        (at > now).then_some(at)
    }

    /// Waits until a login from `peer` may be checked: at once, unless
    /// logins from its address failed, as SERVER.LOGIN_DELAY_MS of `db`
    /// bounds.
    pub async fn wait_turn(&self, db: &Database, peer: &Peer) {
        let Some(most) = millis(db, LOGIN_DELAY_MS) else { return };
        if let Some(at) = self.turn(peer, Instant::now(), most) {
            tokio::time::sleep_until(at.into()).await;
        }
    }

    /// Counts a failed login of `peer`, as of `now`: its address's next
    /// waits a delay more.
    fn failed_at(&self, peer: &Peer, now: Instant, most: Duration) {
        let mut failed = self.failures();
        let source = Source::of(peer);
        if !failed.contains_key(&source) && failed.len() >= MOST_ADDRESSES {
            make_room(&mut failed, now);
        }
        let kept = failed.get(&source).copied().unwrap_or(Failures { count: 0, next: now });
        let count = kept.count.saturating_add(1);
        let after = now.checked_add(delay(count, most)).unwrap_or(now);
        failed.insert(source, Failures { count, next: kept.next.max(after) });
    }

    /// Counts a failed login of `peer`; none if SERVER.LOGIN_DELAY_MS of
    /// `db` is 0.
    pub fn failed(&self, db: &Database, peer: &Peer) {
        if let Some(most) = millis(db, LOGIN_DELAY_MS) {
            self.failed_at(peer, Instant::now(), most);
        }
    }

    /// Forgets the failures of `peer`'s address: a login of it accepted.
    pub fn accepted(&self, peer: &Peer) {
        self.failures().remove(&Source::of(peer));
    }

    /// Runs `check`, a step of a login, on a blocking thread, once fewer
    /// than SERVER.MAX_LOGINS of `db` run.
    ///
    /// # Errors
    ///
    /// A connection failure if the thread failed, which is a defect.
    pub async fn check<T: Send + 'static>(&self, db: &Database, check: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        let permit = self.limit.take(|| count(db, MAX_LOGINS)).await;
        let ran = tokio::task::spawn_blocking(move || {
            let _held = permit;
            check()
        });
        ran.await.map_err(|_| broken("[DESIGN] a login's check failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOST: Duration = Duration::from_secs(10);

    fn at(address: &str) -> Peer {
        Peer::tcp(address.parse().unwrap())
    }

    /// `time`, `wait` later.
    fn after(time: Instant, wait: Duration) -> Instant {
        time.checked_add(wait).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D43: an address's logins wait only after
    /// one failed: 100 ms after the first, doubled with each failure after,
    /// at most the setting's; logins waiting take turns, a delay apart; one
    /// accepted forgets them; another address waits for nothing.
    #[test]
    fn failed_logins_slow_their_address() {
        let logins = Logins::default();
        let (ann, now) = (at("192.0.2.1:1"), Instant::now());
        assert_eq!(logins.turn(&ann, now, MOST), None, "no failure yet");
        logins.failed_at(&ann, now, MOST);
        assert_eq!(logins.turn(&ann, now, MOST), Some(after(now, FIRST_DELAY)));
        assert_eq!(logins.turn(&ann, now, MOST), Some(after(now, FIRST_DELAY.saturating_mul(2))), "turns, a delay apart");
        logins.failed_at(&ann, now, MOST);
        logins.failed_at(&ann, now, MOST);
        assert_eq!(logins.failures()[&Source::of(&ann)].count, 3);
        assert_eq!(delay(3, MOST), FIRST_DELAY.saturating_mul(4));
        assert_eq!(delay(40, MOST), MOST, "at most the setting's");
        assert_eq!(logins.turn(&at("192.0.2.2:1"), now, MOST), None, "another address");
        logins.accepted(&ann);
        assert_eq!(logins.turn(&ann, now, MOST), None, "forgotten");
    }

    /// Verifies: [DESIGN] RULES.md D43: failures are forgotten
    /// `FORGET_AFTER` after the last; IPv6 addresses count by their /64, an
    /// IPv4 one written as IPv6 as itself, and a Unix socket's clients as
    /// one.
    #[test]
    fn failures_are_counted_by_site_and_forgotten() {
        let logins = Logins::default();
        let now = Instant::now();
        logins.failed_at(&at("[2001:db8:1:2::5]:1"), now, MOST);
        assert!(logins.turn(&at("[2001:db8:1:2:ffff::9]:1"), now, MOST).is_some(), "the same /64");
        assert!(logins.turn(&at("[2001:db8:1:3::5]:1"), now, MOST).is_none(), "another /64");
        logins.failed_at(&at("[::ffff:192.0.2.7]:1"), now, MOST);
        assert!(logins.turn(&at("192.0.2.7:1"), now, MOST).is_some());
        logins.failed_at(&Peer::local(), now, MOST);
        assert!(logins.turn(&Peer::local(), now, MOST).is_some());
        let later = after(now, FORGET_AFTER.saturating_add(MOST.saturating_mul(2)));
        assert!(logins.turn(&Peer::local(), later, MOST).is_none(), "forgotten");
        assert!(!logins.failures().contains_key(&Source::Local));
    }

    /// Verifies: [DESIGN] RULES.md D43: no more than `MOST_ADDRESSES`
    /// addresses are kept: past them, those forgotten go first, else the
    /// one longest without a failure.
    #[test]
    fn addresses_kept_are_bounded() {
        let logins = Logins::default();
        let now = Instant::now();
        for n in 0..u32::try_from(MOST_ADDRESSES).unwrap() {
            let address = std::net::Ipv4Addr::from(n);
            logins.failed_at(&Peer::tcp((address, 1_u16).into()), after(now, Duration::from_millis(u64::from(n))), MOST);
        }
        logins.failed_at(&at("198.51.100.1:1"), now, MOST);
        let kept = logins.failures();
        assert_eq!(kept.len(), MOST_ADDRESSES);
        assert!(!kept.contains_key(&Source::Address("0.0.0.0".parse().unwrap())), "the oldest forgotten");
        drop(kept);
        let later = after(now, FORGET_AFTER.saturating_add(MOST.saturating_mul(2)));
        logins.failed_at(&at("198.51.100.2:1"), later, MOST);
        assert_eq!(logins.failures().len(), 1, "those forgotten went first");
    }
}
