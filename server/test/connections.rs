//! Connections: messages out of order or malformed, the limit of
//! connections, the limits of a client's time, a client's view of a server
//! that misbehaves, and the built-in transports.

use std::io::{Read, Write};
use std::net::TcpListener;

use turborel::db::Database;
use turborel_client::connection::{self, Trust};
use turborel_protocol::{Completed, Reply, Request};
use turborel_server::settings::{BATCH_BYTES, IDLE_TIMEOUT_MS, LOGIN_TIMEOUT_MS, MAX_CONNECTIONS, MAX_MESSAGE_BYTES};

use crate::common::{set, start, state};

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: a connection breaking the
/// protocol is told so (SQLSTATE 08006) and closed: a statement before
/// Hello, a kind of message of none, a body past the server's limit,
/// Hello or Answer once authenticated.
#[test]
fn a_connection_breaking_the_protocol_is_closed() {
    let db = Database::new();
    let server = start(&db, &[(MAX_MESSAGE_BYTES, 1024)]);
    let mut raw = server.raw();
    raw.send(&Request::Query("SELECT 1".into()));
    assert_eq!(state(&raw.refusal()), ("08006", -919), "no Hello");
    let mut raw = server.raw();
    raw.send_bytes(&[0, 0, 0, 0, 0]);
    assert_eq!(state(&raw.refusal()), ("08006", -919), "no such kind");
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("X".repeat(1025)));
    assert_eq!(state(&raw.refusal()), ("08006", -919), "too long");
    for again in [
        Request::Hello { version: 1, user: "X".into(), mechanism: "trust".into(), initial: vec![], capabilities: vec![] },
        Request::Answer(vec![]),
    ] {
        let mut raw = server.raw();
        raw.hello("X");
        raw.send(&again);
        assert_eq!(state(&raw.refusal()), ("08006", -919), "{again:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D36, D42: past its limit of connections,
/// SERVER.MAX_CONNECTIONS, a server refuses one (SQLSTATE 08004), and
/// takes one again once another has ended; the limit raised, it takes
/// more at once, with no restart.
#[test]
fn connections_past_the_limit_are_refused() {
    let db = Database::new();
    let server = start(&db, &[(MAX_CONNECTIONS, 1)]);
    let first = server.connect("X");
    let refused = connection::connect(server.address, "X", &mut Trust).err().unwrap();
    assert_eq!(state(&refused), ("08004", -919));
    assert!(refused.message.contains("the server has as many connections as it takes"), "{}", refused.message);
    first.terminate().unwrap();
    // The connection's task ends just after it closes the connection.
    let again = (0..1000).find_map(|_| {
        std::thread::yield_now();
        connection::connect(server.address, "X", &mut Trust).ok()
    });
    assert!(again.is_some());
    set(&db, MAX_CONNECTIONS, 2);
    let more = connection::connect(server.address, "X", &mut Trust).unwrap();
    assert_eq!(state(&connection::connect(server.address, "X", &mut Trust).err().unwrap()).0, "08004", "two, no more");
    drop((again, more));
}

/// A connection to `server` taken once it frees its one place, tried for
/// ten seconds at most.
fn reconnected(server: &crate::common::Running) -> connection::Connection<std::net::TcpStream> {
    let again = (0..1000).find_map(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        connection::connect(server.address, "X", &mut Trust).ok()
    });
    again.expect("the connection let go")
}

/// Verifies: [DESIGN] RULES.md D42: a client that does not authenticate
/// within SERVER.LOGIN_TIMEOUT_MS is told so (SQLSTATE 08006) and closed, and
/// its place is free for another.
#[test]
fn a_client_slow_to_authenticate_is_closed() {
    let db = Database::new();
    let server = start(&db, &[(MAX_CONNECTIONS, 1), (LOGIN_TIMEOUT_MS, 50)]);
    let mut raw = server.raw();
    let refused = raw.refusal();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("SERVER.LOGIN_TIMEOUT_MS"), "{}", refused.message);
    drop(reconnected(&server));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D42: a connection that sends nothing for SERVER.IDLE_TIMEOUT_MS is told so
/// (SQLSTATE 08006) and closed, its open transaction rolled back.
#[test]
fn an_idle_connection_is_closed_and_rolled_back() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[(MAX_CONNECTIONS, 1), (IDLE_TIMEOUT_MS, 50)]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("INSERT INTO T VALUES (1)".into()));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    let refused = raw.refusal();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("SERVER.IDLE_TIMEOUT_MS"), "{}", refused.message);
    let mut c = reconnected(&server);
    assert_eq!(crate::common::rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"], "rolled back");
}

