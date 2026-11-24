//! A system-versioned table read FOR SYSTEM_TIME [ISO/IEC 9075-2:2023
//! §7.6 SR9c, SR9d, GR2c ii 1, §11.30 SR3, GR3]: AS OF a point in time,
//! the rows current then; BETWEEN two, or FROM one TO another, the rows
//! current at any time in that span, historical ones among them.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, rows};

/// A database of schema A.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The error `sql` fails with in `s`, read to its last row if a query,
/// rolled back.
fn refused(s: &Session, sql: &str) -> SqlError {
    let outcome = if sql.starts_with("SELECT") {
        s.open_cursor(sql).and_then(|mut c| {
            while c.fetch()?.is_some() {}
            Ok(())
        })
    } else {
        s.run(sql).map(drop)
    };
    let e = outcome.err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    e
}

/// Asserts `sql` fails in `s` by Syntax Rule `cited`.
fn refused_by(s: &Session, sql: &str, cited: &str) {
    let e = refused(s, sql);
    assert_eq!(e.code, Code::SyntaxRule, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// The rows of `sql` in `s`.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// A system-versioned table of accounts.
const ACCOUNTS: &str = "CREATE TABLE ACCOUNTS (ID INTEGER PRIMARY KEY, BALANCE INTEGER, \
    S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
    PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING";

/// The start of the current row of account `id` in `s`, a TIMESTAMP literal.
fn start(s: &Session, id: i32) -> String {
    let found = query(s, &format!("SELECT S FROM ACCOUNTS WHERE ID = {id}"));
    s.commit().unwrap();
    format!("TIMESTAMP '{}'", found.first().unwrap_or_else(|| panic!("no account {id}")))
}

/// Accounts 1 and 2 opened at 10 and 20, then 1 set to 11 and 2 deleted,
/// then 1 set to 12, each in a transaction of its own: the starts of the
/// three versions of account 1.
fn history(s: &Session) -> [String; 3] {
    ok(s, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 10), (2, 20)"]);
    let first = start(s, 1);
    ok(s, &["UPDATE ACCOUNTS SET BALANCE = 11 WHERE ID = 1", "DELETE FROM ACCOUNTS WHERE ID = 2"]);
    let second = start(s, 1);
    ok(s, &["UPDATE ACCOUNTS SET BALANCE = 12 WHERE ID = 1"]);
    [first, second, start(s, 1)]
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1 D I] AS OF a point in
/// time reads the rows whose system-time period contains it, historical
/// ones among them: the start is included and the end excluded.
#[test]
fn as_of_reads_the_rows_current_then() {
    let db = database();
    let s = db.session("A");
    let [first, second, third] = history(&s);
    let at = |t: &str| query(&s, &format!("SELECT ID, BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF {t} ORDER BY ID"));
    assert_eq!(at(&first), ["1,10", "2,20"]);
    assert_eq!(at(&second), ["1,11"]);
    assert_eq!(at(&third), ["1,12"]);
    assert_eq!(at(&format!("{first} - INTERVAL '1' SECOND")), Vec::<String>::new());
    assert_eq!(at("CURRENT_TIMESTAMP"), ["1,12"]);
    assert_eq!(at("TIMESTAMP '9999-12-31 23:59:59.999999'"), Vec::<String>::new());
    // A historical row ends where its successor begins.
    let ended = query(&s, &format!("SELECT E FROM ACCOUNTS FOR SYSTEM_TIME AS OF {first} WHERE ID = 1"));
    assert_eq!(ended, [second.trim_start_matches("TIMESTAMP '").trim_end_matches('\'')]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1 C, D II, D III] BETWEEN
/// reads the rows current at a time from the first to the second, both
/// included; FROM … TO, the second excluded; SYMMETRIC exchanges points
/// out of order, and ASYMMETRIC, as without either, reads none.
#[test]
fn between_and_from_to_read_the_rows_of_a_span() {
    let db = database();
    let s = db.session("A");
    let [first, second, third] = history(&s);
    let read = |spec: &str| query(&s, &format!("SELECT ID, BALANCE FROM ACCOUNTS FOR SYSTEM_TIME {spec} ORDER BY ID, BALANCE"));
    assert_eq!(read(&format!("BETWEEN {first} AND {second}")), ["1,10", "1,11", "2,20"]);
    assert_eq!(read(&format!("FROM {first} TO {second}")), ["1,10", "2,20"]);
    assert_eq!(read(&format!("BETWEEN SYMMETRIC {third} AND {second}")), ["1,11", "1,12"]);
    assert_eq!(read(&format!("BETWEEN ASYMMETRIC {third} AND {second}")), Vec::<String>::new());
    assert_eq!(read(&format!("BETWEEN {third} AND {second}")), Vec::<String>::new());
    // Of a row current at both points, out of order: none, but SYMMETRIC.
    let later = format!("{third} + INTERVAL '1' SECOND");
    assert_eq!(read(&format!("BETWEEN {later} AND {third}")), Vec::<String>::new());
    assert_eq!(read(&format!("BETWEEN SYMMETRIC {later} AND {third}")), ["1,12"]);
    assert_eq!(read(&format!("FROM {second} TO {second}")), Vec::<String>::new());
    assert_eq!(read(&format!("BETWEEN {second} AND {second}")), ["1,11"]);
    assert_eq!(read(&format!("FROM {first} TO {third} + INTERVAL '1' SECOND")), ["1,10", "1,11", "1,12", "2,20"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR9c, GR2c ii 1 B] Points in time
/// of a DATE, or of a timestamp of another precision, are CAST to the
/// start column's type; a ? is of it; a correlation name and a derived
/// column list name the table read so; a table read twice at two times
/// joins its versions.
#[test]
fn points_are_cast_and_the_table_is_named_as_any() {
    let db = database();
    let s = db.session("A");
    let [first, second, _] = history(&s);
    let today = query(&s, "SELECT COUNT(*) FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_DATE + INTERVAL '1' DAY");
    assert_eq!(today, ["1"]);
    let joined = query(
        &s,
        &format!(
            "SELECT O.B, N.BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF {first} AS O (I, B, X, Y) \
             JOIN ACCOUNTS FOR SYSTEM_TIME AS OF {second} N ON N.ID = O.I"
        ),
    );
    assert_eq!(joined, ["10,11"]);
    // A ? of the start column's type, given the value read of it.
    let prepared = s.prepare("SELECT BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF ? WHERE ID = 1").unwrap();
    assert_eq!(prepared.inputs().len(), 1);
    let mut read = s.open_cursor(&format!("SELECT S FROM ACCOUNTS FOR SYSTEM_TIME AS OF {first} WHERE ID = 1")).unwrap();
    let at = read.fetch().unwrap().and_then(|r| r.first().cloned()).unwrap();
    drop(read);
    let mut found = s.open_prepared(&prepared, &[at]).unwrap();
    assert_eq!(found.fetch().unwrap().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>()), Some(vec!["10".to_owned()]));
    drop(found);
    // In a subquery, of a point an outer query gives none.
    let counted = query(&s, &format!("SELECT (SELECT COUNT(*) FROM ACCOUNTS FOR SYSTEM_TIME FROM {first} TO {second}) FROM ACCOUNTS"));
    assert_eq!(counted, ["2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR9d i, SR9d iii] A table read FOR
/// SYSTEM_TIME is system-versioned, a base table; a point in time is a
/// datetime referencing no column, of the query or one it is in.
#[test]
fn a_reading_is_of_a_versioned_table_and_a_constant_point() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "CREATE TABLE PLAIN (A INTEGER)", "CREATE VIEW SEEN AS SELECT * FROM ACCOUNTS"]);
    refused_by(&s, "SELECT * FROM PLAIN FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP", "§7.6 SR9d i");
    refused_by(&s, "SELECT * FROM SEEN FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP", "§7.6 SR9d i");
    refused_by(
        &s,
        "CREATE VIEW V AS WITH W AS (SELECT * FROM ACCOUNTS) SELECT * FROM W FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP",
        "§7.6 SR9d i",
    );
    refused_by(&s, "SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF S", "§7.6 SR9d iii");
    refused_by(&s, "SELECT * FROM PLAIN P WHERE EXISTS (SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF P.A)", "§7.6 SR9d iii");
    refused_by(
        &s,
        "CREATE PROCEDURE AT_TIME (IN T TIMESTAMP) MODIFIES SQL DATA INSERT INTO PLAIN SELECT ID FROM ACCOUNTS FOR SYSTEM_TIME AS OF T",
        "contains no <SQL parameter reference>",
    );
    refused_by(&s, "SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF 1", "§7.6");
    refused_by(&s, "SELECT * FROM ACCOUNTS FOR SYSTEM_TIME FROM CURRENT_DATE TO 'x'", "§7.6");
    let e = refused(&s, "SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS CURRENT_TIMESTAMP");
    assert_eq!(e.code, Code::Syntax, "{e}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.16 SR1s, §11.9 CR2, §7.16 SR22d] A
/// reading other than AS OF CURRENT_TIMESTAMP is a potential source of
/// non-determinism, in no constraint; a view of one is read, not changed.
/// AS OF CURRENT_TIMESTAMP reads the current rows, as the table is read.
#[test]
fn a_reading_of_history_is_nondeterministic_and_read_only() {
    let db = database();
    let s = db.session("A");
    let [first, ..] = history(&s);
    refused_by(
        &s,
        "CREATE ASSERTION OLD CHECK (NOT EXISTS (SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF TIMESTAMP '2000-01-01 00:00:00'))",
        "§9.16 SR1s",
    );
    ok(&s, &["CREATE ASSERTION NOW CHECK (NOT EXISTS (SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP WHERE BALANCE < 0))"]);
    ok(&s, &[&format!("CREATE VIEW BACK AS SELECT ID, BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF {first}")]);
    assert_eq!(query(&s, "SELECT * FROM BACK ORDER BY ID"), ["1,10", "2,20"]);
    let e = refused(&s, "UPDATE BACK SET BALANCE = 0");
    assert!(e.to_string().contains("read-only"), "{e}");
    ok(&s, &["CREATE VIEW NOWS AS SELECT ID, BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP"]);
    ok(&s, &["UPDATE NOWS SET BALANCE = 13"]);
    assert_eq!(query(&s, "SELECT BALANCE FROM ACCOUNTS"), ["13"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.30 SR3, GR3] DROP SYSTEM VERSIONING
/// RESTRICT is refused by a view, assertion, routine or trigger reading
/// the table FOR SYSTEM_TIME; CASCADE drops them, and leaves what reads it
/// otherwise.
#[test]
fn dropping_versioning_drops_what_reads_its_history() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "CREATE TABLE LOG (N INTEGER)"]);
    ok(&s, &["CREATE VIEW PAST AS SELECT * FROM ACCOUNTS FOR SYSTEM_TIME FROM DATE '2000-01-01' TO DATE '3000-01-01'"]);
    ok(&s, &["CREATE VIEW PLAINLY AS SELECT * FROM ACCOUNTS"]);
    ok(&s, &["ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING CASCADE"]);
    refused_by(&s, "SELECT * FROM PAST", "§");
    assert_eq!(query(&s, "SELECT COUNT(*) FROM PLAINLY"), ["0"]);
    ok(&s, &["ALTER TABLE ACCOUNTS ADD SYSTEM VERSIONING"]);
    ok(&s, &["CREATE VIEW PAST AS SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF DATE '2000-01-01'"]);
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING RESTRICT", "§11.30 SR3a");
    ok(&s, &["DROP VIEW PAST CASCADE"]);
    ok(&s, &["CREATE ASSERTION NOW CHECK (NOT EXISTS (SELECT * FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_TIMESTAMP WHERE BALANCE < 0))"]);
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING RESTRICT", "§11.30 SR3c");
    ok(&s, &["DROP ASSERTION NOW"]);
    ok(
        &s,
        &["CREATE PROCEDURE COUNTED () MODIFIES SQL DATA INSERT INTO LOG SELECT COUNT(*) FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_DATE"],
    );
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING RESTRICT", "§11.30 SR3d");
    ok(&s, &["DROP PROCEDURE COUNTED RESTRICT"]);
    ok(
        &s,
        &["CREATE TRIGGER LOGGED AFTER INSERT ON LOG FOR EACH STATEMENT \
         INSERT INTO LOG SELECT COUNT(*) FROM ACCOUNTS FOR SYSTEM_TIME AS OF CURRENT_DATE WHERE 1 = 0"],
    );
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING RESTRICT", "§11.30 SR3e");
    ok(&s, &["ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING CASCADE"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_NAME = 'LOGGED'"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1, §4.17.2.3] The history
/// read FOR SYSTEM_TIME is kept as the database is closed and opened again.
#[test]
fn a_history_is_read_after_reopening() {
    let scratch = Scratch::new("system-time-reopen");
    let dir = scratch.path("db");
    let first;
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let s = db.session("A");
        first = history(&s)[0].clone();
    }
    let db = Database::open(&dir).unwrap();
    let s = db.session("A");
    let at = query(&s, &format!("SELECT ID, BALANCE FROM ACCOUNTS FOR SYSTEM_TIME AS OF {first} ORDER BY ID"));
    assert_eq!(at, ["1,10", "2,20"]);
}
