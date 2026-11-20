//! The transport under the protocol, against a real server: requests
//! pipelined and their replies, over TCP and over TLS read by the worker
//! itself; a client half closed; TLS's key updates and close_notify; a
//! slow or idle client over TLS; and a server stopping with replies in
//! flight (RULES.md D42, D230-D236).

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};
use turborel::db::Database;
use turborel_client::connection::{self, Trust};
use turborel_client::tls;
use turborel_protocol::compress::{COMPRESSED, decompress};
use turborel_protocol::{Completed, HEADER, Reply, Request, VERSION, header};
use turborel_server::settings::{BATCH_BYTES, COMPRESSION_MIN_BYTES, IDLE_TIMEOUT_MS, MAX_CONNECTIONS};

use crate::common::{Pki, Running, pki, rows, secured, start};

/// A stream to a server, in the clear or over TLS, spoken to by hand.
struct Hand<S: Read + Write> {
    stream: BufReader<S>,
    /// The compressed replies read.
    compressed: usize,
}

type Tls = StreamOwned<ClientConnection, TcpStream>;

impl<S: Read + Write> Hand<S> {
    /// `stream`, logged in as X, trusted.
    fn hello(stream: S) -> Hand<S> {
        Hand::asking(stream, vec![])
    }

    /// `stream`, logged in as X, trusted, asking `capabilities`.
    fn asking(stream: S, capabilities: Vec<String>) -> Hand<S> {
        let mut hand = Hand { stream: BufReader::new(stream), compressed: 0 };
        let hello = Request::Hello {
            version: VERSION,
            user: "X".into(),
            mechanism: "trust".into(),
            initial: vec![],
            capabilities,
            attributes: Vec::new(),
        };
        hand.send(&[hello]);
        assert!(matches!(hand.receive(), Some(Reply::Ready { .. })));
        hand
    }

    /// Sends `requests` in one write.
    fn send(&mut self, requests: &[Request]) {
        let mut bytes = Vec::new();
        for request in requests {
            request.encode(&mut bytes);
        }
        let stream = self.stream.get_mut();
        stream.write_all(&bytes).unwrap();
        stream.flush().unwrap();
    }

    /// The next reply; none once the server has closed.
    fn receive(&mut self) -> Option<Reply> {
        let mut head = [0_u8; HEADER];
        self.stream.read_exact(&mut head).ok()?;
        let (kind, length) = header(head, u32::MAX).unwrap();
        let mut body = vec![0_u8; length];
        self.stream.read_exact(&mut body).unwrap();
        if kind != COMPRESSED {
            return Some(Reply::decode(kind, &body).unwrap());
        }
        self.compressed = self.compressed.saturating_add(1);
        let (inner, inflated) = decompress(&body, u32::MAX).unwrap();
        Some(Reply::decode(inner, &inflated).unwrap())
    }

    /// The rows of a query's replies, as text, to its Complete.
    fn rows(&mut self) -> Vec<String> {
        assert!(matches!(self.receive(), Some(Reply::Description(_))));
        let mut found = Vec::new();
        loop {
            match self.receive().unwrap() {
                Reply::Rows(rows) => found.extend(rows.iter().map(|r| r[0].to_string())),
                Reply::Complete(_) => return found,
                other => panic!("{other:?}"),
            }
        }
    }
}

/// A TLS stream to `server`, trusting `pki`'s authority.
fn tls_to(server: &Running, pki: &Pki) -> Tls {
    let config = tls::trusting(&pki.authority).unwrap();
    let tcp = TcpStream::connect(server.address).unwrap();
    tcp.set_nodelay(true).unwrap();
    StreamOwned::new(ClientConnection::new(Arc::clone(&config), ServerName::try_from("localhost").unwrap()).unwrap(), tcp)
}

/// A database whose table X.T holds `n` rows, K from 0.
fn numbered(n: usize) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, S CHARACTER VARYING(1000))").unwrap();
    let filling = db.session("X");
    let insert = filling.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    for k in 0..n {
        let values =
            [turborel::value::Value::Exact(turborel::value::Decimal::new(k as i128, 0)), turborel::value::Value::Char("s".repeat(500))];
        filling.execute_prepared(&insert, &values).unwrap();
    }
    filling.commit().unwrap();
    db
}

/// The query of row `k` alone.
fn row(k: usize) -> Request {
    Request::Query { sql: format!("SELECT K FROM T WHERE K = {k}"), ends: true, attributes: Vec::new(), multi: false }
}

