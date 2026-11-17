//! Bitemporal tables [ISO/IEC 9075-2:2023 §4.17.2, §4.17.2.3]: a table of
//! an application-time period and a system-time period, system-versioned.
//! Each statement of the one keeps the other: UPDATE and DELETE FOR
//! PORTION OF the application-time period keep the rows they change as
//! historical rows, and stamp the parts they keep; a key WITHOUT OVERLAPS
//! and a temporal foreign key judge the current rows alone; FOR
//! SYSTEM_TIME reads what was recorded at a time, of the periods it
//! recorded.

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

/// Asserts `sql` fails in `s` with `code`, its message holding `cited`.
fn refused_by(s: &Session, sql: &str, (code, cited): (Code, &str)) {
    let e = refused(s, sql);
    assert_eq!(e.code, code, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// The rows of `sql` in `s`.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// A bitemporal table of who works where, when.
const STAFF: &str = "CREATE TABLE STAFF (ID INTEGER, DEPT CHAR(1), \
    VF DATE NOT NULL, VT DATE NOT NULL, PERIOD FOR VALID (VF, VT), \
    S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
    PERIOD FOR SYSTEM_TIME (S, E), PRIMARY KEY (ID, VALID WITHOUT OVERLAPS)) WITH SYSTEM VERSIONING";

/// Every row recorded, current or historical.
const EVER: &str = "FOR SYSTEM_TIME FROM DATE '2000-01-01' TO DATE '3000-01-01'";

/// The latest start of a system-time period of STAFF in `s`, a TIMESTAMP
/// literal.
fn recorded(s: &Session) -> String {
    let found = query(s, "SELECT MAX(S) FROM STAFF");
    s.commit().unwrap();
    format!("TIMESTAMP '{}'", found.first().unwrap())
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR2e, SR3, §11.27, §11.29] A
/// table has an application-time period and a system-time period of
/// columns of their own, defined at once or one added to a table of the
/// other, and is system-versioned with both.
#[test]
fn a_table_has_both_periods_4_17_2() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[STAFF]);
    ok(
        &s,
        &[
            "CREATE TABLE LATER (ID INTEGER, VF DATE NOT NULL, VT DATE NOT NULL, \
         S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
         PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING",
            "ALTER TABLE LATER ADD PERIOD FOR VALID (VF, VT)",
            "CREATE TABLE EARLIER (ID INTEGER, VF DATE NOT NULL, VT DATE NOT NULL, PERIOD FOR VALID (VF, VT))",
            "ALTER TABLE EARLIER ADD PERIOD FOR SYSTEM_TIME (S, E) \
         ADD COLUMN S TIMESTAMP(6) GENERATED ALWAYS AS ROW START ADD COLUMN E TIMESTAMP(6) GENERATED ALWAYS AS ROW END",
            "ALTER TABLE EARLIER ADD SYSTEM VERSIONING",
        ],
    );
    let periods = query(&s, "SELECT TABLE_NAME, PERIOD_NAME FROM INFORMATION_SCHEMA.PERIODS ORDER BY 1, 2");
    assert_eq!(periods, ["EARLIER,SYSTEM_TIME", "EARLIER,VALID", "LATER,SYSTEM_TIME", "LATER,VALID", "STAFF,SYSTEM_TIME", "STAFF,VALID"]);
    let types = query(&s, "SELECT DISTINCT TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A'");
    assert_eq!(types, ["SYSTEM VERSIONED"]);
    // An application-time period is of no system-time column.
    refused_by(
        &s,
        "CREATE TABLE SHARED (S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
         PERIOD FOR SYSTEM_TIME (S, E), PERIOD FOR VALID (S, E))",
        (Code::SyntaxRule, "generated column"),
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.8 GR9, §15.14 GR10a iii 1, GR12]
/// UPDATE and DELETE FOR PORTION OF a versioned table keep each row they
/// change as a historical row, and the parts they keep begin at the
/// transaction timestamp; a row changed in the transaction that began it
/// is changed in place.
#[test]
fn a_portion_changed_keeps_its_history_15_14() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[STAFF, "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-01-01', DATE '2030-01-01')"]);
    let before = recorded(&s);
    ok(&s, &["UPDATE STAFF FOR PORTION OF VALID FROM DATE '2024-01-01' TO DATE '2025-01-01' SET DEPT = 'B'"]);
    let current = query(&s, "SELECT DEPT, VF, VT FROM STAFF ORDER BY VF");
    assert_eq!(current, ["A,2020-01-01,2024-01-01", "B,2024-01-01,2025-01-01", "A,2025-01-01,2030-01-01"]);
    assert_eq!(query(&s, &format!("SELECT DEPT, VF, VT FROM STAFF FOR SYSTEM_TIME AS OF {before}")), ["A,2020-01-01,2030-01-01"]);
    let starts = query(&s, "SELECT COUNT(DISTINCT S) FROM STAFF");
    assert_eq!(starts, ["1"]);
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM STAFF {EVER}")), ["4"]);
    ok(&s, &["DELETE FROM STAFF FOR PORTION OF VALID FROM DATE '2020-01-01' TO DATE '2021-01-01'"]);
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM STAFF {EVER}")), ["5"]);
    assert_eq!(query(&s, "SELECT MIN(VF) FROM STAFF"), ["2021-01-01"]);
    // Begun and changed in one transaction: no historical row.
    ok(
        &s,
        &[
            "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (2, 'C', DATE '2020-01-01', DATE '2030-01-01')",
            "UPDATE STAFF FOR PORTION OF VALID FROM DATE '2022-01-01' TO DATE '2023-01-01' SET DEPT = 'D' WHERE ID = 2",
        ],
    );
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM STAFF {EVER} WHERE ID = 2")), ["3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1, §8.20] What was
/// recorded at a system time is read of its application-time periods: a
/// period predicate reads either period, of current and historical rows.
#[test]
fn both_times_are_read_at_once_7_6() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[STAFF, "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-01-01', DATE '2030-01-01')"]);
    let first = recorded(&s);
    ok(&s, &["UPDATE STAFF FOR PORTION OF VALID FROM DATE '2024-01-01' TO DATE '2025-01-01' SET DEPT = 'B'"]);
    // Where 1 worked on 2024-06-01, as first recorded and as now recorded.
    let at = |when: &str| {
        query(&s, &format!("SELECT DEPT FROM STAFF FOR SYSTEM_TIME AS OF {when} WHERE ID = 1 AND VALID CONTAINS DATE '2024-06-01'"))
    };
    assert_eq!(at(&first), ["A"]);
    assert_eq!(at("CURRENT_TIMESTAMP"), ["B"]);
    let both = format!("SELECT DEPT FROM STAFF {EVER} WHERE VALID CONTAINS DATE '2024-06-01' AND SYSTEM_TIME CONTAINS {first}");
    assert_eq!(query(&s, &both), ["A"]);
    let recorded_then =
        format!("SELECT COUNT(*) FROM STAFF {EVER} WHERE SYSTEM_TIME OVERLAPS PERIOD ({first}, {first} + INTERVAL '1' DAY)");
    assert_eq!(query(&s, &recorded_then), ["4"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR6d i, §11.8 SR18a i, §15.18
/// GR7] A key WITHOUT OVERLAPS and a temporal foreign key of versioned
/// tables judge their current rows alone, not their historical rows.
#[test]
fn keys_judge_the_current_rows_11_7() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE DEPTS (D CHAR(1), DF DATE NOT NULL, DT DATE NOT NULL, PERIOD FOR DP (DF, DT), \
         S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
         PERIOD FOR SYSTEM_TIME (S, E), PRIMARY KEY (D, DP WITHOUT OVERLAPS)) WITH SYSTEM VERSIONING",
            STAFF,
            "ALTER TABLE STAFF ADD FOREIGN KEY (DEPT, PERIOD VALID) REFERENCES DEPTS (D, PERIOD DP)",
            "INSERT INTO DEPTS (D, DF, DT) VALUES ('A', DATE '2020-01-01', DATE '2030-01-01')",
            "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-01-01', DATE '2030-01-01')",
        ],
    );
    ok(&s, &["DELETE FROM STAFF FOR PORTION OF VALID FROM DATE '2020-01-01' TO DATE '2021-01-01'"]);
    // The historical row of 2020 holds no key against a new one.
    ok(&s, &["INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-01-01', DATE '2021-01-01')"]);
    let e = refused(&s, "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-06-01', DATE '2020-07-01')");
    assert_eq!(e.code, Code::Constraint, "{e}");
    // The department's period, cut, no longer covers the staff's.
    let e = refused(&s, "DELETE FROM DEPTS FOR PORTION OF DP FROM DATE '2029-01-01' TO DATE '2030-01-01'");
    assert_eq!(e.code, Code::Constraint, "{e}");
    ok(&s, &["DELETE FROM STAFF FOR PORTION OF VALID FROM DATE '2029-01-01' TO DATE '2030-01-01'"]);
    ok(&s, &["DELETE FROM DEPTS FOR PORTION OF DP FROM DATE '2029-01-01' TO DATE '2030-01-01'"]);
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM DEPTS {EVER}")), ["2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.7 SR3, §11.8 SR18a i, §14.9 SR7a,
/// §14.14 SR7a, §11.28 SR2d, GR1d] SYSTEM_TIME, a reserved word, names
/// no application-time period of a key, a foreign key or FOR PORTION OF;
/// a period predicate references it, and refuses DROP PERIOD FOR
/// SYSTEM_TIME RESTRICT.
#[test]
fn system_time_is_no_application_time_period_11_7() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[STAFF]);
    let syntax = (Code::Syntax, "<application time period name>");
    refused_by(&s, "UPDATE STAFF FOR PORTION OF SYSTEM_TIME FROM DATE '2024-01-01' TO DATE '2025-01-01' SET DEPT = 'B'", syntax);
    refused_by(&s, "DELETE FROM STAFF FOR PORTION OF SYSTEM_TIME FROM DATE '2024-01-01' TO DATE '2025-01-01'", syntax);
    refused_by(
        &s,
        "CREATE TABLE K (ID INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, \
         PERIOD FOR SYSTEM_TIME (S, E), UNIQUE (ID, SYSTEM_TIME WITHOUT OVERLAPS))",
        syntax,
    );
    refused_by(
        &s,
        "CREATE TABLE R (ID INTEGER, F TIMESTAMP(6) NOT NULL, T TIMESTAMP(6) NOT NULL, PERIOD FOR P (F, T), \
         FOREIGN KEY (ID, PERIOD P) REFERENCES STAFF (ID, PERIOD SYSTEM_TIME))",
        syntax,
    );
    ok(
        &s,
        &[
            "CREATE TABLE PLAIN (ID INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
         E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E))",
            "CREATE VIEW NOW_PLAIN AS SELECT ID FROM PLAIN WHERE SYSTEM_TIME CONTAINS CURRENT_TIMESTAMP",
        ],
    );
    refused_by(&s, "ALTER TABLE PLAIN DROP PERIOD FOR SYSTEM_TIME RESTRICT", (Code::SyntaxRule, "§11.28 SR2d i"));
    ok(&s, &["ALTER TABLE PLAIN DROP PERIOD FOR SYSTEM_TIME CASCADE"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'NOW_PLAIN'"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3, GR2, §4.17.2.3] The
/// application-time period of a versioned table is dropped, its key with
/// it under CASCADE, its columns and history kept; both periods and the
/// history are kept as the database is closed and opened again.
#[test]
fn a_bitemporal_table_is_kept_11_28() {
    let scratch = Scratch::new("bitemporal-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let s = db.session("A");
        ok(&s, &[STAFF, "INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'A', DATE '2020-01-01', DATE '2030-01-01')"]);
        ok(&s, &["UPDATE STAFF FOR PORTION OF VALID FROM DATE '2024-01-01' TO DATE '2025-01-01' SET DEPT = 'B'"]);
    }
    let db = Database::open(&dir).unwrap();
    let s = db.session("A");
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM STAFF {EVER} WHERE VALID CONTAINS DATE '2024-06-01'")), ["2"]);
    refused_by(&s, "ALTER TABLE STAFF DROP PERIOD FOR VALID RESTRICT", (Code::SyntaxRule, "§11.28 SR3d ii"));
    ok(&s, &["ALTER TABLE STAFF DROP PERIOD FOR VALID CASCADE"]);
    assert_eq!(query(&s, "SELECT PERIOD_NAME FROM INFORMATION_SCHEMA.PERIODS WHERE TABLE_NAME = 'STAFF'"), ["SYSTEM_TIME"]);
    assert_eq!(query(&s, &format!("SELECT COUNT(*) FROM STAFF {EVER}")), ["4"]);
    ok(&s, &["INSERT INTO STAFF (ID, DEPT, VF, VT) VALUES (1, 'C', DATE '2024-01-01', DATE '2025-01-01')"]);
}
