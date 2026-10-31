//! What the server tells of itself (RULES.md D40, D42): where it listens,
//! that it stops, and hurries past SERVER.SHUTDOWN_TIMEOUT_MS or told again;
//! accepts that failed, TLS and logins that failed, and connections
//! refused, floods told once a minute.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::oneshot;
use turborel::db::Database;
use turborel_client::connection::{self, Trust};
use turborel_server::auth::{Authenticator, Exchange, Peer, Step};
use turborel_server::native::Native;
use turborel_server::server::{Endpoint, Server};
use turborel_server::settings::{MAX_CONNECTIONS, SHUTDOWN_TIMEOUT_MS};
use turborel_server::transport::{Accepting, Listener};

use crate::common::{Kept, busy, pki, secured, start, start_with, state};

/// A database of X's table T, of `ROWS` rows; its owner OWN; its events
/// kept.
fn database() -> (Database, Kept) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    let s = db.session("X");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db.create_user("OWN", b"").unwrap();
    db.set_owner("OWN").unwrap();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    (db, kept)
}

/// The rows of T: its cross join of three, its rows cubed, runs long.
const ROWS: u32 = 400;

/// A long statement of X, run over a connection to `address`, a server of
/// `db`, once it has begun: what it gave, once it ends, its failure's
/// SQLSTATE if it failed. The owner OWN waits for it embedded, so that
/// connection is the server's only one: one the owner had closed could
/// still be counted a moment after its client saw it closed.
fn waiting(db: &Database, address: std::net::SocketAddr) -> std::thread::JoinHandle<Option<String>> {
    let mut c = connection::connect(address, "X", &mut Trust).unwrap();
    let running = std::thread::spawn(move || c.query("SELECT COUNT(*) FROM T A, T B, T C").err().map(|f| f.sqlstate));
    while busy(db, "OWN") != "1" {
        std::thread::yield_now();
    }
    running
}

/// Verifies: [DESIGN] RULES.md D40, D42, D44: a server tells where it
/// listens as it starts, and that it stops; past SERVER.SHUTDOWN_TIMEOUT_MS
/// it hurries, told: the statement a session runs is cancelled, and its
/// connection closed, its session rolled back; so the server stops before
/// a long statement ends.
///
/// Regression test (R-TEST-7): a stop waited for every statement to end,
/// with no deadline.
#[test]
fn a_stop_past_its_deadline_cancels_statements() {
    let (db, kept) = database();
    let server = start(&db, &[(SHUTDOWN_TIMEOUT_MS, 50)]);
    let address = server.address.to_string();
    let running = waiting(&db, server.address);
    server.stop();
    let outcome = running.join().unwrap();
    assert!(outcome.is_some(), "it failed, cancelled or its connection closed, before it ended: {outcome:?}");
    assert_eq!(kept.of("Listening"), [format!("Listening {{ addresses: [{address:?}] }}")]);
    assert_eq!(kept.of("Stopping"), ["Stopping { connections: 1 }"]);
    assert_eq!(kept.of("StopHurried"), ["StopHurried { sessions: 1 }"]);
}

/// Verifies: [DESIGN] RULES.md D42: a server told to stop a second time
/// hurries at once, SERVER.SHUTDOWN_TIMEOUT_MS 0, no limit, notwithstanding;
/// one whose connections end in time is not hurried.
#[test]
fn a_second_stop_hurries_the_server() {
    let (db, kept) = database();
    let mut server = Server::new(db.clone()).unwrap();
    crate::common::set(&db, turborel_server::settings::PORT, 0);
    crate::common::set(&db, SHUTDOWN_TIMEOUT_MS, 0);
    server.add_authenticator(Arc::new(turborel_server::auth::Trust)).unwrap();
    let runtime = turborel_server::runtime::runtime(Some(1)).unwrap();
    let native: Arc<dyn turborel_server::server::Protocol> = Arc::new(Native);
    let endpoints = runtime.block_on(turborel_server::listening::bind(&db, &native)).unwrap();
    let address = endpoints[0].listener.address().parse().unwrap();
    let ((stop, stopped), (hurry, hurried)) = (oneshot::channel::<()>(), oneshot::channel::<()>());
    let serving = runtime.spawn(server.serve_until(endpoints, async { drop(stopped.await) }, async { drop(hurried.await) }));
    let running = waiting(&db, address);
    stop.send(()).unwrap();
    hurry.send(()).unwrap();
    runtime.block_on(serving).unwrap();
    assert!(running.join().unwrap().is_some());
    assert_eq!(kept.of("StopHurried"), ["StopHurried { sessions: 1 }"]);
    let calm = start(&db, &[(SHUTDOWN_TIMEOUT_MS, 0)]);
    calm.connect("X").terminate().unwrap();
    calm.stop();
    assert_eq!(kept.of("StopHurried").len(), 1, "not hurried");
}

