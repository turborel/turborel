//! Isolation levels below SERIALIZABLE [ISO/IEC 9075:1992 §4.28, §14.1
//! GR5] (RULES.md D29): REPEATABLE READ reads at one snapshot, READ
//! COMMITTED and READ UNCOMMITTED at one for each statement, while no
//! cursor is open; neither checks its reads at commit, so write skew
//! commits, but no update is lost, no dirty read is made, and constraints
//! are checked serializably at every level.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{exec, rows};

/// The levels below SERIALIZABLE a transaction may write at: READ
/// UNCOMMITTED is READ ONLY [ISO/IEC 9075:1992 §14.1 SR3, SR4].
const WEAKER: [&str; 2] = ["READ COMMITTED", "REPEATABLE READ"];

/// A database of `T (K, V)` holding (1, 10) and (2, 20), committed.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    s.commit().unwrap();
    db
}

/// A session of `db` whose next transaction is at `level`.
fn at_level(db: &Database, level: &str) -> Session {
    let s = db.session("A");
    s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    s
}

/// The value of row `k` of `T`, as `s` reads it.
fn value(s: &Session, k: i32) -> String {
    rows(s, &format!("SELECT V FROM T WHERE K = {k}")).unwrap().concat()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, Table 9, §14.1 GR5] no level reads
/// a change not committed (P1); READ COMMITTED and READ UNCOMMITTED read,
/// at each statement, what committed before it (P2 possible); REPEATABLE
/// READ and SERIALIZABLE read again what they read (P2 not possible).
#[test]
fn each_level_reads_what_its_snapshot_holds_4_28() {
    for (level, again) in [("READ UNCOMMITTED", "11"), ("READ COMMITTED", "11"), ("REPEATABLE READ", "10"), ("SERIALIZABLE", "10")] {
        let db = database();
        let (s, other) = (at_level(&db, level), db.session("A"));
        assert_eq!(value(&s, 1), "10", "{level}");
        other.run("UPDATE T SET V = 11 WHERE K = 1").unwrap();
        assert_eq!(value(&s, 1), "10", "{level}: no dirty read");
        other.commit().unwrap();
        assert_eq!(value(&s, 1), again, "{level}: a read again");
        s.commit().unwrap();
        assert_eq!(value(&s, 1), "11", "{level}: a new transaction");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] SERIALIZABLE refuses two
/// transactions that each read both rows and change one (write skew)
/// with a serialization failure (SQLCODE -911); below it they commit, as
/// their reads are not checked [§14.1 GR5].
#[test]
fn write_skew_commits_only_below_serializable_4_28() {
    for level in WEAKER.into_iter().chain(["SERIALIZABLE"]) {
        let db = database();
        let (s, other) = (at_level(&db, level), at_level(&db, level));
        assert_eq!(rows(&s, "SELECT SUM(V) FROM T"), Ok(vec!["30".into()]));
        assert_eq!(rows(&other, "SELECT SUM(V) FROM T"), Ok(vec!["30".into()]));
        s.run("UPDATE T SET V = V - 10 WHERE K = 1").unwrap();
        other.run("UPDATE T SET V = V - 20 WHERE K = 2").unwrap();
        s.commit().unwrap();
        let second = other.commit().map_err(|e| e.code);
        if level == "SERIALIZABLE" {
            assert_eq!(second, Err(Code::Conflict), "{level}");
        } else {
            assert_eq!(second, Ok(()), "{level}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] no level loses an update: the
/// second writer of a row is refused (SQLCODE -911) while the first is
/// open, and a write over a change committed after the snapshot it reads
/// at is refused too; READ COMMITTED's statement reads at a snapshot of
/// its own, which holds the change, and writes over it.
#[test]
fn no_update_is_lost_at_any_level_4_28() {
    for level in WEAKER {
        let db = database();
        let (s, other) = (at_level(&db, level), db.session("A"));
        assert_eq!(value(&s, 1), "10");
        other.run("UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
        assert_eq!(exec(&s, "UPDATE T SET V = V + 100 WHERE K = 1"), Err(Code::Conflict), "{level}: a writer open");
        other.commit().unwrap();
        let after = exec(&s, "UPDATE T SET V = V + 100 WHERE K = 1").map(drop);
        s.commit().unwrap();
        if level == "REPEATABLE READ" {
            assert_eq!(after, Err(Code::Conflict), "{level}: committed after the snapshot");
            assert_eq!(value(&s, 1), "11");
        } else {
            assert_eq!(after, Ok(()), "{level}");
            assert_eq!(value(&s, 1), "111", "{level}: over the change, not instead of it");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §14.1 GR5] while a cursor of a READ
/// COMMITTED transaction is open, its statements read at the snapshot the
/// cursor reads at, so the cursor's rows are never read at a later one, nor
/// a row written over a change the cursor did not see;
/// once it is closed, a statement reads what committed before it.
#[test]
fn read_committed_keeps_its_snapshot_while_a_cursor_is_open_14_1() {
    let db = database();
    let (s, other) = (at_level(&db, "READ COMMITTED"), db.session("A"));
    let mut cursor = s.open_cursor("SELECT V FROM T").unwrap();
    assert_eq!(cursor.fetch().unwrap().map(|r| r[0].to_string()), Some("10".into()));
    other.run("UPDATE T SET V = 21 WHERE K = 2").unwrap();
    other.commit().unwrap();
    assert_eq!(value(&s, 2), "20", "the cursor's snapshot");
    assert_eq!(cursor.fetch().unwrap().map(|r| r[0].to_string()), Some("20".into()));
    assert_eq!(exec(&s, "UPDATE T SET V = V + 1 WHERE K = 2"), Err(Code::Conflict), "a write at the cursor's snapshot");
    drop(cursor);
    assert_eq!(value(&s, 2), "21", "a snapshot of its own");
    exec(&s, "UPDATE T SET V = V + 1 WHERE K = 2").unwrap();
    s.commit().unwrap();
    assert_eq!(value(&s, 2), "22");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] the checks of constraints are
/// serializable at every level: a child row inserted under a parent a
/// transaction committed the deletion of meanwhile is refused at commit
/// (SQLCODE -911), at every level, so no row is left referencing nothing.
#[test]
fn constraints_are_checked_serializably_at_every_level_4_28() {
    for level in WEAKER {
        let db = Database::new();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION A
               CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY)
               CREATE TABLE C (K INTEGER REFERENCES P)",
        )
        .unwrap();
        let s = at_level(&db, level);
        s.run("INSERT INTO P VALUES (1)").unwrap();
        s.commit().unwrap();
        let (s, other) = (at_level(&db, level), at_level(&db, level));
        s.run("INSERT INTO C VALUES (1)").unwrap();
        other.run("DELETE FROM P WHERE K = 1").unwrap();
        other.commit().unwrap();
        assert_eq!(s.commit().map_err(|e| e.code), Err(Code::Conflict), "{level}");
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM C"), Ok(vec!["0".into()]), "{level}");
    }
}
