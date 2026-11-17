//! System-versioned tables [ISO/IEC 9075-2:2023 §4.17.2.3, §11.3, §11.27
//! to §11.30, §15.8, §15.11, §15.14]. A table's system-time period,
//! PERIOD FOR SYSTEM_TIME of its ROW START and ROW END columns, is given by
//! the database: each row written begins at the transaction timestamp and
//! ends at the highest value of its type. WITH SYSTEM VERSIONING keeps the
//! rows changed or deleted as historical system rows, so a statement reads
//! its current rows alone, and its columns are not altered while it is.

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

/// The error `sql` fails with in `s`, rolled back.
fn refused(s: &Session, sql: &str) -> SqlError {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
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

/// The highest TIMESTAMP(6), the end of a current system row.
const HIGHEST: &str = "9999-12-31 23:59:59.999999";

/// Verifies: [ISO/IEC 9075-2:2023 §15.11 GR3b, GR3c, §15.14 GR10a iii 1 C]
/// A row inserted begins at the transaction timestamp and ends at the
/// highest TIMESTAMP(6); one updated by a later transaction begins anew,
/// and a statement reads the current row alone.
#[test]
fn a_row_written_is_stamped_15_11() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 10)"]);
    let first = query(&s, "SELECT S, E FROM ACCOUNTS");
    assert!(first[0].ends_with(HIGHEST), "{first:?}");
    ok(&s, &["UPDATE ACCOUNTS SET BALANCE = 20"]);
    let second = query(&s, "SELECT S, E FROM ACCOUNTS");
    assert!(second[0] > first[0], "{first:?} then {second:?}");
    assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS"), ["1,20"]);
    ok(&s, &["DELETE FROM ACCOUNTS"]);
    assert_eq!(query(&s, "SELECT ID FROM ACCOUNTS"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.44, §15.14 GR10a iii 1 B]
/// Each change of one transaction is stamped with its one transaction
/// timestamp: a row it inserted and then updated is changed in place, and
/// keeps its start, as the rows it inserted alike.
#[test]
fn one_transaction_stamps_its_rows_alike_4_44() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS]);
    for sql in ["INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 10)", "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (2, 10)"] {
        s.run(sql).unwrap();
    }
    s.run("UPDATE ACCOUNTS SET BALANCE = 30 WHERE ID = 1").unwrap();
    assert_eq!(query(&s, "SELECT COUNT(DISTINCT S) FROM ACCOUNTS"), ["1"]);
    s.commit().unwrap();
    assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS ORDER BY ID"), ["1,30", "2,10"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.17 GR16a iii 1, §15.14 GR10a iii 1 A]
/// A transaction that changes a row another began after its own
/// transaction timestamp is refused (2201H).
#[test]
fn a_row_begun_later_is_not_changed_15_14() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    ok(&a, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 10)"]);
    a.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    a.run("INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (2, 10)").unwrap();
    ok(&b, &["UPDATE ACCOUNTS SET BALANCE = 11 WHERE ID = 1"]);
    let e = refused(&a, "UPDATE ACCOUNTS SET BALANCE = 12 WHERE ID = 1");
    assert_eq!(e.sqlstate().as_str(), "2201H", "{e}");
    assert_eq!(query(&a, "SELECT ID, BALANCE FROM ACCOUNTS"), ["1,11"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.17.2.3, §11.7]
/// Constraints judge the current rows alone: a key updated many times, or
/// deleted and inserted again, is no duplicate of its historical rows.
#[test]
fn constraints_judge_current_rows_4_17_2_3() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 10)"]);
    for n in 0..3 {
        ok(&s, &[&format!("UPDATE ACCOUNTS SET BALANCE = {n}")]);
    }
    ok(&s, &["DELETE FROM ACCOUNTS", "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 99)"]);
    let e = refused(&s, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 5)");
    assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS"), ["1,99"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.4 SR6c, §11.3 GR1c, §21.3.8]
