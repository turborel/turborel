//! Definitions that call a registered function `[EXTENSION]`, read again
//! as they were taken when made (RULES.md D74).

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::{Code, Result, fail};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, MAX_EXACT_PRECISION, Spelled, Value};

use crate::common::{Scratch, after_crash, code, exec, rows};

/// BUMP(n), n + 1, of an exact number.
struct Bump;
impl ScalarFunction for Bump {
    fn name(&self) -> &'static str {
        "BUMP"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [DataType::Exact { .. }] => Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric }),
            _ => fail(Code::SyntaxRule, "BUMP takes one exact number"),
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(match &args[0] {
            Value::Exact(d) => Value::Exact(d.checked_add(turborel::value::Decimal::new(1, 0))?),
            _ => Value::Null,
        })
    }
}

/// A database with BUMP registered and schema A.
fn registered() -> Database {
    let db = Database::new();
    db.register_scalar_function(Box::new(Bump)).unwrap();
    db
}

/// Each kind of definition that keeps its text, calling BUMP.
const DEFINITIONS: [&str; 9] = [
    "CREATE TABLE T (A INTEGER CHECK (BUMP(A) > 0), B INTEGER GENERATED ALWAYS AS (BUMP(A)))",
    "CREATE TABLE LOGGED (V INTEGER)",
    "CREATE TRIGGER TR AFTER INSERT ON T REFERENCING NEW ROW AS N FOR EACH ROW INSERT INTO LOGGED VALUES (BUMP(N.A))",
    "CREATE VIEW V (A, P) AS SELECT A, BUMP(A) FROM T",
    "CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM T WHERE BUMP(A) > 100))",
    "CREATE DOMAIN POSITIVE AS INTEGER CHECK (BUMP(VALUE) > 1)",
    "CREATE TABLE D (X POSITIVE)",
    "CREATE FUNCTION BUMPED (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN BUMP(BUMP(X))",
    "CREATE SCHEMA S CREATE VIEW W (P) AS SELECT BUMP(A) FROM A.T",
];