/// Verifies: [DESIGN] RULES.md D42: a client that stops taking its rows
/// for SERVER.IDLE_TIMEOUT_MS is let go: the worker sending them is freed,
/// the session rolled back, and the connection's place freed.
#[test]
fn a_client_not_taking_its_rows_is_let_go() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S CHARACTER(1000))").unwrap();
    let filling = db.session("X");
    for k in 0..4000 {
        filling.run(&format!("INSERT INTO T VALUES ({k}, 'row')")).unwrap();
    }
    filling.commit().unwrap();
    let server = start(&db, &[(MAX_CONNECTIONS, 1), (BATCH_BYTES, 1), (IDLE_TIMEOUT_MS, 100)]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("DELETE FROM T WHERE K = 0".into()));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    // Four megabytes of rows, more than the sockets hold, never read.
    raw.send(&Request::Query("SELECT K, S FROM T".into()));
    let mut c = reconnected(&server);
    assert_eq!(crate::common::rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["4000"], "rolled back");
    drop(raw);
}

/// A server of one connection that answers each message read with the
/// next of `script`, then closes.
fn scripted(script: Vec<Vec<Reply>>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for replies in script {
            let mut head = [0_u8; 5];
            stream.read_exact(&mut head).unwrap();
            let mut body = vec![0_u8; u32::from_le_bytes(head[1..].try_into().unwrap()) as usize];
            stream.read_exact(&mut body).unwrap();
            let mut bytes = Vec::new();
            for reply in replies {
                reply.encode(&mut bytes);
            }
            stream.write_all(&bytes).unwrap();
        }
    });
    address
}

/// Verifies: [DESIGN] RULES.md D42: a client loses its connection, and is
/// told so (SQLSTATE 08006), to a server that sends what does not belong:
/// rows for Hello, Ready for a statement, a challenge among rows, or
/// anything after Terminate; or that sends nothing, gone; or that is not
/// there. A failure that does not end a connection leaves it working.
#[test]
fn a_client_loses_a_connection_its_server_breaks() {
    let lost = |address| connection::connect(address, "X", &mut Trust).err().map(|f| state(&f).0.to_owned());
    assert_eq!(lost(scripted(vec![vec![Reply::Rows(vec![])]])), Some("08006".into()), "rows for Hello");
    assert_eq!(lost(scripted(vec![])), Some("08006".into()), "nothing");
    let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    assert_eq!(lost(gone), Some("08006".into()), "no server");
    let readied = || Reply::Ready {
        authorization: "X".into(),
        key: turborel_protocol::CancelKey([0; 16]),
        capabilities: vec![],
        server: turborel::product(),
    };
    let ready = || vec![readied()];
    let mut c = connection::connect(scripted(vec![ready(), vec![readied()]]), "X", &mut Trust).unwrap();
    assert_eq!(state(&c.query("SELECT 1").unwrap_err()).0, "08006");
    let rows = vec![Reply::Description(vec![]), Reply::Challenge(vec![])];
    let mut c = connection::connect(scripted(vec![ready(), rows]), "X", &mut Trust).unwrap();
    assert_eq!(state(&c.query("SELECT 1").unwrap_err()).0, "08006");
    let answered = vec![ready(), vec![Reply::Complete(Completed { sqlcode: 0, sqlstate: "00000".into(), active: false })]];
    let c = connection::connect(scripted(answered), "X", &mut Trust).unwrap();
    assert_eq!(state(&c.terminate().unwrap_err()).0, "08006");
    let refused = turborel_protocol::Failure::from(&turborel_protocol::broken("no"));
    let c = connection::connect(scripted(vec![ready(), vec![Reply::Error(refused.clone())]]), "X", &mut Trust).unwrap();
    assert_eq!(c.terminate().unwrap_err(), refused);
    let kept = turborel_protocol::Failure { sqlstate: "42000".into(), ..refused };
    let script = vec![ready(), vec![Reply::Error(kept.clone())], vec![Reply::Description(vec![]), Reply::Error(kept.clone())], vec![]];
    let mut c = connection::connect(scripted(script), "X", &mut Trust).unwrap();
    assert_eq!(c.query("SELECT 1").unwrap_err(), kept);
    assert_eq!(c.query("SELECT 1").unwrap_err(), kept, "it goes on");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D42: a reply cut off within its body loses
/// the connection (SQLSTATE 08006).
#[test]
fn a_reply_cut_short_loses_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut hello = [0_u8; 64];
        let _ = stream.read(&mut hello).unwrap();
        stream.write_all(&[b'R', 10, 0, 0, 0, 1]).unwrap();
    });
    let lost = connection::connect(address, "X", &mut Trust).err().unwrap();
    assert_eq!(state(&lost).0, "08006");
    server.join().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md D42: a