/// Sends a hundred queries in one write and checks each is answered, in
/// order.
fn pipelined<S: Read + Write>(hand: &mut Hand<S>) {
    hand.send(&(0..100).map(row).collect::<Vec<_>>());
    for k in 0..100 {
        assert_eq!(hand.rows(), [k.to_string()], "answered in order");
    }
}

/// Verifies: [DESIGN] RULES.md D45, D231: requests a client pipelines are
/// each answered, in order, their replies sent together, in the clear and
/// over TLS; a pipeline whose replies pass SERVER.BATCH_BYTES is answered
/// whole too, a batch at a time.
#[test]
fn pipelined_requests_are_answered_in_order() {
    let pki = pki("transport-pipelined");
    let db = numbered(200);
    secured(&db, &pki, "ON");
    let server = start(&db, &[]);
    pipelined(&mut Hand::hello(TcpStream::connect(server.address).unwrap()));
    pipelined(&mut Hand::hello(tls_to(&server, &pki)));
    crate::common::set(&db, BATCH_BYTES, 1);
    pipelined(&mut Hand::hello(TcpStream::connect(server.address).unwrap()));
}

/// Verifies: [DESIGN] RULES.md D230: over TLS, the worker reads and writes
/// the session itself: a request longer than a TLS record is read whole, a
/// result longer than the sockets hold is sent whole, and a key update
/// the client asks for amid requests is answered, the requests after it
/// read under the new keys.
#[test]
fn over_tls_the_worker_reads_and_writes_the_session_itself() {
    let pki = pki("transport-tls-direct");
    let db = numbered(4000);
    secured(&db, &pki, "REQUIRED");
    let server = start(&db, &[]);
    let mut hand = Hand::hello(tls_to(&server, &pki));
    let long = format!("SELECT K FROM T WHERE K = 7 -- {}\n", "a comment ".repeat(5_000));
    hand.send(&[Request::Query { sql: long, ends: true, attributes: Vec::new(), multi: false }]);
    assert_eq!(hand.rows(), ["7"], "a request of many records");
    hand.send(&[Request::Query { sql: "SELECT K, S FROM T".into(), ends: true, attributes: Vec::new(), multi: false }]);
    assert_eq!(hand.rows().len(), 4000, "two megabytes of rows");
    hand.stream.get_mut().conn.refresh_traffic_keys().unwrap();
    pipelined(&mut hand);
    hand.send(&[Request::Terminate]);
    let mut rest = Vec::new();
    hand.stream.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "closed by close_notify, nothing after");
}

