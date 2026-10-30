//! How a connection ends at the server's word: ended by its operator, its
//! user dropped, or idle in a transaction too long.

use std::time::Duration;

use turborel::db::Database;
use turborel::value::Value;
use turborel_protocol::{Completed, Reply, Request};
use turborel_server::settings::IDLE_IN_TRANSACTION_TIMEOUT_MS;

use crate::common::{Running, rows, start, state};

/// The ID of the connection of `user` that `server` serves.
fn id_of(server: &Running, user: &str) -> u64 {
    server.operator.connections().into_iter().find(|(_, u)| u.as_deref() == Some(user)).unwrap().0
}

/// The rows of `table`, as `user` counts them in a session of its own,
/// embedded in the program that serves `db`.
fn count(db: &Database, user: &str, table: &str) -> String {
    let session = db.session(user);
    let mut cursor = session.open_cursor(&format!("SELECT COUNT(*) FROM {table}")).unwrap();
    let row = cursor.fetch().unwrap().unwrap();
    drop(cursor);
    session.rollback().unwrap();
    let [Value::Exact(n)] = row.as_slice() else { panic!("{row:?}") };
    n.to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D42, D44: an operator ends a connection by its ID: one idle is closed,
/// told so (SQLSTATE 08006), its transaction rolled back; one whose
/// statement waits for a lock has it cancelled, then is closed, so the
/// lock is no longer waited for; the others go on. An ID of none served
/// ends nothing.
#[test]
fn an_operator_ends_a_connection() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let mut x = server.connect("X");
    let mut y = server.connect("Y");
    x.query("INSERT INTO T VALUES (1)").unwrap();
    assert!(server.operator.end(id_of(&server, "X")));
    let ended = x.query("VALUES 1").unwrap_err();
    assert_eq!(state(&ended), ("08006", -919));
    assert!(ended.message.contains("operator ended the connection"), "{}", ended.message);
    assert!(x.is_lost());
    assert_eq!(count(&db, "X", "T"), "0", "rolled back");
    assert_eq!(rows(&y.query("VALUES 1").unwrap()), ["1"], "the other goes on");
    let holder = db.session("X");
    holder.run("INSERT INTO T VALUES (2)").unwrap();
    let mut waiting = server.raw();
    waiting.hello("X");
    waiting.send(&Request::Query("DELETE FROM T".into()));
    let id = id_of(&server, "X");
    assert!(server.operator.end(id));
    let mut last = None;
    while let Some(reply) = waiting.receive() {
        last = Some(reply);
    }
    let Some(Reply::Error(failure)) = last else { panic!("{last:?}") };
    assert_eq!(state(&failure), ("08006", -919), "{failure:?}");
    holder.commit().unwrap();
    assert_eq!(count(&db, "X", "T"), "1", "the DELETE never ran, or was cancelled");
    assert!(!server.operator.end(id), "gone");
    assert!(!server.operator.end(999));
    y.terminate().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a, §22.1 Table 23];