/// Makes `DEFINITIONS` in a session of A, its setting as the database's.
fn define(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for sql in DEFINITIONS {
        a.direct(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
}

/// Uses each definition with the row `n`, in a session that takes no
/// calls of registered functions itself.
fn used(a: &Session, n: i32) {
    a.run("SET SQL.FUNCTION_CALLS = 'OFF'").unwrap();
    assert_eq!(code(a, "SELECT BUMP(1) FROM T"), Code::Syntax, "the session's own calls are refused");
    a.run(&format!("INSERT INTO T (A) VALUES ({n})")).unwrap();
    let bumped = n + 1;
    assert_eq!(rows(a, &format!("SELECT B FROM T WHERE A = {n}")), Ok(vec![bumped.to_string()]), "generated column");
    assert_eq!(rows(a, &format!("SELECT COUNT(*) FROM LOGGED WHERE V = {bumped}")), Ok(vec!["1".into()]), "trigger");
    assert_eq!(rows(a, &format!("SELECT P FROM V WHERE A = {n}")), Ok(vec![bumped.to_string()]), "view");
    assert_eq!(rows(a, &format!("SELECT P FROM S.W WHERE P = {bumped}")), Ok(vec![bumped.to_string()]), "schema's view");
    assert_eq!(rows(a, &format!("SELECT BUMPED(A) FROM T WHERE A = {n}")), Ok(vec![(n + 2).to_string()]), "routine");
    assert_eq!(exec(a, "INSERT INTO T (A) VALUES (-5)"), Err(Code::Constraint), "CHECK");
    assert_eq!(exec(a, "INSERT INTO T (A) VALUES (100)"), Err(Code::Constraint), "assertion");
    assert_eq!(exec(a, "INSERT INTO D VALUES (0)"), Err(Code::Constraint), "domain");
    a.run(&format!("INSERT INTO D VALUES ({n})")).unwrap();
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D74: each kind of definition that keeps
/// its text, made calling a registered function, is used so after it is
/// made, by a session that refuses such calls of its own.
#[test]
fn stored_calls_are_taken_when_used() {
    let db = registered();
    define(&db);
    used(&db.session("A"), 1);
}

/// Verifies: [EXTENSION] RULES.md D74, D25: each such definition is read
/// again so after the database is closed and opened, and after a crash,
/// its definitions replayed from the log.
#[test]
fn stored_calls_survive_reopen_and_crash_recovery() {
    let scratch = Scratch::new("stored-function-calls");
    let dir = scratch.path("db");
    let open = || {
        let db = registered();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    define(&db);
    used(&db.session("A"), 1);
    db.close().unwrap();
    drop(db);
    let db = open();
    used(&db.session("A"), 2);
    db.close().unwrap();
    drop(db);
    // Made again, but crashed before a checkpoint: replayed from the log.
    std::fs::remove_dir_all(&dir).unwrap();
    let db = open();
    define(&db);
    drop(db);
    let db = open();
    used(&db.session("A"), 3);
    drop(db);
    let db = open();
    used(&db.session("A"), 4);
}

/// Verifies: [EXTENSION] RULES.md D74: with SQL.FUNCTION_CALLS OFF, a
/// definition calling a registered function is refused as it is made, by
/// a session or the database; a `<default option>` calls none, either
/// way [ISO/IEC 9075-2:2023 §11.5].
#[test]
fn stored_calls_are_refused_while_off() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (A INTEGER) CREATE TABLE LOGGED (V INTEGER)").unwrap();
    let a = db.session("A");
    a.run("SET SQL.FUNCTION_CALLS = 'OFF'").unwrap();
    for sql in DEFINITIONS.iter().filter(|sql| sql.contains("BUMP")) {
        let sql = sql.replace("CREATE TABLE T ", "CREATE TABLE T2 ");
        assert_eq!(a.direct(&sql).map(|_| ()).map_err(|e| e.code), Err(Code::Syntax), "{sql}");
        a.rollback().unwrap();
    }
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("OFF".into())).unwrap();
    let schema = "CREATE SCHEMA AUTHORIZATION B CREATE VIEW W (P) AS SELECT BUMP(1) FROM (VALUES (1)) AS X (Y)";
    assert_eq!(db.schema(schema).unwrap_err().code, Code::Syntax);
    for on in ["ON", "OFF"] {
        a.run(&format!("SET SQL.FUNCTION_CALLS = '{on}'")).unwrap();
        assert_eq!(exec(&a, "CREATE TABLE E (A INTEGER DEFAULT BUMP(1))"), Err(Code::Syntax), "{on}");
        assert_eq!(exec(&a, "CREATE DOMAIN E AS INTEGER DEFAULT BUMP(1)"), Err(Code::Syntax), "{on}");
        a.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D74: with SQL.FUNCTION_CALLS OFF, a
/// definition made on others that call a registered function is taken,
/// theirs read again as they were made: a view of a view, a column added
/// to a table of a generated column and a CHECK, and a table of a domain.
#[test]
fn older_stored_calls_are_taken_while_off() {
    let db = registered();
    define(&db);
    let a = db.session("A");
    a.run("SET SQL.FUNCTION_CALLS = 'OFF'").unwrap();
    for sql in [
        "CREATE VIEW V2 (Q) AS SELECT P FROM V",
        "ALTER TABLE T ADD COLUMN C INTEGER",
        "ALTER TABLE T ADD CONSTRAINT POSITIVE_C CHECK (C IS NULL OR C > 0)",
        "CREATE TABLE D2 (X POSITIVE)",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
    a.run("INSERT INTO T (A, C) VALUES (7, 1)").unwrap();
    assert_eq!(rows(&a, "SELECT Q FROM V2"), Ok(vec!["8".into()]));
    assert_eq!(exec(&a, "INSERT INTO D2 VALUES (0)"), Err(Code::Constraint));
}