/// Verifies: [DESIGN] RULES.md D42, D234: a client that closes its side
/// after its requests hears every reply, then the connection closes; it
/// did not terminate its session, which is rolled back [ISO/IEC 9075:1992
/// §20.1 GR2a].
#[test]
fn a_half_closed_client_hears_every_reply() {
    let db = numbered(10);
    let server = start(&db, &[]);
    let tcp = TcpStream::connect(server.address).unwrap();
    let mut hand = Hand::hello(tcp.try_clone().unwrap());
    let mut requests = vec![Request::Query { sql: "DELETE FROM T".into(), ends: false, attributes: Vec::new(), multi: false }];
    requests.extend((0..5).map(|k| Request::Query {
        sql: format!("SELECT K FROM T WHERE K = {k}"),
        ends: false,
        attributes: Vec::new(),
        multi: false,
    }));
    hand.send(&requests);
    tcp.shutdown(Shutdown::Write).unwrap();
    assert!(matches!(hand.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    for _ in 0..5 {
        assert!(hand.rows().is_empty(), "deleted in its transaction");
    }
    let told = hand.receive();
    assert!(matches!(&told, None | Some(Reply::Error(_))), "{told:?}");
    assert!(hand.receive().is_none(), "closed");
    let mut c = server.connect("X");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["10"], "rolled back");
}

/// Verifies: [DESIGN] RULES.md D230, D234: a TLS client whose socket
/// closes without close_notify is lost, never taken to have ended: its
/// session is rolled back, as a connection lost [ISO/IEC 9075:1992 §20.1
/// GR2a]; one that ends with close_notify hears the server's.
#[test]
fn a_tls_client_closed_without_close_notify_is_lost() {
    let pki = pki("transport-close-notify");
    let db = numbered(10);
    secured(&db, &pki, "REQUIRED");
    let server = start(&db, &[(MAX_CONNECTIONS, 1)]);
    let mut hand = Hand::hello(tls_to(&server, &pki));
    hand.send(&[Request::Query { sql: "DELETE FROM T".into(), ends: false, attributes: Vec::new(), multi: false }]);
    assert!(matches!(hand.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    hand.stream.get_mut().sock.shutdown(Shutdown::Both).unwrap();
    drop(hand);
    let config = tls::trusting(&pki.authority).unwrap();
    let again = (0..10_000).find_map(|_| {
        std::thread::yield_now();
        tls::connect(server.address, ("localhost", &config), "X", &mut Trust).ok()
    });
    let mut c = again.expect("its place freed");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["10"], "rolled back");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D42, D234: over TLS, a client that stops
/// taking its rows, or sends nothing, for SERVER.IDLE_TIMEOUT_MS is let
/// go: its worker freed, its session rolled back and its place freed.
#[test]
fn a_slow_or_idle_tls_client_is_let_go() {
    let pki = pki("transport-tls-slow");
    let db = numbered(4000);
    secured(&db, &pki, "REQUIRED");
    let server = start(&db, &[(MAX_CONNECTIONS, 1), (BATCH_BYTES, 1), (IDLE_TIMEOUT_MS, 100)]);
    let mut slow = Hand::hello(tls_to(&server, &pki));
    slow.send(&[Request::Query { sql: "DELETE FROM T WHERE K = 0".into(), ends: false, attributes: Vec::new(), multi: false }]);
    assert!(matches!(slow.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    slow.send(&[Request::Query { sql: "SELECT K, S FROM T".into(), ends: false, attributes: Vec::new(), multi: false }]);
    let config = tls::trusting(&pki.authority).unwrap();
    let reconnect = || {
        (0..1_000).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tls::connect(server.address, ("localhost", &config), "X", &mut Trust).ok()
        })
    };
    let mut c = reconnect().expect("the slow client let go");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["4000"], "rolled back");
    drop(slow);
    let mut idle = c;
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(idle.query("SELECT COUNT(*) FROM T").is_err(), "the idle client let go");
    reconnect().expect("its place freed");
}

/// Verifies: [DESIGN] RULES.md D42, D235: a server told to stop finishes
/// the request it runs and sends its replies whole before it closes the
/// connection; a request pipelined behind it is not run, the client told
/// the server stops.
#[test]
fn a_server_stopping_sends_the_replies_in_flight() {
    let db = numbered(10);
    db.create_user("OWN", b"").unwrap();
    db.set_owner("OWN").unwrap();
    let server = start(&db, &[]);
    let holder = db.session("X");
    holder.run("UPDATE T SET S = 'held' WHERE K = 1").unwrap();
    let mut hand = Hand::hello(TcpStream::connect(server.address).unwrap());
    hand.send(&[
        Request::Query { sql: "UPDATE T SET S = 'mine' WHERE K = 1".into(), ends: true, attributes: Vec::new(), multi: false },
        row(2),
    ]);
    // The update is in flight, waiting on the lock, before the stop.
    while crate::common::busy(&db, "OWN") != "1" {
        std::thread::yield_now();
    }
    let address = server.address;
    let stopping = std::thread::spawn(move || server.stop());
    while connection::connect(address, "X", &mut Trust).is_ok() {
        std::thread::yield_now();
    }
    holder.commit().unwrap();
    assert!(matches!(hand.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))), "the update in flight answered");
    let Some(Reply::Error(told)) = hand.receive() else { panic!("told") };
    assert!(told.message.contains("stopping"), "{told:?}");
    assert!(hand.receive().is_none(), "closed");
    stopping.join().unwrap();
    let session = db.session("X");
    let mut cursor = session.open_cursor("SELECT S FROM T WHERE K = 1").unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "mine", "committed");
}

