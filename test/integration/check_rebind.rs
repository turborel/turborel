//! CHECK conditions calling registered functions `[EXTENSION]`. The
//! geometry routines kept in a CHECK are bound again, whatever
//! SQL.FUNCTION_CALLS is then, after reopen, recovery and ALTER (RULES.md
//! D74). A registered function not deterministic is refused where the
//! standard wants a deterministic condition (RULES.md D133) [ISO/IEC
//! 9075-2:2023 §11.9 SR5, CR2, §11.4 SR10d, §9.16 SR1am].

use std::sync::atomic::{AtomicI64, Ordering};

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::{Code, Result, fail};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

use crate::common::{Scratch, after_crash, exec, rows};

/// TICK(n), n plus a count of the calls before, so not deterministic.
struct Tick(AtomicI64);
impl ScalarFunction for Tick {
    fn name(&self) -> &'static str {
        "TICK"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [DataType::Exact { .. }] => Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric }),
            _ => fail(Code::SyntaxRule, "TICK takes one exact number"),
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let ticks = self.0.fetch_add(1, Ordering::Relaxed);
        Ok(match &args[0] {
            Value::Exact(d) => Value::Exact(d.checked_add(Decimal::new(ticks.into(), 0))?),
            _ => Value::Null,
        })
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// The definitions of schema A, each CHECK calling a geometry routine:
/// a column's, a table's, a domain's and an assertion.
const DEFINITIONS: [&str; 4] = [
    "CREATE DOMAIN WEST AS ST_POINT CHECK (ST_X(VALUE) < 100)",
    "CREATE TABLE SITE (ID INTEGER, P ST_POINT CHECK (ST_X(P) >= 0), W WEST, CONSTRAINT NORTH CHECK (P.ST_Y() >= 0))",
    "CREATE ASSERTION NOT_ORIGIN CHECK (NOT EXISTS (SELECT * FROM SITE WHERE ST_X(P) = 0 AND ST_Y(P) = 0))",
    "CREATE TABLE TALLY (N INTEGER)",
];

/// Makes `DEFINITIONS` in a session of A whose setting takes the calls,
/// then sets the database's to take none.
fn define(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    a.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    for sql in DEFINITIONS {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("OFF".into())).unwrap();
}

/// Inserts the site `id` at (`x`, `y`), its W at (`w`, 1), in session `a`.
fn site(a: &Session, id: i32, (x, y): (i32, i32), w: i32) -> std::result::Result<(), Code> {
    let sql = format!("INSERT INTO SITE VALUES ({id}, NEW ST_POINT({x}, {y}), NEW ST_POINT({w}, 1))");
    let done = exec(a, &sql).map(|_| ());
    if done.is_err() {
        a.rollback().unwrap();
    }
    done
}

/// Checks each CHECK of `DEFINITIONS` in a session of A that takes no
/// calls of registered functions itself, with the new site `id`.
fn checked(db: &Database, id: i32) {
    let a = db.session("A");
    assert_eq!(exec(&a, "SELECT ST_X(P) FROM SITE"), Err(Code::Syntax), "the session's own call is refused");
    assert_eq!(site(&a, id, (-1, 1), 1), Err(Code::Constraint), "column CHECK");
    assert_eq!(site(&a, id, (1, -1), 1), Err(Code::Constraint), "table CHECK");
    assert_eq!(site(&a, id, (1, 1), 100), Err(Code::Constraint), "domain CHECK");
    assert_eq!(site(&a, id, (0, 0), 1), Err(Code::Constraint), "assertion");
    assert_eq!(site(&a, id, (id, 1), 1), Ok(()), "a row each CHECK takes");
    a.commit().unwrap();
}

