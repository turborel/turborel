//! \[DESIGN\] TCP keepalive (RULES.md D42): on for each TCP connection the
//! server takes, so a client whose machine or network is gone, never to
//! close the connection, is found out by the system, and its session
//! rolled back, its locks let go, even with no SERVER.IDLE_TIMEOUT. Its
//! times are the system's own unless SERVER.KEEPALIVE_IDLE_MS,
//! SERVER.KEEPALIVE_INTERVAL_MS and SERVER.KEEPALIVE_PROBES say otherwise,
//! read as each connection is taken; they are set by socket2's socket
//! options (RULES.md D42's dependencies).
//!
//! Platforms: Linux, macOS and Windows take all three, as do the BSDs,
//! illumos and Android. The systems count the times in seconds but
//! Windows, so a time is rounded up to whole seconds everywhere. Windows
//! sets the idle time and the interval together: one given, the other is
//! Windows' own default (2 hours idle, 1 second between probes, as its
//! `SIO_KEEPALIVE_VALS` documents), not a value of its registry; its
//! probes are set only from Windows 10 version 1703, which first has
//! `TCP_KEEPCNT`. A time a platform cannot set is refused, so every
//! connection taken fails, told as the server's failures to accept are.

use std::io;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use turborel::db::Database;

use crate::settings::{self, KEEPALIVE_IDLE_MS, KEEPALIVE_INTERVAL_MS, KEEPALIVE_PROBES};

/// \[DESIGN\] The milliseconds of a second, as the systems count TCP
/// keepalive's times.
const MS_PER_SECOND: u64 = 1000;

/// TCP keepalive's times for a connection: each the system's own if none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Keepalive {
    /// How long a connection is idle before the first probe.
    pub idle: Option<Duration>,
    /// How long between probes unanswered.
    pub interval: Option<Duration>,
    /// The probes unanswered after which the connection is closed.
    pub probes: Option<u32>,
}

impl Keepalive {
    /// The times `db`'s settings say, each rounded up to whole seconds;
    /// the system's own for a setting of 0.
    pub fn of(db: &Database) -> Keepalive {
        let probes = u32::try_from(settings::count(db, KEEPALIVE_PROBES)).ok().filter(|&n| n > 0);
        Keepalive { idle: seconds(db, KEEPALIVE_IDLE_MS), interval: seconds(db, KEEPALIVE_INTERVAL_MS), probes }
    }

    /// Turns keepalive on for `stream`, at these times.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot; `Unsupported` if a time is given that
    /// the platform cannot set.
    pub fn apply(&self, stream: &tokio::net::TcpStream) -> io::Result<()> {
        let socket = SockRef::from(stream);
        socket.set_keepalive(true)?;
        if *self == Keepalive::default() {
            return Ok(());
        }
        socket.set_tcp_keepalive(&self.params()?)
    }

    /// socket2's form of these times.
    fn params(&self) -> io::Result<TcpKeepalive> {
        let (idle, interval) = together(self.idle, self.interval);
        probes(with_interval(with_idle(TcpKeepalive::new(), idle)?, interval)?, self.probes)
    }
}

/// `db`'s setting `name` of milliseconds, rounded up to whole seconds; none
/// if it is 0.
fn seconds(db: &Database, name: &str) -> Option<Duration> {
    let ms = u64::try_from(settings::count(db, name)).unwrap_or(u64::MAX);
    (ms > 0).then(|| Duration::from_secs(ms.div_ceil(MS_PER_SECOND)))
}

/// \[DESIGN\] Windows' own idle time before the first probe, as its
/// `SIO_KEEPALIVE_VALS` documents: 2 hours.
#[cfg(windows)]
const WINDOWS_IDLE: Duration = Duration::from_secs(2 * 60 * 60);
/// \[DESIGN\] Windows' own interval between probes, as its
/// `SIO_KEEPALIVE_VALS` documents: 1 second.
#[cfg(windows)]
const WINDOWS_INTERVAL: Duration = Duration::from_secs(1);

/// The idle time and interval Windows is given: both or neither, as it
/// sets them together, Windows' own default for the one not given.
#[cfg(windows)]
fn together(idle: Option<Duration>, interval: Option<Duration>) -> (Option<Duration>, Option<Duration>) {
    if idle.is_none() && interval.is_none() {
        return (None, None);
    }
    (Some(idle.unwrap_or(WINDOWS_IDLE)), Some(interval.unwrap_or(WINDOWS_INTERVAL)))
}

/// The idle time and interval, as given: set apart on this platform.
#[cfg(not(windows))]
fn together(idle: Option<Duration>, interval: Option<Duration>) -> (Option<Duration>, Option<Duration>) {
    (idle, interval)
}

/// The failure of a time `setting` names, which this platform cannot set:
/// only on platforms that cannot set every time.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "fuchsia",
    target_os = "windows"
)))]
fn unsupported(setting: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, format!("[DESIGN] {setting}: TCP keepalive's time is not set on this platform"))
}

/// `params` with the idle time `idle`, if given.
#[cfg(not(any(target_os = "openbsd", target_os = "haiku", target_os = "vita")))]
#[expect(clippy::unnecessary_wraps, reason = "refused on platforms that cannot set it")]
fn with_idle(params: TcpKeepalive, idle: Option<Duration>) -> io::Result<TcpKeepalive> {
    Ok(match idle {
        Some(time) => params.with_time(time),
        None => params,
    })
}

/// `params`, refused if an idle time is given: this platform cannot set it.
#[cfg(any(target_os = "openbsd", target_os = "haiku", target_os = "vita"))]
fn with_idle(params: TcpKeepalive, idle: Option<Duration>) -> io::Result<TcpKeepalive> {
    idle.map_or(Ok(params), |_| Err(unsupported(KEEPALIVE_IDLE_MS)))
}

/// `params` with the interval `interval`, if given.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "fuchsia",
    target_os = "windows"
))]
#[expect(clippy::unnecessary_wraps, reason = "refused on platforms that cannot set it")]
fn with_interval(params: TcpKeepalive, interval: Option<Duration>) -> io::Result<TcpKeepalive> {
    Ok(match interval {
        Some(time) => params.with_interval(time),
        None => params,
    })
}

/// `params`, refused if an interval is given: this platform cannot set it.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "fuchsia",
    target_os = "windows"
)))]
fn with_interval(params: TcpKeepalive, interval: Option<Duration>) -> io::Result<TcpKeepalive> {
    interval.map_or(Ok(params), |_| Err(unsupported(KEEPALIVE_INTERVAL_MS)))
}

/// `params` with the probes `count`, if given.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "fuchsia",
    target_os = "windows"
))]
#[expect(clippy::unnecessary_wraps, reason = "refused on platforms that cannot set it")]
fn probes(params: TcpKeepalive, count: Option<u32>) -> io::Result<TcpKeepalive> {
    Ok(match count {
        Some(n) => params.with_retries(n),
        None => params,
    })
}

/// `params`, refused if probes are given: this platform cannot set them.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "illumos",
    target_os = "fuchsia",
    target_os = "windows"
)))]
fn probes(params: TcpKeepalive, count: Option<u32>) -> io::Result<TcpKeepalive> {
    count.map_or(Ok(params), |_| Err(unsupported(KEEPALIVE_PROBES)))
}

#[cfg(test)]
mod tests;
