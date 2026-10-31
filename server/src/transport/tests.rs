//! Unit tests of the TCP listener: each connection it takes has its
//! keepalive on, at the times its database's settings say, read back from
//! the socket (RULES.md R-TEST-1).

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;

use socket2::SockRef;
use turborel::db::Database;
use turborel::value::{Decimal, Value};

use super::Tcp;
use crate::settings::{self, KEEPALIVE_IDLE_MS, KEEPALIVE_INTERVAL_MS, KEEPALIVE_PROBES};

/// A TCP listener at a port of loopback the system chose, on a runtime
/// of its own.
struct Loopback {
    runtime: tokio::runtime::Runtime,
    tcp: Tcp,
}

impl Loopback {
    /// One listening, keepalive's times read from `db` if given.
    fn new(db: Option<Database>) -> Loopback {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();
        let bound = runtime.block_on(Tcp::bind("127.0.0.1:0".parse().unwrap())).unwrap();
        let tcp = match db {
            Some(db) => bound.kept_alive_by(db),
            None => bound,
        };
        Loopback { runtime, tcp }
    }

    /// A connection it takes, set up as it sets one up, and the client's
    /// end.
    fn take(&self) -> (tokio::net::TcpStream, tokio::net::TcpStream) {
        self.runtime.block_on(async {
            let client = tokio::net::TcpStream::connect(self.tcp.local_addr().unwrap());
            let (connected, accepted) = tokio::join!(client, self.tcp.listener.accept());
            let (server, _) = accepted.unwrap();
            self.tcp.set_up(&server).unwrap();
            (server, connected.unwrap())
        })
    }
}

/// A database with the server's settings declared, keepalive's times as
/// given: milliseconds idle, milliseconds between probes, probes.
fn timed((idle, interval, probes): (i128, i128, i128)) -> Database {
    let db = Database::new();
    settings::declare(&db).unwrap();
    for (name, n) in [(KEEPALIVE_IDLE_MS, idle), (KEEPALIVE_INTERVAL_MS, interval), (KEEPALIVE_PROBES, probes)] {
        db.set_setting(name, &Value::Exact(Decimal::new(n, 0))).unwrap();
    }
    db
}

/// Verifies: [DESIGN] RULES.md D42: a TCP connection the server takes
/// has its keepalive on, so a client gone without closing is found out;
/// with no database, or settings of 0, at the system's own times, left as
/// they were.
#[test]
fn a_connection_taken_is_kept_alive() {
    let plain = Loopback::new(None);
    let (server, client) = plain.take();
    assert!(SockRef::from(&server).keepalive().unwrap());
    assert!(!SockRef::from(&client).keepalive().unwrap(), "off unless set");
    let zero = Loopback::new(Some(timed((0_i128, 0_i128, 0_i128))));
    let (default, _client) = zero.take();
    assert!(SockRef::from(&default).keepalive().unwrap());
    system_times_kept(&server, &default);
}

/// The system's own idle time on both `server` and `default`, as on a
/// socket never given one.
#[cfg(not(any(windows, target_os = "haiku", target_os = "openbsd", target_os = "vita")))]
fn system_times_kept(server: &tokio::net::TcpStream, default: &tokio::net::TcpStream) {
    let fresh = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    let own = fresh.tcp_keepalive_time().unwrap();
    assert_eq!(SockRef::from(server).tcp_keepalive_time().unwrap(), own);
    assert_eq!(SockRef::from(default).tcp_keepalive_time().unwrap(), own);
}

/// Nothing to read back: these platforms have no option to read the idle
/// time by.
#[cfg(any(windows, target_os = "haiku", target_os = "openbsd", target_os = "vita"))]
fn system_times_kept(_: &tokio::net::TcpStream, _: &tokio::net::TcpStream) {}

/// Verifies: [DESIGN] RULES.md D42: SERVER.KEEPALIVE_IDLE_MS,
/// SERVER.KEEPALIVE_INTERVAL_MS and SERVER.KEEPALIVE_PROBES are set on
/// each TCP connection taken, read back from its socket, a time rounded up
/// to whole seconds; a change is taken for the connections after.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_settings_times_are_set_on_each_connection() {
    let db = timed((7_000_i128, 2_500_i128, 4_i128));
    let listening = Loopback::new(Some(db.clone()));
    let (server, _client) = listening.take();
    let socket = SockRef::from(&server);
    assert!(socket.keepalive().unwrap());
    assert_eq!(socket.tcp_keepalive_time().unwrap(), Duration::from_secs(7_u64));
    assert_eq!(socket.tcp_keepalive_interval().unwrap(), Duration::from_secs(3_u64), "rounded up");
    assert_eq!(socket.tcp_keepalive_retries().unwrap(), 4_u32);
    db.set_setting(KEEPALIVE_PROBES, &Value::Exact(Decimal::new(6_i128, 0))).unwrap();
    let (next, _also) = listening.take();
    assert_eq!(SockRef::from(&next).tcp_keepalive_retries().unwrap(), 6_u32);
    assert_eq!(SockRef::from(&server).tcp_keepalive_retries().unwrap(), 4_u32, "a connection taken keeps its own");
}

/// Verifies: [DESIGN] RULES.md D42: each of keepalive's times is set
/// apart: one given, the others are the system's own, as on a socket never
/// given them.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn each_time_is_set_apart() {
    let fresh = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    let own = (fresh.tcp_keepalive_time().unwrap(), fresh.tcp_keepalive_interval().unwrap(), fresh.tcp_keepalive_retries().unwrap());
    let read = |stream: &tokio::net::TcpStream| {
        let socket = SockRef::from(stream);
        (socket.tcp_keepalive_time().unwrap(), socket.tcp_keepalive_interval().unwrap(), socket.tcp_keepalive_retries().unwrap())
    };
    let probes = Loopback::new(Some(timed((0_i128, 0_i128, 5_i128))));
    let (counted, _client) = probes.take();
    assert_eq!(read(&counted), (own.0, own.1, 5_u32));
    let idle = Loopback::new(Some(timed((9_000_i128, 0_i128, 0_i128))));
    let (idled, _other) = idle.take();
    assert_eq!(read(&idled), (Duration::from_secs(9_u64), own.1, own.2));
}

/// Verifies: [DESIGN] RULES.md D42: on Windows, SERVER.KEEPALIVE_PROBES is
/// set on each TCP connection taken, read back from its socket; its idle
/// time and interval have no option to read them back by, so only their
/// setting without failure is shown.
#[cfg(windows)]
#[test]
fn the_settings_probes_are_set_on_each_connection() {
    let listening = Loopback::new(Some(timed((7_000_i128, 0_i128, 4_i128))));
    let (server, _client) = listening.take();
    let socket = SockRef::from(&server);
    assert!(socket.keepalive().unwrap());
    assert_eq!(socket.tcp_keepalive_retries().unwrap(), 4_u32);
}
