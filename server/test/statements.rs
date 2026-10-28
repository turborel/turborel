//! Statements over a connection: their rows and completions, their errors
//! as the database gives them, rows in batches, and sessions: one per
//! connection, ended as the connection ends, beside the embedding
//! program's own.

use std::sync::Arc;

use turborel::catalog::TableDesc;
use turborel::db::{Database, Direct};
use turborel::error::{Code, Result, fail};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::SessionEnd;
use turborel::txn::TxId;
use turborel::value::{Decimal, Value};
use turborel_protocol::{Completed, Failure, Reply, Request};
use turborel_server::settings::{BATCH_BYTES, DEFAULT_BATCH_BYTES, WORKERS};

use crate::common::{done, rows, start, state};

fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, C CHAR(3), F REAL)").unwrap();
    db
}

/// The rows of `sql` in a session of `db`'s own, as `rows` gives them.
fn embedded(db: &Database, sql: &str) -> Vec<String> {
    let session = db.session("X");
    let mut cursor = session.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(r) = cursor.fetch().unwrap() {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1, §20.2]; [DESIGN]
/// RULES.md D42: a client runs direct SQL statements in its session, as
/// its authorization identifier: a query gives its columns, named and
/// typed, and its rows, every kind of value exact; other statements their
/// completion, SQLCODE 100 for a searched DELETE that finds no row. The
/// embedding program's sessions see what the client commits, and the
/// client what they commit.
#[test]
fn a_client_runs_direct_statements() {
    let db = database();
    let server = start(&db, &[(WORKERS, 2)]);
    let mut c = server.connect("X");
    assert_eq!(c.authorization(), "X");
    assert_eq!(done(&c.query("INSERT INTO T VALUES (1, 'AB', 1.5E0)").unwrap()), 0);
    assert_eq!(done(&c.query("INSERT INTO T (K) VALUES (2)").unwrap()), 0);
    let out = c.query("SELECT K, C, F FROM T ORDER BY K DESC").unwrap();
    assert_eq!(rows(&out), ["2,NULL,NULL", "1,AB ,1.5E0"]);
    let turborel_client::connection::Outcome::Rows { columns, .. } = &out else { panic!("no rows") };
    let named: Vec<_> = columns.iter().map(|c| (c.name.as_deref(), c.data_type.is_character(), c.not_null)).collect();
    assert_eq!(named, [(Some("K"), false, true), (Some("C"), true, false), (Some("F"), false, false)]);
    assert_eq!(done(&c.query("DELETE FROM T WHERE K = 9").unwrap()), 100);
    assert_eq!(rows(&c.query("SELECT K FROM T WHERE K = 9").unwrap()), Vec::<String>::new());
    assert_eq!(embedded(&db, "SELECT K FROM T"), Vec::<String>::new(), "not committed yet");
    assert_eq!(done(&c.query("COMMIT").unwrap()), 0);
    assert_eq!(embedded(&db, "SELECT K FROM T ORDER BY K"), ["1", "2"]);
    let own = db.session("X");
    own.run("INSERT INTO T (K) VALUES (3)").unwrap();
    own.commit().unwrap();
    assert_eq!(rows(&c.query("SELECT K FROM T WHERE K = 3").unwrap()), ["3"]);
    c.terminate().unwrap();
}

/// An engine, `failing`, whose tables give a row, then fail to read the
/// next (SQLCODE -915): a failure while a cursor is fetched from, after
/// rows were sent. A test-local plug-in (RULES.md D6); it takes no rows.
struct Failing;

struct FailingTable;

impl StorageEngine for Failing {
    fn name(&self) -> &'static str {
        "failing"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(FailingTable))
    }
}

impl TableStorage for FailingTable {
    fn insert(&self, _: TxId, _: Row) -> Result<RowId> {
        fail(Code::Unsupported, "takes no rows")
    }
    fn update(&self, _: TxId, _: RowId, _: Row) -> Result<()> {
        fail(Code::Unsupported, "takes no rows")
    }
    fn delete(&self, _: TxId, _: RowId) -> Result<()> {
        fail(Code::Unsupported, "takes no rows")
    }
    fn restore(&self, _: TxId, _: RowId, _: Row) -> Result<()> {
        fail(Code::Unsupported, "takes no rows")
    }
    fn get(&self, _: TxId, _: RowId) -> Result<Option<Row>> {
        Ok(None)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        let one = Value::Exact(Decimal::new(1, 0));
        Box::new(vec![Ok((1, vec![one])), fail(Code::Io, "told to fail")].into_iter())
    }
    fn row_count(&self) -> usize {
        2
    }
}

