//! Protocol version 1 over a connection (RULES.md D45): prepared statements
//! and their cursors, read a batch at a time [ISO/IEC 9075:1992 §17];
//! requests sent before the last is answered; and a running statement
//! cancelled from another connection.

use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::connection::Outcome;
use turborel_protocol::{Completed, Reply, Request};
use turborel_server::settings::MAX_STATEMENTS;

use crate::common::{done, rows, start, state};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A database whose table X.T holds K from 1 to `n`.
fn database(n: i128) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    let s = db.session("X");
    for k in 1..=n {
        s.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db
}

fn keys(batch: &[Vec<Value>]) -> Vec<String> {
    batch.iter().map(|r| r[0].to_string()).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6, §17.8, §17.10, §17.14,
/// §17.15, §17.16, §17.7]; [DESIGN] RULES.md D45: a statement prepared over
/// a connection is described, its parameters' types and its columns; a
/// query's cursor opened with values sends the rows asked for and says it
/// has more, which fetch sends, until it has none and is closed; closed,
/// it is opened again; another statement executes with its values.
#[test]
fn prepared_statements_read_a_batch_at_a_time() {
    let db = database(5);
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let query = c.prepare("SELECT K FROM T WHERE K > ? ORDER BY K").unwrap();
    assert_eq!(query.parameters().len(), 1);
    assert_eq!(query.columns()[0].name.as_deref(), Some("K"));
    let first = c.open_cursor(&query, &[int(0)], 2).unwrap();
    assert_eq!((keys(&first.rows), first.more), (vec!["1".to_owned(), "2".to_owned()], true));
    let next = c.fetch(&query, 2).unwrap();
    assert_eq!((keys(&next.rows), next.more), (vec!["3".to_owned(), "4".to_owned()], true));
    let rest = c.fetch(&query, 0).unwrap();
    assert_eq!((keys(&rest.rows), rest.more), (vec!["5".to_owned()], false), "all sent: closed");
    let again = c.open_cursor(&query, &[int(3)], 1).unwrap();
    assert_eq!((keys(&again.rows), again.more), (vec!["4".to_owned()], true));
    c.close(&query).unwrap();
    let Outcome::Rows { rows: all, .. } = c.execute(&query, &[int(4)]).unwrap() else { panic!("rows") };
    assert_eq!(keys(&all), ["5"]);
    let insert = c.prepare("INSERT INTO T VALUES (?)").unwrap();
    assert_eq!(insert.columns().len(), 0);
    assert!(matches!(c.execute(&insert, &[int(6)]).unwrap(), Outcome::Done(Completed { sqlcode: 0, .. })));
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["6"]);
    c.deallocate(insert).unwrap();
    c.deallocate(query).unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.2 GR1, §13.3 GR1, §13.4
/// GR1, §17.7 GR1, GR2, §17.9 GR5a, §17.10 GR1]: over a connection, a
/// cursor opened while open, or fetched or closed while not, is an invalid
/// cursor state (SQLSTATE 24000); a statement destroyed while its cursor
/// is open too; one not prepared, an invalid statement name (26000);
/// values not one for each parameter, 07001. The connection goes on.
#[test]
fn prepared_statements_are_refused_saying_why() {
    let db = database(3);
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let query = c.prepare("SELECT K FROM T WHERE K > ?").unwrap();
    assert_eq!(state(&c.fetch(&query, 1).unwrap_err()).0, "24000", "not open");
    assert_eq!(state(&c.close(&query).unwrap_err()).0, "24000", "not open");
    assert_eq!(state(&c.open_cursor(&query, &[], 1).unwrap_err()).0, "07001");
    assert!(c.open_cursor(&query, &[int(0)], 1).unwrap().more);
    assert_eq!(state(&c.open_cursor(&query, &[int(0)], 1).unwrap_err()).0, "24000", "open");
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Execute { statement: 9, values: vec![], rows: 0 });
    let Some(Reply::Error(unknown)) = raw.receive() else { panic!("no error") };
    assert_eq!(state(&unknown), ("26000", -921));
    raw.send(&Request::Deallocate { statement: 9 });
    assert!(matches!(raw.receive(), Some(Reply::Error(f)) if f.sqlstate == "26000"));
    raw.send(&Request::Prepare { statement: 1, sql: "SELECT K FROM T".into() });
    assert!(matches!(raw.receive(), Some(Reply::Prepared { statement: 1, .. })));
    raw.send(&Request::Execute { statement: 1, values: vec![], rows: 1 });
    assert!(matches!(raw.receive(), Some(Reply::Description(_))));
    assert!(matches!(raw.receive(), Some(Reply::Rows(_))));
    assert!(matches!(raw.receive(), Some(Reply::Suspended)));
    raw.send(&Request::Deallocate { statement: 1 });
    assert!(matches!(raw.receive(), Some(Reply::Error(f)) if f.sqlstate == "24000"), "open");
    raw.send(&Request::Prepare { statement: 1, sql: "SELECT 1 FROM T".into() });
    assert!(matches!(raw.receive(), Some(Reply::Error(f)) if f.sqlstate == "24000"), "prepared again, its cursor open [§17.6 GR6]");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["3"], "the connection goes on");
}