/// A system-time period column is NOT NULL implicitly; the table is
/// SYSTEM VERSIONED in TABLES and its period SYSTEM_TIME in PERIODS; its
/// history is no table of the Information Schema.
#[test]
fn a_versioned_table_is_described_11_3() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS]);
    let nullable = "SELECT COLUMN_NAME, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'ACCOUNTS' AND COLUMN_NAME IN ('S', 'E') ORDER BY 1";
    assert_eq!(query(&s, nullable), ["E,NO", "S,NO"]);
    let kinds = "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A'";
    assert_eq!(query(&s, kinds), ["ACCOUNTS,SYSTEM VERSIONED"]);
    let periods = "SELECT PERIOD_NAME, START_COLUMN_NAME, END_COLUMN_NAME FROM INFORMATION_SCHEMA.PERIODS";
    assert_eq!(query(&s, periods), ["SYSTEM_TIME,S,E"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME LIKE '%HISTORY%'"), ["0"]);
    ok(&s, &["ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING RESTRICT"]);
    assert_eq!(query(&s, kinds), ["ACCOUNTS,BASE TABLE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR1, SR2a, SR3, SR2e v 1; §11.4
/// SR6a, SR6b]
/// A table definition is refused WITH SYSTEM VERSIONING of no system-time
/// period, two of them, one of a temporary table, one whose columns are not
/// ROW START and ROW END, or a ROW START column of no period or of no
/// datetime type.
#[test]
fn a_system_time_period_is_defined_so_11_3() {
    let db = database();
    let s = db.session("A");
    let row = |t: &str| format!("S {t} GENERATED ALWAYS AS ROW START, E {t} GENERATED ALWAYS AS ROW END");
    let ts = row("TIMESTAMP");
    refused_by(&s, "CREATE TABLE T (ID INTEGER) WITH SYSTEM VERSIONING", "§11.3 SR3");
    refused_by(&s, &format!("CREATE TABLE T (ID INTEGER, {ts}) WITH SYSTEM VERSIONING"), "§11.3 SR3");
    refused_by(
        &s,
        &format!("CREATE TABLE T (ID INTEGER, {ts}, PERIOD FOR SYSTEM_TIME (S, E), PERIOD FOR SYSTEM_TIME (S, E))"),
        "§11.3 SR1",
    );
    refused_by(&s, &format!("CREATE GLOBAL TEMPORARY TABLE T (ID INTEGER, {ts}, PERIOD FOR SYSTEM_TIME (S, E))"), "§11.3 SR2a");
    refused_by(&s, &format!("CREATE TABLE T (ID INTEGER, {ts})"), "§11.4 SR6a");
    refused_by(&s, &format!("CREATE TABLE T (ID INTEGER, {}, PERIOD FOR SYSTEM_TIME (S, E))", row("INTEGER")), "§11.4 SR6b");
    let plain = "CREATE TABLE T (ID INTEGER, S TIMESTAMP NOT NULL, E TIMESTAMP NOT NULL, PERIOD FOR SYSTEM_TIME (S, E))";
    refused_by(&s, plain, "SR2e v 1");
    refused_by(&s, &format!("CREATE TABLE T (ID INTEGER, {ts}, PERIOD FOR SYSTEM_TIME (E, S))"), "§11.4 SR6a");
    // A system-time period of a table not versioned, and one with an
    // application-time period beside it, are defined.
    let both = format!(
        "CREATE TABLE T (ID INTEGER, A DATE NOT NULL, B DATE NOT NULL, {}, PERIOD FOR SYSTEM_TIME (S, E), PERIOD FOR P (A, B))",
        row("DATE")
    );
    ok(&s, &[&both, "INSERT INTO T (ID, A, B) VALUES (1, DATE '2026-01-01', DATE '2026-02-01')"]);
    assert_eq!(query(&s, "SELECT E FROM T"), ["9999-12-31"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.11 SR13d, §14.12 SR18h iv, §14.15 SR3]
/// A value given a system-time period column by INSERT needs OVERRIDING
/// USER VALUE, which ignores it; DEFAULT needs none; no UPDATE or MERGE
/// sets one.
#[test]
fn system_time_columns_are_given_by_the_database_14_11() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS]);
    let given = "VALUES (1, 10, TIMESTAMP '2000-01-01 00:00:00', TIMESTAMP '2001-01-01 00:00:00')";
    refused_by(&s, &format!("INSERT INTO ACCOUNTS {given}"), "§14.11 SR13d");
    refused_by(&s, &format!("INSERT INTO ACCOUNTS OVERRIDING SYSTEM VALUE {given}"), "§14.11 SR13d");
    ok(&s, &[&format!("INSERT INTO ACCOUNTS OVERRIDING USER VALUE {given}"), "INSERT INTO ACCOUNTS VALUES (2, 20, DEFAULT, DEFAULT)"]);
    let ends = query(&s, "SELECT E FROM ACCOUNTS ORDER BY ID");
    assert_eq!(ends, [HIGHEST, HIGHEST]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM ACCOUNTS WHERE S < TIMESTAMP '2002-01-01 00:00:00'"), ["0"]);
    refused_by(&s, "UPDATE ACCOUNTS SET S = TIMESTAMP '2000-01-01 00:00:00'", "§14.15 SR3");
    refused_by(&s, "UPDATE ACCOUNTS SET E = DEFAULT", "§14.15 SR3");
    let merge = "MERGE INTO ACCOUNTS USING (VALUES (3)) AS N (ID) ON ACCOUNTS.ID = N.ID \
        WHEN NOT MATCHED THEN INSERT VALUES (N.ID, 0, TIMESTAMP '2000-01-01 00:00:00', TIMESTAMP '2001-01-01 00:00:00')";
    refused_by(&s, merge, "§14.12 SR18h iv");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.11 SR2, §11.12 SR2, SR9, §11.23 SR3,
/// §14.10 SR3, §11.3 SR9b i, §11.28 SR2a]
/// The columns of a system-versioned table are not added, altered or
/// dropped, nor its rows truncated, nor is it the source of a LIKE, nor its
/// system-time period dropped; a system-time period column has no default.
#[test]
fn a_versioned_table_keeps_its_columns_11_11() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS]);
    refused_by(&s, "ALTER TABLE ACCOUNTS ADD COLUMN X INTEGER", "§11.11 SR2");
    refused_by(&s, "ALTER TABLE ACCOUNTS ALTER COLUMN BALANCE SET DEFAULT 0", "§11.12 SR2");
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP COLUMN BALANCE CASCADE", "§11.23 SR3");
    refused_by(&s, "TRUNCATE TABLE ACCOUNTS", "§14.10 SR3");
    refused_by(&s, "CREATE TABLE COPY (LIKE ACCOUNTS)", "§11.3 SR9b i");
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP PERIOD FOR SYSTEM_TIME CASCADE", "§11.28 SR2a");
    ok(&s, &["ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING CASCADE", "ALTER TABLE ACCOUNTS ADD COLUMN X INTEGER"]);
    refused_by(&s, "ALTER TABLE ACCOUNTS ALTER COLUMN S SET DEFAULT CURRENT_TIMESTAMP", "§11.12 SR9");
    refused_by(&s, "ALTER TABLE ACCOUNTS DROP COLUMN S CASCADE", "§11.23");
    ok(&s, &["TRUNCATE TABLE ACCOUNTS", "CREATE TABLE COPY (LIKE ACCOUNTS)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.9 SR1, §11.47 SR5, §11.49 SR12,
/// §11.7 SR6b, §11.8 SR10, §11.4 SR6a]
/// No CHECK, assertion over a system-versioned table, or trigger's action
/// references a system-time period column, nor is one of a unique key or
/// a foreign key; ADD COLUMN adds no ROW START column.
#[test]
fn system_time_columns_are_referenced_by_no_constraint_11_9() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "CREATE TABLE LOG (N INTEGER)"]);
    refused_by(&s, "ALTER TABLE ACCOUNTS ADD CONSTRAINT C CHECK (S > TIMESTAMP '2000-01-01 00:00:00')", "§11.9 SR1");
    refused_by(&s, "CREATE ASSERTION A CHECK (NOT EXISTS (SELECT * FROM ACCOUNTS WHERE E < S))", "§11.47 SR5");
    let trigger = "CREATE TRIGGER G AFTER UPDATE ON ACCOUNTS REFERENCING NEW ROW AS N FOR EACH ROW \
        INSERT INTO LOG VALUES (EXTRACT(YEAR FROM N.S))";
    refused_by(&s, trigger, "§11.49 SR12");
    refused_by(&s, "ALTER TABLE ACCOUNTS ADD CONSTRAINT U UNIQUE (ID, S)", "§11.7 SR6b");
    let fk = "CREATE TABLE STAMPS (ID INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
        E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E), \
        FOREIGN KEY (S) REFERENCES TIMES (T))";
    ok(&s, &["CREATE TABLE TIMES (T TIMESTAMP(6) PRIMARY KEY)"]);
    refused_by(&s, fk, "§11.8 SR10");
    ok(&s, &["ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING CASCADE"]);
    refused_by(&s, "ALTER TABLE ACCOUNTS ADD COLUMN R TIMESTAMP GENERATED ALWAYS AS ROW START", "§11.4 SR6a");
    // Their other columns, and assertions over a table not versioned, are
    // referenced as any.
    ok(
        &s,
        &[
            "ALTER TABLE ACCOUNTS ADD CONSTRAINT C CHECK (BALANCE >= 0)",
            "CREATE ASSERTION A CHECK (NOT EXISTS (SELECT * FROM ACCOUNTS WHERE E < S))",
        ],
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.29, §11.30]
/// ADD SYSTEM VERSIONING makes a table of a system-time period
/// system-versioned, and is refused of one of none, one versioned already
/// or a temporary table; DROP SYSTEM VERSIONING of one not versioned is
/// refused.
#[test]
fn versioning_is_added_and_dropped_11_29() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE PLAIN (ID INTEGER)", "CREATE GLOBAL TEMPORARY TABLE TEMP (ID INTEGER)"]);
    refused_by(&s, "ALTER TABLE PLAIN ADD SYSTEM VERSIONING", "§11.29 SR4");
    refused_by(&s, "ALTER TABLE TEMP ADD SYSTEM VERSIONING", "§11.29 SR2");
    refused_by(&s, "ALTER TABLE PLAIN DROP SYSTEM VERSIONING RESTRICT", "§11.30 SR2");
    ok(&s, &[ACCOUNTS]);
    refused_by(&s, "ALTER TABLE ACCOUNTS ADD SYSTEM VERSIONING", "§11.29 SR5");
    ok(
        &s,
        &[
            "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 1)",
            "ALTER TABLE ACCOUNTS DROP SYSTEM VERSIONING CASCADE",
            "ALTER TABLE ACCOUNTS ADD SYSTEM VERSIONING",
            "UPDATE ACCOUNTS SET BALANCE = 2",
        ],
    );
    assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS"), ["1,2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.27 SR4, GR1, §11.28 GR1]