/// Opens the database in `dir`, kept in files.
fn open(dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D74, D25: a column's, a table's and a
/// domain's CHECK and an assertion calling geometry routines, made while
/// the session took the calls, are bound again and checked by sessions
/// that take none. So they are as made, after the database is closed and
/// opened from its checkpoint image, and after a crash, replayed from the
/// log.
#[test]
fn geometry_checks_hold_after_reopen_and_recovery_with_calls_off() {
    let scratch = Scratch::new("check-rebind");
    let dir = scratch.path("db");
    let db = open(&dir);
    define(&db);
    checked(&db, 1);
    db.close().unwrap();
    drop(db);
    let db = open(&dir);
    checked(&db, 2);
    db.close().unwrap();
    drop(db);
    // Made again, then crashed before any checkpoint, so replayed.
    std::fs::remove_dir_all(&dir).unwrap();
    let db = open(&dir);
    define(&db);
    drop(db);
    let db = open(&dir);
    checked(&db, 3);
    drop(db);
    let db = open(&dir);
    checked(&db, 4);
    assert_eq!(rows(&db.session("A"), "SELECT ID FROM SITE ORDER BY ID"), Ok(vec!["3".into(), "4".into()]));
}

/// Verifies: [EXTENSION] RULES.md D74: with SQL.FUNCTION_CALLS OFF, ALTER
/// TABLE and ALTER DOMAIN bind the table's and domain's CHECKs calling
/// geometry routines again and keep them; a new CHECK or generated column
/// calling one is refused as it is written, and taken from a session
/// whose setting is ON, then checked after a crash with the setting OFF.
/// A regression test: ALTER TABLE took such a call while the session's
/// setting was OFF.
#[test]
fn alter_keeps_geometry_checks_and_a_new_one_needs_the_setting() {
    let scratch = Scratch::new("check-rebind-alter");
    let dir = scratch.path("db");
    let db = open(&dir);
    define(&db);
    let a = db.session("A");
    for sql in [
        "ALTER TABLE SITE ADD COLUMN NOTE INTEGER",
        "ALTER TABLE SITE ALTER COLUMN NOTE SET DEFAULT 1",
        "ALTER TABLE SITE DROP COLUMN NOTE RESTRICT",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.run("ALTER DOMAIN WEST SET DEFAULT NULL").unwrap();
    a.commit().unwrap();
    checked(&db, 1);
    let east = "ALTER TABLE SITE ADD CONSTRAINT EAST CHECK (ST_X(P) < 50)";
    assert_eq!(exec(&a, east), Err(Code::Syntax), "a new CHECK is checked against the session's setting");
    assert_eq!(exec(&a, "ALTER DOMAIN WEST ADD CONSTRAINT LOW CHECK (ST_Y(VALUE) < 50)"), Err(Code::Syntax));
    assert_eq!(exec(&a, "ALTER TABLE SITE ADD COLUMN GX DOUBLE PRECISION GENERATED ALWAYS AS (ST_X(P))"), Err(Code::Syntax));
    a.rollback().unwrap();
    a.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    a.run(east).unwrap();
    a.run("ALTER DOMAIN WEST ADD CONSTRAINT LOW CHECK (ST_Y(VALUE) < 50)").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let db = open(&dir);
    checked(&db, 2);
    let a = db.session("A");
    assert_eq!(site(&a, 3, (60, 1), 1), Err(Code::Constraint), "the CHECK added");
    assert_eq!(exec(&a, "INSERT INTO SITE VALUES (3, NEW ST_POINT(3, 1), NEW ST_POINT(1, 60))"), Err(Code::Constraint), "the domain's");
}

/// The statements, each with a call of TICK where the standard wants a
/// deterministic expression.
const NONDETERMINISTIC: [&str; 7] = [
    "CREATE TABLE T1 (A INTEGER CHECK (TICK(A) > 0))",
    "CREATE TABLE T2 (A INTEGER, CHECK (TICK(A) > 0))",
    "ALTER TABLE TALLY ADD CONSTRAINT TICKED CHECK (TICK(N) > 0)",
    "CREATE DOMAIN TICKED AS INTEGER CHECK (TICK(VALUE) > 0)",
    "CREATE ASSERTION TICKED CHECK (NOT EXISTS (SELECT * FROM TALLY WHERE TICK(N) < 0))",
    "CREATE TABLE T3 (A INTEGER, B INTEGER GENERATED ALWAYS AS (TICK(A)))",
    "CREATE INDEX TICKED ON TALLY ((TICK(N)))",
];

/// Verifies: [ISO/IEC 9075-2:2023 §11.9 SR5, CR2, §11.4 SR10d, §9.16
/// SR1am] [EXTENSION] RULES.md D133: a registered function that declares
/// itself not deterministic is a potential source of non-determinism, so
/// a column's, a table's or a domain's CHECK, one added by ALTER TABLE,
/// an assertion, a generation expression or a key part of an index
/// calling it is refused, SQLCODE -103; a query calls it.
#[test]
fn a_registered_function_not_deterministic_is_refused_in_a_check() {
    let db = Database::new();
    db.register_scalar_function(Box::new(Tick(AtomicI64::new(0)))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE TALLY (N INTEGER)").unwrap();
    let a = db.session("A");
    for sql in NONDETERMINISTIC {
        let refused = a.direct(sql).map(|_| ()).unwrap_err();
        assert_eq!(refused.code, Code::SyntaxRule, "{sql}: {refused:?}");
        assert!(refused.msg.contains("TICK"), "{sql}: {}", refused.msg);
        a.rollback().unwrap();
    }
    a.run("INSERT INTO TALLY VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT TICK(N) FROM TALLY"), Ok(vec!["1".into()]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.9 SR5, §11.4 SR10d] [EXTENSION]
/// RULES.md D133: the geometry routines declare themselves deterministic,
/// so a generation expression and a CHECK of the same table call them.
#[test]
fn geometry_routines_are_deterministic_in_checks_and_generated_columns() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    a.run("CREATE TABLE R (P ST_POINT CHECK (P.ST_X() <> 7), X DOUBLE PRECISION GENERATED ALWAYS AS (ST_X(P)))").unwrap();
    a.run("INSERT INTO R (P) VALUES (NEW ST_POINT(2, 3))").unwrap();
    assert_eq!(rows(&a, "SELECT X FROM R"), Ok(vec!["2E0".into()]));
    assert_eq!(exec(&a, "INSERT INTO R (P) VALUES (NEW ST_POINT(7, 3))"), Err(Code::Constraint));
}