/// A listener that fails to accept `FAILURES` times, then waits for ever;
/// counting its accepts.
struct Failing(Arc<AtomicUsize>);

const FAILURES: usize = 3;

impl Listener for Failing {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async move {
            if self.0.fetch_add(1, Ordering::SeqCst) < FAILURES {
                return Err(std::io::Error::other("no file descriptor left"));
            }
            std::future::pending().await
        })
    }

    fn address(&self) -> String {
        "failing".into()
    }
}

/// Verifies: [DESIGN] RULES.md D40, D42: accepts that fail are told, an
/// error with what failed, at most once a minute, those after counted.
///
/// Regression test (R-TEST-7): a failed accept was slept over, untold.
#[test]
fn failed_accepts_are_told_once_a_minute() {
    let (db, kept) = database();
    let server = Server::new(db.clone()).unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let listener: Box<dyn Listener> = Box::new(Failing(Arc::clone(&accepts)));
    let endpoint = Endpoint { listener, protocol: Arc::new(Native), tls: None };
    let runtime = turborel_server::runtime::runtime(Some(1)).unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = runtime.spawn(server.serve(vec![endpoint], async { drop(stopped.await) }));
    while accepts.load(Ordering::SeqCst) <= FAILURES {
        std::thread::yield_now();
    }
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap();
    assert_eq!(kept.of("Listening"), [r#"Listening { addresses: ["failing"] }"#]);
    assert_eq!(kept.of("AcceptFailed"), [r#"AcceptFailed { failures: 1, message: "no file descriptor left" }"#]);
}

/// Verifies: [DESIGN] RULES.md D40, D42: connections refused past
/// SERVER.MAX_CONNECTIONS are told, a warning with the limit, at most once
/// a minute.
#[test]
fn connections_refused_are_told_once_a_minute() {
    let (db, kept) = database();
    let server = start(&db, &[(MAX_CONNECTIONS, 1)]);
    let held = server.connect("X");
    for _ in 0..2 {
        let refused = connection::connect(server.address, "X", &mut Trust).err().unwrap();
        assert_eq!(state(&refused).0, "08004");
    }
    assert_eq!(kept.of("ConnectionsRefused"), ["ConnectionsRefused { refused: 1, limit: 1 }"]);
    drop(held);
}

/// An authenticator, `secret`, that refuses every proof: a test-local
/// plug-in (RULES.md D6).
struct Refusing;

struct Refusal;

impl Authenticator for Refusing {
    fn mechanism(&self) -> &'static str {
        "secret"
    }
    fn begin(&self, _: &str, _: &Peer) -> Box<dyn Exchange> {
        Box::new(Refusal)
    }
}

impl Exchange for Refusal {
    fn step(&mut self, _: &[u8]) -> Step {
        Step::Refuse("[DESIGN] not proven".into())
    }
}

/// The client's side of `secret`: its proof sent with Hello.
struct Proof;

impl connection::Mechanism for Proof {
    fn name(&self) -> &'static str {
        "secret"
    }
    fn initial(&mut self) -> Vec<u8> {
        b"hunter2".to_vec()
    }
    fn answer(&mut self, _: &[u8]) -> connection::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

/// Verifies: [EXTENSION] RULES.md D40, D42, D43: a login refused is told,
/// for an audit of who tried to connect: the user it named, where from,
/// and the mechanism and why; never what it proved by.
///
/// Regression test (R-TEST-7): a failed login was told to its client
/// alone.
#[test]
fn a_failed_login_is_told_without_its_proof() {
    let (db, kept) = database();
    let server = start_with(&db, &[], vec![Arc::new(Refusing)]);
    let peer = hello(server.address, (1, "ann", "secret"));
    let refused = connection::connect(server.address, "BOB", &mut Proof).err().unwrap();
    assert_eq!(state(&refused).0, "28000");
    let told = kept.of("LoginFailed");
    assert_eq!(told.len(), 2, "{told:?}");
    assert_eq!(told[0], format!(r#"LoginFailed {{ user: "ANN", peer: "{peer}", mechanism: "secret", message: "[DESIGN] not proven" }}"#));
    assert!(told[1].contains(r#"user: "BOB""#), "{told:?}");
    assert!(kept.all().iter().all(|e| !e.contains("hunter2")), "never the proof");
}

/// A Hello of `version`, naming `user` and `mechanism`, its proof
/// "hunter2", sent to `address` over a connection of its own, read to its
/// end: the connection's own address.
fn hello(address: std::net::SocketAddr, (version, user, mechanism): (u32, &str, &str)) -> std::net::SocketAddr {
    let mut stream = TcpStream::connect(address).unwrap();
    let peer = stream.local_addr().unwrap();
    let hello = turborel_protocol::Request::Hello {
        version,
        user: user.into(),
        mechanism: mechanism.into(),
        initial: b"hunter2".to_vec(),
        capabilities: vec![],
    };
    let mut bytes = Vec::new();
    hello.encode(&mut bytes);
    stream.write_all(&bytes).unwrap();
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).unwrap();
    peer
}

/// Verifies: [EXTENSION] RULES.md D40, D42, D43: a login the native
/// protocol refuses before any authenticator runs is told as one an
/// authenticator refuses: a Hello of a protocol version not served, the
/// user read as an identifier; of a user name that is no identifier, as
/// written; of a mechanism not served; never the proof.
///
/// Regression test (R-TEST-7): these were refused untold.
#[test]
fn a_hello_refused_before_authentication_is_told() {
    let (db, kept) = database();
    let server = start_with(&db, &[], vec![Arc::new(Refusing)]);
    let version = hello(server.address, (2, "ann", "secret"));
    let unnamed = hello(server.address, (1, "a b", "secret"));
    let unserved = hello(server.address, (1, "ann", "nothing"));
    let told = kept.of("LoginFailed");
    assert_eq!(told.len(), 3, "{told:?}");
    assert_eq!(
        told[0],
        format!(
            r#"LoginFailed {{ user: "ANN", peer: "{version}", mechanism: "secret", message: "protocol version 2 is not served; this server speaks 1" }}"#
        )
    );
    assert!(told[1].starts_with(&format!(r#"LoginFailed {{ user: "a b", peer: "{unnamed}", mechanism: "secret", message: ""#)), "{told:?}");
    assert_eq!(
        told[2],
        format!(r#"LoginFailed {{ user: "ANN", peer: "{unserved}", mechanism: "nothing", message: "mechanism nothing is not served" }}"#)
    );
    assert!(kept.all().iter().all(|e| !e.contains("hunter2")), "never the proof");
}

/// Verifies: [DESIGN] RULES.md D40, D42: a TLS handshake that fails is
/// told, a warning with the peer and why.
#[test]
fn a_failed_tls_handshake_is_told() {
    let pki = pki("tls-failed");
    let (db, kept) = database();
    secured(&db, &pki, "ON");
    let server = start(&db, &[]);
    let mut stream = TcpStream::connect(server.address).unwrap();
    let peer = stream.local_addr().unwrap();
    // A handshake record, of no handshake.
    stream.write_all(&[22, 3, 1, 0, 4, 0xff, 0xff, 0xff, 0xff]).unwrap();
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest);
    // The stream may close before the failure is told: once the server has
    // stopped, every connection's task has ended.
    server.stop();
    let told = kept.of("TlsFailed");
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].starts_with(&format!(r#"TlsFailed {{ peer: "{peer}", message: ""#)), "{told:?}");
}