/// [DESIGN] RULES.md D42, D43, D44: in SQL, the database's owner cancels
/// what another connection runs, by its ID, CANCEL CONNECTION, that
/// connection's statement failing (SQLCODE -920), its session going on;
/// and ends it, END CONNECTION, that connection then closed (SQLSTATE
/// 08006), rolled back. Each in the owner's own transaction too, which
/// goes on; ending its own connection, the owner's is ended so. Another user is refused (SQLCODE -104); an ID of none served
/// is refused (SQLSTATE 08003), the owner's connection going on.
#[test]
fn the_owner_ends_and_cancels_connections_in_sql() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER) GRANT SELECT ON T TO PUBLIC").unwrap();
    let filling = db.session("X");
    for k in 0..100 {
        filling.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
    }
    filling.commit().unwrap();
    let server = start(&db, &[]);
    let mut owner = server.connect("OWNER");
    let mut x = server.connect("X");
    let id = id_of(&server, "X");
    let refused = x.query(&format!("END CONNECTION {id}")).unwrap_err();
    assert_eq!(state(&refused).1, -104, "not the owner");
    let none = owner.query("END CONNECTION 999").unwrap_err();
    assert_eq!(state(&none), ("08003", -919));
    assert!(!owner.is_lost(), "of another connection: the owner's goes on");
    owner.query("VALUES 1").unwrap();
    assert!(owner.in_transaction());
    let mut long = server.raw();
    long.hello("Y");
    long.send(&Request::Query("SELECT COUNT(*) FROM X.T A, X.T B, X.T C, X.T D".into()));
    let y = id_of(&server, "Y");
    long.0.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    // Cancelled once it runs: a cancel before is of nothing.
    let cancelled = loop {
        owner.query(&format!("CANCEL CONNECTION {y}")).unwrap();
        match long.receive() {
            None | Some(Reply::Description(_)) => {}
            Some(reply) => break reply,
        }
    };
    let Reply::Error(failure) = cancelled else { panic!("{cancelled:?}") };
    assert_eq!(state(&failure), ("TB920", -920));
    long.0.set_read_timeout(None).unwrap();
    long.send(&Request::Query("VALUES 1".into()));
    assert!(matches!(long.receive(), Some(Reply::Description(_))), "its session goes on");
    x.query("INSERT INTO T VALUES (100)").unwrap();
    owner.query(&format!("END CONNECTION {id}")).unwrap();
    assert!(owner.in_transaction(), "the owner's transaction goes on");
    let ended = x.query("VALUES 1").unwrap_err();
    assert_eq!(state(&ended), ("08006", -919));
    assert_eq!(count(&db, "X", "T"), "100", "rolled back");
    owner.query("COMMIT").unwrap();
    let own = id_of(&server, "OWNER");
    // Its own: ended as its statement ends, its reply let go or not.
    drop(owner.query(&format!("END CONNECTION {own}")));
    assert_eq!(state(&owner.query("VALUES 1").unwrap_err()).0, "08006", "a session ends its own so too");
}

/// Verifies: [DESIGN] RULES.md D43: a user's sessions end once the user is
/// dropped: its next statement is refused (SQLSTATE 28000) and its
/// connection closed, its transaction rolled back. A new password ends
/// none; nor does dropping a user end the session of another identifier,
/// a user or not.
#[test]
fn a_dropped_users_sessions_end() {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    db.create_user("BOB", b"y").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION BOB CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let (mut ann, mut bob, mut x) = (server.connect("ann"), server.connect("bob"), server.connect("X"));
    bob.query("INSERT INTO T VALUES (1)").unwrap();
    db.set_verifier("BOB", b"z").unwrap();
    assert_eq!(rows(&bob.query("SELECT COUNT(*) FROM T").unwrap()), ["1"], "a new password ends none");
    ann.query("DROP USER bob").unwrap();
    let dropped = bob.query("VALUES 1").unwrap_err();
    assert_eq!(state(&dropped), ("28000", -919));
    assert!(dropped.message.contains("the user BOB was dropped"), "{}", dropped.message);
    assert!(bob.is_lost());
    assert_eq!(count(&db, "BOB", "T"), "0", "rolled back");
    assert_eq!(rows(&ann.query("VALUES 1").unwrap()), ["1"], "a user still");
    assert_eq!(rows(&x.query("VALUES 1").unwrap()), ["1"], "not a user");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR2a]; [DESIGN] RULES.md
/// D36, D42: a client that sends nothing for
/// SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS while its transaction is active
/// is told so (SQLSTATE 08006) and closed, its transaction rolled back and
/// its locks let go; one outside a transaction waits as long as it likes.
#[test]
fn a_connection_idle_in_a_transaction_is_closed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[(IDLE_IN_TRANSACTION_TIMEOUT_MS, 100)]);
    let mut raw = server.raw();
    raw.hello("X");
    std::thread::sleep(Duration::from_millis(300));
    raw.send(&Request::Query("INSERT INTO T VALUES (1)".into()));
    assert!(matches!(raw.receive(), Some(Reply::Complete(Completed { sqlcode: 0, active: true, .. }))), "outside one, it waited");
    let refused = raw.refusal();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("SERVER.IDLE_IN_TRANSACTION_TIMEOUT_MS"), "{}", refused.message);
    let holder = db.session("X");
    holder.run("DELETE FROM T").unwrap();
    holder.rollback().unwrap();
    assert_eq!(count(&db, "X", "T"), "0", "rolled back, its locks let go");
}