/// ADD PERIOD FOR SYSTEM_TIME adds its ROW START and ROW END columns, each
/// row there beginning at the transaction timestamp and ending at the
/// highest value; without them, or of others, it is refused. DROP PERIOD
/// FOR SYSTEM_TIME drops both columns.
#[test]
fn a_system_time_period_is_added_with_its_columns_11_27() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE T (ID INTEGER)", "INSERT INTO T VALUES (1), (2)"]);
    let start = "ADD COLUMN S DATE GENERATED ALWAYS AS ROW START";
    let end = "ADD COLUMN E DATE GENERATED ALWAYS AS ROW END";
    refused_by(&s, "ALTER TABLE T ADD PERIOD FOR SYSTEM_TIME (S, E)", "§11.27 SR4c");
    refused_by(&s, &format!("ALTER TABLE T ADD PERIOD FOR SYSTEM_TIME (S, E) {end} {start}"), "§11.27 SR4d");
    refused_by(&s, &format!("ALTER TABLE T ADD PERIOD FOR SYSTEM_TIME (E, S) {start} {end}"), "§11.27 SR4e");
    refused_by(&s, &format!("ALTER TABLE T ADD PERIOD FOR P (S, E) {start} {end}"), "§11.27 SR5d");
    ok(&s, &[&format!("ALTER TABLE T ADD PERIOD FOR SYSTEM_TIME (S, E) {start} {end}")]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM T WHERE S = CURRENT_DATE AND E = DATE '9999-12-31'"), ["2"]);
    refused_by(&s, &format!("ALTER TABLE T ADD PERIOD FOR SYSTEM_TIME (S, E) {start} {end}"), "§11.27 SR4a");
    ok(&s, &["ALTER TABLE T ADD SYSTEM VERSIONING", "UPDATE T SET ID = ID + 10"]);
    ok(&s, &["ALTER TABLE T DROP SYSTEM VERSIONING CASCADE", "ALTER TABLE T DROP PERIOD FOR SYSTEM_TIME RESTRICT"]);
    assert_eq!(query(&s, "SELECT * FROM T ORDER BY ID"), ["11", "12"]);
    refused_by(&s, "ALTER TABLE T DROP PERIOD FOR SYSTEM_TIME RESTRICT", "§11.28 SR2b");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.31, §11.2]
