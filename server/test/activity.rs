//! What INFORMATION_SCHEMA.CONNECTIONS shows of what each connection's
//! session does now (RULES.md D520): the statement it runs and when it
//! began, when its transaction began, and what it waits on; and who sees
//! which connections.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use turborel::clock::FixedClock;
use turborel::db::Database;
use turborel_server::settings::WORKERS;

use crate::common::{rows, start};

/// 2026-11-20 12:00:00 UTC, in microseconds from 1970.
const NOON: i64 = 1_795_176_000_000_000;
/// How many times a test looks for a wait, a few milliseconds apart.
const LOOKS: usize = 2_000;

/// A database whose owner is OWNER, with X's table T of the row 1, its
/// clock standing at `NOON`.
fn owned() -> Database {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.set_clock(Arc::new(FixedClock::at(NOON))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let x = db.session("X");
    x.run("INSERT INTO T VALUES (1)").unwrap();
    x.commit().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D520, D43: CONNECTIONS shows a connection
/// running a statement its text and when it began, and one idle neither;
/// a connection with a transaction open when it began. The database's
/// owner sees every connection; another user their own alone, until
/// granted the role MONITOR the owner created.
#[test]
fn a_connection_shows_its_statement_and_its_transaction() {
    let db = owned();
    let server = start(&db, &[]);
    let mut owner = server.connect("OWNER");
    let mut x = server.connect("X");
    x.query("INSERT INTO T VALUES (2)").unwrap();
    let doing = "SELECT ID, STATE, STATEMENT_TEXT, STATEMENT_BEGAN, TRANSACTION_BEGAN, WAITING_ON \
                 FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID";
    let began = "2026-11-20 12:00:00.000000+00:00";
    assert_eq!(
        rows(&owner.query(doing).unwrap()),
        [format!("1,RUNNING,{doing},{began},{began},NULL"), format!("2,IDLE,NULL,NULL,{began},NULL")]
    );
    assert_eq!(rows(&x.query("SELECT ID FROM INFORMATION_SCHEMA.CONNECTIONS").unwrap()), ["2"], "its own alone");
    owner.query("ROLLBACK").unwrap();
    owner.query("CREATE ROLE MONITOR").unwrap();
    owner.query("GRANT MONITOR TO X").unwrap();
    owner.query("COMMIT").unwrap();
    assert_eq!(rows(&x.query("SELECT ID FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID").unwrap()), ["1", "2"], "granted MONITOR");
    x.query("ROLLBACK").unwrap();
}

/// What `reader` reads of connection `id`'s wait: what it waits on, the
/// lock's name and its holder; once it waits on something, or after
/// `LOOKS` looks.
fn wait_of(reader: &turborel::db::Session, id: u64) -> String {
    let sql = format!("SELECT WAITING_ON, LOCK_NAME LIKE '%T%', LOCK_HOLDER FROM INFORMATION_SCHEMA.CONNECTIONS WHERE ID = {id}");
    let mut seen = String::new();
    for _ in 0..LOOKS {
        seen = turborel_test_support::database::rows(reader, &sql).unwrap().concat();
        reader.rollback().unwrap();
        if !seen.starts_with("NULL") {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    seen
}

/// Verifies: [DESIGN] RULES.md D520: a connection waiting for a row
/// another connection's open transaction changed shows the lock and the
/// connection holding it, and none once it goes on.
#[test]
fn a_connection_shows_the_lock_it_waits_on() {
    let db = owned();
    let server = start(&db, &[]);
    let mut holder = server.connect("X");
    holder.query("UPDATE T SET K = 2 WHERE K = 1").unwrap();
    let mut waiter = server.connect("X");
    let waiting = thread::spawn(move || {
        waiter.query("UPDATE T SET K = 3 WHERE K = 1").unwrap();
        waiter
    });
    let owner = db.session("OWNER");
    assert_eq!(wait_of(&owner, 2), "LOCK,TRUE,1", "the row, held by the first");
    holder.query("ROLLBACK").unwrap();
    let mut waiter = waiting.join().unwrap();
    let after = "SELECT ID, WAITING_ON FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID";
    assert_eq!(turborel_test_support::database::rows(&owner, after).unwrap(), ["1,NULL", "2,NULL"]);
    waiter.query("ROLLBACK").unwrap();
}

/// Verifies: [DESIGN] RULES.md D520: a request waiting for a worker, the
/// only one busy, shows WORKER; a lock held by a session no connection
/// serves shows no holder.
#[test]
fn a_connection_shows_it_waits_for_a_worker() {
    let db = owned();
    let server = start(&db, &[(WORKERS, 1)]);
    let holder = db.session("X");
    holder.run("UPDATE T SET K = 2 WHERE K = 1").unwrap();
    let mut waiter = server.connect("X");
    let waiting = thread::spawn(move || {
        waiter.query("UPDATE T SET K = 3 WHERE K = 1").unwrap();
        waiter
    });
    let owner = db.session("OWNER");
    assert_eq!(wait_of(&owner, 1), "LOCK,TRUE,NULL", "held by no connection");
    let mut queued = server.connect("X");
    let asking = thread::spawn(move || {
        queued.query("VALUES 1").unwrap();
        queued
    });
    assert_eq!(wait_of(&owner, 2), "WORKER,NULL,NULL", "the only worker waits on the row");
    holder.rollback().unwrap();
    let mut waiter = waiting.join().unwrap();
    asking.join().unwrap();
    waiter.query("ROLLBACK").unwrap();
}