/// Verifies: [DESIGN] RULES.md D45: requests sent before the last is
/// answered are each answered, in order; a connection holds at most
/// SERVER.MAX_STATEMENTS prepared statements, one prepared again taking
/// its own place.
#[test]
fn requests_are_answered_in_order() {
    let db = database(2);
    let server = start(&db, &[(MAX_STATEMENTS, 1)]);
    let mut raw = server.raw();
    raw.hello("X");
    let batch = [
        Request::Prepare { statement: 7, sql: "SELECT K FROM T WHERE K = ?".into() },
        Request::Execute { statement: 7, values: vec![int(2)], rows: 0 },
        Request::Query("SELECT COUNT(*) FROM T".into()),
        Request::Prepare { statement: 8, sql: "SELECT K FROM T".into() },
        Request::Prepare { statement: 7, sql: "SELECT K FROM T".into() },
    ];
    let mut bytes = Vec::new();
    for request in &batch {
        request.encode(&mut bytes);
    }
    raw.send_bytes(&bytes);
    assert!(matches!(raw.receive(), Some(Reply::Prepared { statement: 7, .. })));
    assert!(matches!(raw.receive(), Some(Reply::Description(_))));
    assert!(matches!(raw.receive(), Some(Reply::Rows(r)) if r.len() == 1 && r[0][0].to_string() == "2"));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    assert!(matches!(raw.receive(), Some(Reply::Description(_))));
    assert!(matches!(raw.receive(), Some(Reply::Rows(_))));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    assert!(matches!(raw.receive(), Some(Reply::Error(f)) if f.sqlcode == -918), "past the limit");
    assert!(matches!(raw.receive(), Some(Reply::Prepared { statement: 7, .. })), "its own place");
}

/// Verifies: [DESIGN] RULES.md D44, D45: a statement a connection runs is
/// cancelled from another thread, over a new connection with the key its
/// Ready gave: it fails (SQLCODE -920) and the session goes on. A cancel
/// with a key of no connection does nothing.
#[test]
fn a_running_statement_is_cancelled_over_another_connection() {
    let db = database(300);
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let canceller = c.canceller().unwrap();
    let mut raw = server.raw();
    raw.send(&Request::Cancel(turborel_protocol::CancelKey([1; 16])));
    assert!(raw.receive().is_none(), "closed, nothing said");
    let running = std::thread::spawn(move || {
        let outcome = c.query("SELECT COUNT(*) FROM T A, T B, T C");
        (c, outcome.map(|o| done_or_rows(&o)))
    });
    // Cancelled once it runs: a cancel before is of nothing.
    while !running.is_finished() {
        canceller.cancel().unwrap();
        std::thread::yield_now();
    }
    let (mut c, outcome) = running.join().unwrap();
    let failure = outcome.unwrap_err();
    assert_eq!(state(&failure), ("TB920", -920));
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["300"], "the session goes on");
    assert_eq!(done(&c.query("COMMIT").unwrap()), 0);
}

fn done_or_rows(outcome: &Outcome) -> usize {
    match outcome {
        Outcome::Rows { rows, .. } => rows.len(),
        Outcome::Done(_) => 0,
    }
}

/// Runs a long statement over `c` on a thread, cancelling it with
/// `canceller` until it ends: what it gave, and `c` back.
fn cancelled<S: std::io::Read + std::io::Write + Send + 'static>(
    mut c: turborel_client::connection::Connection<S>,
    canceller: &turborel_client::Canceller,
) -> (turborel_client::connection::Connection<S>, i32) {
    let running = std::thread::spawn(move || {
        let outcome = c.query("SELECT COUNT(*) FROM T A, T B, T C");
        (c, outcome.err().map_or(0, |f| f.sqlcode))
    });
    while !running.is_finished() {
        canceller.cancel().unwrap();
        std::thread::yield_now();
    }
    running.join().unwrap()
}

/// Verifies: [DESIGN] RULES.md D44, D45: a statement is cancelled over the
/// way its connection was made: over TLS, with the same trust in the
/// server, and on Unix over its socket. A connection over a stream given
/// to it cannot reach its server again, so has no canceller; a canceller
/// never shows its key, and fails to cancel once the server is gone.
#[test]
fn a_statement_is_cancelled_over_tls_and_a_socket() {
    let pki = crate::common::pki("cancel-tls");
    let db = database(300);
    crate::common::secured(&db, &pki, "ON");
    #[cfg(unix)]
    let socket = pki.scratch.path("s");
    #[cfg(unix)]
    db.set_setting(turborel_server::settings::SOCKET, &Value::Char(socket.display().to_string())).unwrap();
    let server = start(&db, &[]);
    let secure = crate::common::over_tls(&server, &pki, "localhost").unwrap();
    let canceller = secure.canceller().unwrap();
    assert_eq!(format!("{canceller:?}"), "Canceller(..)");
    let (_, code) = cancelled(secure, &canceller);
    assert_eq!(code, -920, "over TLS");
    #[cfg(unix)]
    {
        let local = turborel_client::connection::connect_local(&socket, "X", &mut turborel_client::connection::Trust).unwrap();
        let local_canceller = local.canceller().unwrap();
        let (_, code) = cancelled(local, &local_canceller);
        assert_eq!(code, -920, "over the socket");
    }
    let stream = std::net::TcpStream::connect(server.address).unwrap();
    let given = turborel_client::connection::Connection::open(stream, "X", &mut turborel_client::connection::Trust).unwrap();
    assert!(given.canceller().is_none());
    drop(server);
    assert_eq!(canceller.cancel().map_err(|f| state(&f).0.to_owned()).err().as_deref(), Some("08006"), "the server is gone");
}
