//! A connection's home waiting on its client for the next request, longer
//! than a fast client takes (RULES.md D842): the client's idle time is
//! counted from its last reply whoever waits, its permit is free for
//! another connection meanwhile, and the server stopping or its operator
//! ending the connection ends the wait.

use std::time::Duration;

use turborel::db::Database;
use turborel_protocol::{Completed, Reply, Request};
use turborel_server::settings::{IDLE_IN_TRANSACTION_TIMEOUT_MS, IDLE_TIMEOUT_MS, WORKERS};

use turborel_test_support::server::Raw;

use crate::common::{busy, rows, start, state};

/// \[DESIGN\] An idle limit, shorter than a home waits for its client at
/// most, so the home gives the client back at the limit.
const LIMIT_MS: i128 = 300;
/// \[DESIGN\] A pause past `LIMIT_MS` and short of twice it: a server that
/// counted the client's idle time anew from the home's giving it back
/// would still take a request sent after it.
const PAST_LIMIT: Duration = Duration::from_millis(450);
/// \[DESIGN\] A pause past what a worker waits on a fast client
/// (`native/lent.rs`'s `WAITED`), so the home waits on.
const THINKING: Duration = Duration::from_millis(5);

/// A database whose schema X has a table T of one integer column.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    db
}

/// `sql`, as a request that keeps its transaction.
fn query(sql: &str) -> Request {
    Request::Query { sql: sql.into(), ends: false, attributes: Vec::new(), multi: false }
}

/// The replies to the request just sent on `raw`, to its Complete.
fn answered(raw: &mut Raw) -> Vec<Reply> {
    let mut replies = Vec::new();
    while let Some(reply) = raw.receive() {
        let complete = matches!(reply, Reply::Complete(_));
        replies.push(reply);
        if complete {
            break;
        }
    }
    replies
}

/// The failure `raw`'s server has sent by now, read without waiting for
/// one, and the connection closed after it.
fn told_by_now(raw: &mut Raw) -> turborel_protocol::Failure {
    raw.0.set_nonblocking(true).unwrap();
    let mut first = [0_u8; 1];
    assert!(raw.0.peek(&mut first).is_ok_and(|n| n == 1), "told by now");
    raw.0.set_nonblocking(false).unwrap();
    raw.refusal()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D42, D842: a client idle for SERVER.IDLE_TIMEOUT_MS from its last reply
/// is closed then, told so (SQLSTATE 08006), though its home waited on it
/// first and gave it back to the connection's task; one that thinks for
/// less is answered.
///
/// Regression test (R-TEST-7): the task counted the idle time anew from
/// when the home gave the client back, so a client could idle for the
/// limit twice over.
#[test]
fn a_client_idle_past_the_limit_is_closed_though_its_home_waited() {
    let db = database();
    let server = start(&db, &[(IDLE_TIMEOUT_MS, LIMIT_MS)]);
    let mut raw = server.raw();
    raw.hello("X");
    for _ in 0..3 {
        std::thread::sleep(THINKING);
        raw.send(&query("VALUES 1"));
        assert!(matches!(answered(&mut raw).last(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))), "answered");
    }
    std::thread::sleep(PAST_LIMIT);
    let refused = told_by_now(&mut raw);
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("SERVER.IDLE_TIMEOUT_MS"), "{}", refused.message);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D42, D842: a client idle in its active transaction for
/// SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS from its last reply is closed
/// then, though its home waited on it first, its transaction rolled back.
#[test]
fn a_client_idle_in_a_transaction_past_the_limit_is_closed_though_its_home_waited() {
    let db = database();
    let server = start(&db, &[(IDLE_IN_TRANSACTION_TIMEOUT_MS, LIMIT_MS)]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&query("INSERT INTO T VALUES (1)"));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, active: true, .. }))));
    std::thread::sleep(PAST_LIMIT);
    let refused = told_by_now(&mut raw);
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS"), "{}", refused.message);
    let mut c = server.connect("X");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"], "rolled back");
}

/// Verifies: [DESIGN] RULES.md D42, D815, D842: a home waiting on its
/// client holds no permit to run statements, so with SERVER.WORKERS 1
/// another connection's statement runs while the first client thinks,
/// and the first goes on after it.
#[test]
fn a_home_waiting_on_its_client_holds_no_permit() {
    let db = database();
    db.create_user("OWN", b"").unwrap();
    db.set_owner("OWN").unwrap();
    let server = start(&db, &[(WORKERS, 1)]);
    let mut a = server.connect("X");
    assert_eq!(rows(&a.query("VALUES 1").unwrap()), ["1"]);
    while busy(&db, "OWN") != "0" {
        std::thread::yield_now();
    }
    let mut b = server.connect("X");
    assert_eq!(rows(&b.query("VALUES 2").unwrap()), ["2"], "run while the first's home waits");
    assert_eq!(rows(&a.query("VALUES 3").unwrap()), ["3"], "the first goes on");
    a.terminate().unwrap();
    b.terminate().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D42, D842: an operator ending a connection whose home waits on its
/// client closes it, told so (SQLSTATE 08006), its transaction rolled
/// back, without a request from the client.
#[test]
fn an_operator_ends_a_connection_whose_home_waits() {
    let db = database();
    let server = start(&db, &[]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&query("INSERT INTO T VALUES (1)"));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    std::thread::sleep(THINKING);
    let id = server.operator.connections().into_iter().find(|(_, u)| u.as_deref() == Some("X")).unwrap().0;
    assert!(server.operator.end(id));
    let refused = raw.refusal();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("operator ended the connection"), "{}", refused.message);
    let mut c = server.connect("X");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"], "rolled back");
}

/// Verifies: [DESIGN] RULES.md D42, D235, D842: a server stopping while a
/// connection's home waits on its client closes the connection and
/// stops, the client told it is stopping or finding it closed.
#[test]
fn a_server_stops_while_a_home_waits() {
    let db = database();
    let server = start(&db, &[]);
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&query("VALUES 1"));
    assert!(matches!(answered(&mut raw).last(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    std::thread::sleep(THINKING);
    server.stop();
    let mut last = None;
    while let Some(reply) = raw.receive() {
        last = Some(reply);
    }
    match last {
        None => {}
        Some(Reply::Error(failure)) => assert_eq!(state(&failure).0, "08006", "{failure:?}"),
        Some(other) => panic!("{other:?}"),
    }
}