/// The error `sql` raises in a session of `db`'s own, run directly or
/// while its rows are fetched.
fn embedded_error(db: &Database, sql: &str) -> turborel::error::SqlError {
    let session = db.session("X");
    match session.direct(sql) {
        Err(e) => e,
        Ok(Direct::Rows(mut cursor)) => loop {
            if let Err(e) = cursor.fetch() {
                break e;
            }
        },
        Ok(Direct::Done(done)) => panic!("{done:?}"),
    }
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-2: a statement's error reaches
/// the client as the database gave it: its SQLCODE, SQLSTATE, message,
/// rule and position, whether raised before any row or while rows are
/// fetched, after some have been sent; the connection and its session go
/// on.
#[test]
fn errors_reach_the_client_as_the_database_gives_them() {
    let db = database();
    db.session("X").run("INSERT INTO T (K) VALUES (0)").unwrap();
    db.register_engine(Box::new(Failing)).unwrap();
    db.place_table("Y.B", "failing").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION Y CREATE TABLE B (K INTEGER) GRANT SELECT ON B TO PUBLIC").unwrap();
    // Batches of a row, so a row goes before the failure.
    let server = start(&db, &[(BATCH_BYTES, 1)]);
    let mut c = server.connect("X");
    for sql in ["SELECT NOPE FROM T", "SELEKT K FROM T", "INSERT INTO T VALUES (NULL, 'A', 1)", "SELECT 2 / K FROM T", "SELECT K FROM Y.B"]
    {
        let failure = c.query(sql).unwrap_err();
        // The session's transaction, begun by the first, goes on.
        assert!(failure.active, "{sql}");
        assert_eq!(failure, Failure { active: true, ..Failure::from(&embedded_error(&db, sql)) }, "{sql}");
        assert!(!failure.ends_connection());
    }
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("SELECT K FROM Y.B".into()));
    assert!(matches!(raw.receive(), Some(Reply::Description(_))));
    assert!(matches!(raw.receive(), Some(Reply::Rows(rows)) if rows.len() == 1), "the row read before");
    assert!(matches!(raw.receive(), Some(Reply::Error(failure)) if failure.sqlcode == -915));
    assert_eq!(done(&c.query("INSERT INTO T (K) VALUES (1)").unwrap()), 0);
    assert_eq!(rows(&c.query("SELECT K FROM T ORDER BY K").unwrap()), ["0", "1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.2 GR3]; [DESIGN] RULES.md D42: a
/// query's description comes first, then its rows in batches of about
/// the bytes set, then its completion: SQLCODE 0 if there were rows, 100
/// if none.
#[test]
fn rows_come_in_batches_after_their_description() {
    let db = database();
    db.session("X").run("INSERT INTO T (K) VALUES (1)").unwrap();
    let s = db.session("X");
    s.run("INSERT INTO T (K) VALUES (2)").unwrap();
    s.run("INSERT INTO T (K) VALUES (3)").unwrap();
    s.commit().unwrap();
    for (batch, sizes) in [(1, vec![1, 1, 1]), (i128::try_from(DEFAULT_BATCH_BYTES).unwrap(), vec![3])] {
        let server = start(&db, &[(BATCH_BYTES, batch)]);
        let mut raw = server.raw();
        raw.hello("X");
        raw.send(&Request::Query("SELECT K FROM T ORDER BY K".into()));
        assert!(matches!(raw.receive(), Some(Reply::Description(columns)) if columns.len() == 1));
        for size in sizes {
            assert!(matches!(raw.receive(), Some(Reply::Rows(rows)) if rows.len() == size));
        }
        assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
        raw.send(&Request::Query("SELECT K FROM T WHERE K > 3".into()));
        assert!(matches!(raw.receive(), Some(Reply::Description(_))));
        assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 100, .. }))));
    }
}

/// Verifies: [X3.135-1986 §7.1 GR2]; [ISO/IEC 9075:1992 §20.1 GR2a];
/// [DESIGN] RULES.md D42: a session the client terminates ends as
/// SESSION_END says, committed by default, rolled back when set so; one
/// whose connection is lost, or cut by the server stopping, terminated
/// unexpectedly, so is rolled back whatever SESSION_END says; the client
/// cut off is told so (SQLSTATE 08006), and its connection takes no more.
#[test]
fn a_session_ends_as_its_connection_does() {
    let db = database();
    let server = start(&db, &[(WORKERS, 2)]);
    let mut c = server.connect("X");
    c.query("INSERT INTO T (K) VALUES (1)").unwrap();
    c.terminate().unwrap();
    assert_eq!(embedded(&db, "SELECT K FROM T"), ["1"], "committed");
    db.set_session_end(SessionEnd::Rollback).unwrap();
    let mut c = server.connect("X");
    c.query("INSERT INTO T (K) VALUES (2)").unwrap();
    c.terminate().unwrap();
    db.set_session_end(SessionEnd::Commit).unwrap();
    assert_eq!(embedded(&db, "SELECT K FROM T"), ["1"], "rolled back");
    let mut raw = server.raw();
    raw.hello("X");
    raw.send(&Request::Query("INSERT INTO T (K) VALUES (3)".into()));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, .. }))));
    raw.0.shutdown(std::net::Shutdown::Write).unwrap();
    assert_eq!(state(&raw.refusal()).0, "08006", "the server closes, saying why");
    assert_eq!(embedded(&db, "SELECT K FROM T"), ["1"], "lost, so rolled back");
    let mut c = server.connect("X");
    c.query("INSERT INTO T (K) VALUES (4)").unwrap();
    server.stop();
    assert_eq!(embedded(&db, "SELECT K FROM T"), ["1"], "cut, so rolled back");
    let cut = c.query("SELECT K FROM T").unwrap_err();
    assert_eq!(state(&cut).0, "08006");
    assert!(cut.ends_connection());
    assert_eq!(c.query("SELECT K FROM T").unwrap_err(), cut, "it takes no more");
}

/// Verifies: [DESIGN] RULES.md D27, D42: many clients run at once, on
/// fewer workers than clients, each in its own session; what each commits
/// is all there.
#[test]
fn many_clients_run_at_once() {
    let db = database();
    let server = start(&db, &[(WORKERS, 2)]);
    std::thread::scope(|scope| {
        for client in 0..4 {
            let server = &server;
            scope.spawn(move || {
                let mut c = server.connect("X");
                for k in 0..25 {
                    c.query(&format!("INSERT INTO T (K) VALUES ({})", client * 100 + k)).unwrap();
                }
                c.query("COMMIT").unwrap();
                c.terminate().unwrap();
            });
        }
    });
    assert_eq!(embedded(&db, "SELECT COUNT(*) FROM T"), ["100"]);
}