/// Verifies: [DESIGN] RULES.md D232, D235: a server told to stop a second
/// time hurries, and lets go a client that takes none of its rows, the
/// worker waiting on it in the write stopping, though SERVER.IDLE_TIMEOUT_MS
/// is 0, no limit; the client is closed, its session rolled back.
#[test]
fn a_hurried_server_lets_go_a_client_taking_no_rows() {
    use turborel_server::native::Native;
    use turborel_server::server::{Protocol, Server};
    use turborel_server::settings::{PORT, SHUTDOWN_TIMEOUT_MS};
    let db = numbered(4000);
    let mut server = Server::new(db.clone()).unwrap();
    for (name, n) in [(PORT, 0), (SHUTDOWN_TIMEOUT_MS, 0), (IDLE_TIMEOUT_MS, 0), (BATCH_BYTES, 1)] {
        crate::common::set(&db, name, n);
    }
    server.add_authenticator(Arc::new(turborel_server::auth::Trust)).unwrap();
    let runtime = turborel_server::runtime::runtime(Some(1)).unwrap();
    let native: Arc<dyn Protocol> = Arc::new(Native);
    let endpoints = runtime.block_on(turborel_server::listening::bind(&db, &native)).unwrap();
    let address: std::net::SocketAddr = endpoints[0].listener.address().parse().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (hurry, hurried) = tokio::sync::oneshot::channel::<()>();
    let serving = runtime.spawn(server.serve_until(endpoints, async { drop(stopped.await) }, async { drop(hurried.await) }));
    let mut slow = Hand::hello(TcpStream::connect(address).unwrap());
    slow.send(&[Request::Query { sql: "DELETE FROM T WHERE K = 0".into(), ends: false, attributes: Vec::new(), multi: false }]);
    assert!(matches!(slow.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    slow.send(&[Request::Query { sql: "SELECT K, S FROM T".into(), ends: false, attributes: Vec::new(), multi: false }]);
    assert!(matches!(slow.receive(), Some(Reply::Description(_))), "its rows begun, then not taken");
    stop.send(()).unwrap();
    hurry.send(()).unwrap();
    runtime.block_on(serving).unwrap();
    drop(slow);
    let session = db.session("X");
    let mut cursor = session.open_cursor("SELECT COUNT(*) FROM T").unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "4000", "rolled back");
}

/// Verifies: [DESIGN] RULES.md D196, D231: a client granted compression
/// that pipelines its requests is answered in order, each reply long
/// enough compressed on its own, though the replies share a write.
#[test]
fn pipelined_replies_granted_compression_come_in_order() {
    let db = numbered(10);
    let server = start(&db, &[(COMPRESSION_MIN_BYTES, 64)]);
    let tcp = TcpStream::connect(server.address).unwrap();
    let mut hand = Hand::asking(tcp, vec![turborel_protocol::capability::COMPRESSION.into()]);
    let requests: Vec<Request> = (0..10)
        .map(|k| Request::Query { sql: format!("SELECT S, K FROM T WHERE K = {k}"), ends: true, attributes: Vec::new(), multi: false })
        .collect();
    hand.send(&requests);
    for k in 0..10 {
        assert!(matches!(hand.receive(), Some(Reply::Description(_))));
        let Some(Reply::Rows(rows)) = hand.receive() else { panic!("rows") };
        assert_eq!(rows[0][1].to_string(), k.to_string(), "in order");
        assert!(matches!(hand.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    }
    assert!(hand.compressed >= 10, "each row of 500 characters compressed: {}", hand.compressed);
}

/// The replies up to and including the first `last` says ends them, and
/// how many of them were Statuses.
fn statuses_until<S: Read + Write>(hand: &mut Hand<S>, last: fn(&Reply) -> bool) -> usize {
    let mut statuses = 0_usize;
    loop {
        let reply = hand.receive().expect("a reply");
        statuses = statuses.saturating_add(usize::from(matches!(reply, Reply::Status { .. })));
        if last(&reply) {
            return statuses;
        }
    }
}

/// Verifies: [DESIGN] RULES.md D190: with `status`, a login tells each of
/// the session's settings once, and a statement after it tells only those
/// it changed. Regression test (R-TEST-7): the login's Statuses were not
/// held as told, so the first statement sent five of them again.
#[test]
fn a_statement_after_a_login_tells_only_what_it_changed() {
    let db = Database::new();
    let server = start(&db, &[]);
    let mut hand = Hand { stream: BufReader::new(TcpStream::connect(server.address).unwrap()), compressed: 0 };
    let capabilities = vec!["status".to_owned()];
    let hello = Request::Hello {
        version: VERSION,
        user: "X".into(),
        mechanism: "trust".into(),
        initial: vec![],
        capabilities,
        attributes: Vec::new(),
    };
    hand.send(&[hello]);
    assert!(statuses_until(&mut hand, |r| matches!(r, Reply::Ready { .. })) >= 7, "every status at login");
    for _ in 0..2 {
        hand.send(&[Request::Query { sql: "VALUES 1".into(), ends: true, attributes: Vec::new(), multi: false }]);
        assert_eq!(statuses_until(&mut hand, |r| matches!(r, Reply::Complete(_))), 0, "nothing changed");
    }
}