/// client gone while its rows are sent: the server stops sending them and
/// fetching them, rolls its session back, and frees its connection for
/// another.
#[test]
fn a_client_gone_amid_its_rows_is_let_go() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let filling = db.session("X");
    for k in 0..2000 {
        filling.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
    }
    filling.commit().unwrap();
    let server = start(&db, &[(MAX_CONNECTIONS, 1), (BATCH_BYTES, 1)]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("DELETE FROM T WHERE K = 0".into()));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    raw.send(&Request::Query("SELECT K FROM T".into()));
    assert!(matches!(raw.receive(), Some(Reply::Description(_))));
    drop(raw);
    let again = (0..10_000).find_map(|_| {
        std::thread::yield_now();
        connection::connect(server.address, "X", &mut Trust).ok()
    });
    let mut c = again.expect("the connection let go");
    assert_eq!(crate::common::rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["2000"], "rolled back");
}

/// Verifies: [DESIGN] RULES.md D42: a Unix socket's client is local, so
/// trusted; the socket file is removed when its listener is dropped; one
/// left behind is replaced, but not one a listener listens at, nor a file
/// that is not a socket.
#[cfg(unix)]
#[test]
fn a_unix_socket_serves_this_machine() {
    use std::sync::Arc;

    use turborel_server::transport::{Listener, Local};
    let scratch = crate::common::Scratch::new("unix-socket");
    let path = scratch.path("s");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let stale = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(stale);
    let local = runtime.block_on(Local::bind(&path)).unwrap();
    assert_eq!(local.address(), path.display().to_string());
    assert!(runtime.block_on(Local::bind(&path)).is_err(), "listened at");
    let mut server = turborel_server::server::Server::new(Database::new()).unwrap();
    server.add_authenticator(Arc::new(turborel_server::auth::Trust)).unwrap();
    let listener: Box<dyn Listener> = Box::new(local);
    let endpoint = turborel_server::server::Endpoint { listener, protocol: Arc::new(turborel_server::native::Native), tls: None };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = runtime.spawn(server.serve(vec![endpoint], async { drop(stopped.await) }));
    let mut c = connection::connect_local(&path, "X", &mut Trust).unwrap();
    assert_eq!(state(&c.query("SELECT 1").unwrap_err()).1, -102_i32, "a statement runs, and fails as it should");
    c.terminate().unwrap();
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap();
    assert!(!path.exists(), "removed");
    std::fs::write(&path, b"not a socket").unwrap();
    assert!(runtime.block_on(Local::bind(&path)).is_err(), "not a socket");
    assert!(connection::connect_local(&scratch.path("none"), "X", &mut Trust).is_err());
}

/// Verifies: [DESIGN] RULES.md D42: on Windows, a named pipe's client is
/// local, so trusted; two are served at once, an instance waiting while
/// another serves; a second listener at a name one holds is refused as in
/// use; once the listener is dropped, the name may be listened at again;
/// a pipe no server holds cannot be connected to.
#[cfg(windows)]
#[test]
fn a_named_pipe_serves_this_machine() {
    use std::sync::Arc;

    use turborel_server::transport::{Listener, Pipe};
    let scratch = crate::common::Scratch::new("named-pipe");
    let path = scratch.socket("s");
    let name = path.display().to_string();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pipe = runtime.block_on(async { Pipe::bind(&name) }).unwrap();
    assert_eq!(pipe.address(), name);
    let again = runtime.block_on(async { Pipe::bind(&name) });
    assert_eq!(again.err().map(|e| e.kind()), Some(std::io::ErrorKind::AddrInUse), "listened at");
    let mut server = turborel_server::server::Server::new(Database::new()).unwrap();
    server.add_authenticator(Arc::new(turborel_server::auth::Trust)).unwrap();
    let listener: Box<dyn Listener> = Box::new(pipe);
    let endpoint = turborel_server::server::Endpoint { listener, protocol: Arc::new(turborel_server::native::Native), tls: None };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = runtime.spawn(server.serve(vec![endpoint], async { drop(stopped.await) }));
    let mut first = connection::connect_local(&path, "X", &mut Trust).unwrap();
    let second = connection::connect_local(&path, "Y", &mut Trust).unwrap();
    assert_eq!(state(&first.query("SELECT 1").unwrap_err()).1, -102_i32, "a statement runs, and fails as it should");
    assert_eq!(second.authorization(), "Y");
    first.terminate().unwrap();
    second.terminate().unwrap();
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap();
    drop(runtime.block_on(async { Pipe::bind(&name) }).unwrap());
    assert!(connection::connect_local(&scratch.socket("none"), "X", &mut Trust).is_err());
}