/// A system-versioned table is dropped with its history, by DROP TABLE or
/// DROP SCHEMA, and one of its name defined again begins anew; one
/// defined in CREATE SCHEMA is versioned.
#[test]
fn a_versioned_table_is_dropped_whole_11_31() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 1)", "UPDATE ACCOUNTS SET BALANCE = 2"]);
    ok(&s, &["DROP TABLE ACCOUNTS CASCADE", "CREATE TABLE ACCOUNTS (ID INTEGER)"]);
    let plain = "SELECT TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'ACCOUNTS'";
    assert_eq!(query(&s, plain), ["BASE TABLE"], "its history dropped with it");
    ok(&s, &["DROP TABLE ACCOUNTS CASCADE", ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 3)"]);
    assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS"), ["1,3"]);
    db.schema(&format!("CREATE SCHEMA B AUTHORIZATION A {ACCOUNTS}")).unwrap();
    let kinds = "SELECT TABLE_SCHEMA, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'ACCOUNTS' ORDER BY 1";
    assert_eq!(query(&s, kinds), ["A,SYSTEM VERSIONED", "B,SYSTEM VERSIONED"]);
    ok(&s, &["UPDATE B.ACCOUNTS SET BALANCE = 0", "DROP SCHEMA B CASCADE"]);
    assert_eq!(query(&s, kinds), ["A,SYSTEM VERSIONED"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.17.2.3] [DESIGN] RULES.md D82
/// A system-versioned table, and its rows, stay so as the database is
/// closed and opened again, through a checkpoint or the log.
#[test]
fn a_versioned_table_is_kept_4_17_2_3() {
    let scratch = Scratch::new("system-versioned-reopen");
    let dir = scratch.path("db");
    let check = |db: &Database| {
        let s = db.session("A");
        assert_eq!(query(&s, "SELECT TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'ACCOUNTS'"), ["SYSTEM VERSIONED"]);
        assert_eq!(query(&s, "SELECT ID, BALANCE FROM ACCOUNTS ORDER BY ID"), ["1,2", "2,5"]);
        refused_by(&s, "ALTER TABLE ACCOUNTS ADD COLUMN X INTEGER", "§11.11 SR2");
    };
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let s = db.session("A");
        ok(&s, &[ACCOUNTS, "INSERT INTO ACCOUNTS (ID, BALANCE) VALUES (1, 1), (2, 5)", "UPDATE ACCOUNTS SET BALANCE = 2 WHERE ID = 1"]);
        check(&db);
        db.close().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        check(&db);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    check(&db);
}
