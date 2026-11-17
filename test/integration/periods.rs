//! Application-time periods [ISO/IEC 9075-2:2023 §4.16, §11.3, §11.27,
//! §11.28]. PERIOD FOR names a table's start and end columns as a period,
//! with the period constraint CHECK (start < end); CREATE TABLE defines one
//! and ALTER TABLE adds or drops one. A period's columns are not dropped,
//! nor made nullable, while it is.

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

/// The table of an application-time period BOOKED.
const BOOKINGS: &str = "CREATE TABLE BOOKINGS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E))";

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B, GR4e]
/// A table defined with a period has its period constraint, CHECK (S < E),
/// named for the table and the period: a row that starts where it ends or
/// after is refused (23000), and the constraint is in the Information
/// Schema.
#[test]
fn a_period_of_a_table_is_kept_by_its_constraint_11_3() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[BOOKINGS, "INSERT INTO BOOKINGS VALUES (1, DATE '2026-01-01', DATE '2026-01-05')"]);
    for bad in ["(2, DATE '2026-01-05', DATE '2026-01-05')", "(3, DATE '2026-01-06', DATE '2026-01-05')"] {
        let e = refused(&s, &format!("INSERT INTO BOOKINGS VALUES {bad}"));
        assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    }
    let e = refused(&s, "UPDATE BOOKINGS SET E = S");
    assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    assert_eq!(
        query(
            &s,
            "SELECT CONSTRAINT_NAME, CONSTRAINT_TYPE FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS \
             WHERE TABLE_NAME = 'BOOKINGS' AND CONSTRAINT_NAME LIKE '%PERIOD%'"
        ),
        ["BOOKINGS_BOOKED_PERIOD,CHECK"]
    );
    assert_eq!(query(&s, "SELECT COUNT(*) FROM BOOKINGS"), ["1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR2e iii, iv]
/// The columns of a period may be timestamps, and are known not nullable
/// by a PRIMARY KEY as by NOT NULL; the period constraint's name is
/// numbered past one taken.
#[test]
fn a_period_of_timestamps_named_past_a_taken_name_11_3() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE OTHER (K INTEGER CONSTRAINT T_P_PERIOD CHECK (K > 0))",
            "CREATE TABLE T (S TIMESTAMP(3), E TIMESTAMP(3) NOT NULL, PRIMARY KEY (S), PERIOD FOR P (S, E))",
            "INSERT INTO T VALUES (TIMESTAMP '2026-01-01 00:00:00', TIMESTAMP '2026-01-01 00:00:01')",
        ],
    );
    assert_eq!(
        query(
            &s,
            "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'T' AND CONSTRAINT_NAME LIKE '%PERIOD%'"
        ),
        ["T_P_PERIOD_1"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR1, SR2a, SR2b, SR2d, SR2e]
/// CREATE TABLE refuses a second application-time period, a period of a
/// temporary or typed table, a column named as the period, a column that
/// is not one, one column for both ends, types that differ or are not
/// datetimes, a domain's name, a nullable or deferrable column, and an
/// identity or generated one.
#[test]
fn create_table_refuses_a_period_against_its_rules_11_3() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE DOMAIN DAYS AS DATE", "CREATE TYPE PT AS (S DATE, E DATE) NOT FINAL REF IS SYSTEM GENERATED"]);
    for (sql, cited) in [
        ("CREATE TABLE T (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E), PERIOD FOR Q (S, E))", "§11.3 SR1"),
        ("CREATE GLOBAL TEMPORARY TABLE T (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2a"),
        ("CREATE TABLE T (P INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2d"),
        ("CREATE TABLE T (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (X, E))", "§11.3 SR2e i"),
        ("CREATE TABLE T (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, X))", "§11.3 SR2e ii"),
        ("CREATE TABLE T (S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, S))", "§11.3 SR2e"),
        ("CREATE TABLE T (S DATE NOT NULL, E TIMESTAMP NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2e iii"),
        ("CREATE TABLE T (S TIMESTAMP(3) NOT NULL, E TIMESTAMP(6) NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2e iii"),
        ("CREATE TABLE T (S INTEGER NOT NULL, E INTEGER NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2e iii"),
        ("CREATE TABLE T (S DAYS NOT NULL, E DAYS NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2e iii"),
        ("CREATE TABLE T (S DATE, E DATE NOT NULL, PERIOD FOR P (S, E))", "§11.3 SR2e iv"),
        ("CREATE TABLE T (S DATE NOT NULL, E DATE CONSTRAINT N NOT NULL DEFERRABLE, PERIOD FOR P (S, E))", "§11.3 SR2e iv"),
        (
            "CREATE TABLE T (S DATE NOT NULL, E DATE GENERATED ALWAYS AS (S + INTERVAL '1' DAY) NOT NULL, PERIOD FOR P (S, E))",
            "§11.3 SR2e v 2 A",
        ),
    ] {
        refused_by(&s, sql, cited);
    }
    // [SR2b] a typed table's elements are of a <typed table element
    // list>, which has no period.
    assert_eq!(refused(&s, "CREATE TABLE T OF PT (PERIOD FOR P (S, E))").code, Code::Syntax);
    assert!(s.run("SELECT * FROM T").is_err(), "no table T is made");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR2e v 1, §11.27 SR4c, §11.28 SR2b]
/// PERIOD FOR SYSTEM_TIME is no application-time period: its columns are
/// of ROW START and ROW END, not columns the table has, so it is refused
/// of those by CREATE TABLE and ADD PERIOD, and DROP PERIOD of a table
/// with none (`system_versioned.rs`).
#[test]
fn a_system_time_period_is_of_its_own_columns() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[BOOKINGS]);
    refused_by(&s, "CREATE TABLE T (S TIMESTAMP NOT NULL, E TIMESTAMP NOT NULL, PERIOD FOR SYSTEM_TIME (S, E))", "SR2e v 1");
    refused_by(&s, "ALTER TABLE BOOKINGS ADD PERIOD FOR SYSTEM_TIME (S, E)", "§11.27 SR4c");
    refused_by(&s, "ALTER TABLE BOOKINGS DROP PERIOD FOR SYSTEM_TIME RESTRICT", "§11.28 SR2b");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3]
/// PERIOD is a non-reserved word: a column of that name is defined,
/// queried and dropped, PERIOD FOR telling a period apart.
#[test]
fn a_column_named_period_is_a_column() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE T (K INTEGER, PERIOD INTEGER)", "INSERT INTO T VALUES (1, 2)"]);
    assert_eq!(query(&s, "SELECT PERIOD FROM T"), ["2"]);
    ok(&s, &["ALTER TABLE T ADD PERIOD2 INTEGER", "ALTER TABLE T DROP PERIOD CASCADE"]);
    assert_eq!(query(&s, "SELECT * FROM T"), ["1,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.27 GR2]
/// ALTER TABLE ADD PERIOD adds the period and its constraint, which the
/// table's rows are judged by: one whose start is not before its end
/// refuses it (23000), and the table is left as it was.
#[test]
fn add_period_judges_the_rows_11_27() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &["CREATE TABLE T (K INTEGER, S DATE NOT NULL, E DATE NOT NULL)", "INSERT INTO T VALUES (1, DATE '2026-01-01', DATE '2026-01-01')"],
    );
    let e = refused(&s, "ALTER TABLE T ADD PERIOD FOR P (S, E)");
    assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    ok(&s, &["DROP TABLE T CASCADE"]);
    ok(
        &s,
        &[
            "CREATE TABLE T (K INTEGER, S DATE NOT NULL, E DATE NOT NULL)",
            "INSERT INTO T VALUES (1, DATE '2026-01-01', DATE '2026-01-02')",
            "ALTER TABLE T ADD PERIOD FOR P (S, E)",
        ],
    );
    let e = refused(&s, "INSERT INTO T VALUES (2, DATE '2026-01-03', DATE '2026-01-02')");
    assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    refused_by(&s, "ALTER TABLE T ADD PERIOD FOR Q (S, E)", "§11.27 SR5b");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.27 SR2, SR3, SR5]
/// ADD PERIOD refuses a temporary or referenceable table, a column named
/// as the period, columns that are not the table's, of types that differ
/// or are not datetimes, possibly nullable, or generated; of a domain over
/// DATE it adds one, judging the declared type.
#[test]
fn add_period_refuses_against_its_rules_11_27() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE DOMAIN DAYS AS DATE",
            "CREATE TYPE PT AS (S DATE, E DATE) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TABLE TY OF PT (REF IS ID SYSTEM GENERATED)",
            "CREATE GLOBAL TEMPORARY TABLE TEMP (S DATE NOT NULL, E DATE NOT NULL)",
            "CREATE TABLE T (P INTEGER, S DATE NOT NULL, E DATE NOT NULL, N DATE, TS TIMESTAMP NOT NULL, \
             G DATE GENERATED ALWAYS AS (S + INTERVAL '1' DAY) NOT NULL, DS DAYS NOT NULL, DE DAYS NOT NULL)",
        ],
    );
    for (sql, cited) in [
        ("ALTER TABLE TEMP ADD PERIOD FOR Q (S, E)", "§11.27 SR2"),
        ("ALTER TABLE TY ADD PERIOD FOR Q (S, E)", "§11.27 SR3"),
        ("ALTER TABLE T ADD PERIOD FOR P (S, E)", "§11.27 SR5c"),
        ("ALTER TABLE T ADD PERIOD FOR Q (X, E)", "§11.27 SR5e"),
        ("ALTER TABLE T ADD PERIOD FOR Q (S, X)", "§11.27 SR5f"),
        ("ALTER TABLE T ADD PERIOD FOR Q (S, TS)", "§11.27 SR5g"),
        ("ALTER TABLE T ADD PERIOD FOR Q (P, P)", "§11.27 SR5e, SR5f"),
        ("ALTER TABLE T ADD PERIOD FOR Q (S, N)", "§11.27 SR5h"),
        ("ALTER TABLE T ADD PERIOD FOR Q (S, G)", "§11.27 SR5i"),
    ] {
        refused_by(&s, sql, cited);
    }
    ok(&s, &["ALTER TABLE T ADD PERIOD FOR Q (DS, DE)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3a, GR2]
/// DROP PERIOD drops the period and its constraint, RESTRICT or CASCADE,
/// after which a row may end before it starts and the columns may be
/// dropped; a period the table has not is refused.
#[test]
fn drop_period_drops_its_constraint_11_28() {
    let db = database();
    let s = db.session("A");
    ok(&s, &[BOOKINGS]);
    refused_by(&s, "ALTER TABLE BOOKINGS DROP PERIOD FOR OTHER RESTRICT", "§11.28 SR3a");
    ok(&s, &["ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED RESTRICT"]);
    refused_by(&s, "ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED CASCADE", "§11.28 SR3a");
    ok(&s, &["INSERT INTO BOOKINGS VALUES (1, DATE '2026-01-05', DATE '2026-01-01')", "ALTER TABLE BOOKINGS DROP COLUMN E CASCADE"]);
    assert_eq!(
        query(
            &s,
            "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_NAME = 'BOOKINGS' AND CONSTRAINT_NAME LIKE '%PERIOD%'"
        ),
        ["0"]
    );
    ok(&s, &["ALTER TABLE BOOKINGS ADD E DATE DEFAULT DATE '2027-01-01' NOT NULL", "ALTER TABLE BOOKINGS ADD PERIOD FOR BOOKED (S, E)"]);
    ok(&s, &["ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED CASCADE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.23 SR6, §11.26 SR10]
/// While a period is, its columns are not dropped, its constraint is not
/// dropped but with it, and no constraint is dropped whose going makes
/// one of its columns possibly nullable; one that leaves it known not
/// nullable is dropped.
#[test]
fn a_period_keeps_its_columns_11_23_11_26() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &["CREATE TABLE T (K INTEGER, S DATE CONSTRAINT SN NOT NULL, E DATE CONSTRAINT EN NOT NULL, \
           CONSTRAINT PK PRIMARY KEY (E), PERIOD FOR P (S, E))"],
    );
    refused_by(&s, "ALTER TABLE T DROP COLUMN S CASCADE", "§11.23 SR6");
    refused_by(&s, "ALTER TABLE T DROP COLUMN E CASCADE", "§11.23 SR6");
    refused_by(&s, "ALTER TABLE T DROP CONSTRAINT T_P_PERIOD CASCADE", "§11.26 SR10a");
    refused_by(&s, "ALTER TABLE T DROP CONSTRAINT SN CASCADE", "§11.26 SR10b");
    ok(&s, &["ALTER TABLE T DROP CONSTRAINT EN RESTRICT"]);
    refused_by(&s, "ALTER TABLE T DROP CONSTRAINT PK CASCADE", "§11.26 SR10b");
    ok(&s, &["ALTER TABLE T DROP COLUMN K CASCADE", "ALTER TABLE T DROP PERIOD FOR P CASCADE", "ALTER TABLE T DROP CONSTRAINT SN CASCADE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.16.1]
/// A period's descriptor is kept across reopening, from the log and from
/// a checkpoint: its constraint still judges rows and its columns still
/// are not dropped.
#[test]
fn a_period_is_kept_across_reopening_4_16() {
    let scratch = Scratch::new("periods-reopen");
    let dir = scratch.path("db");
    let check = |db: &Database| {
        let s = db.session("A");
        let e = refused(&s, "INSERT INTO BOOKINGS VALUES (2, DATE '2026-01-05', DATE '2026-01-01')");
        assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
        refused_by(&s, "ALTER TABLE BOOKINGS DROP COLUMN S CASCADE", "§11.23 SR6");
    };
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        ok(&db.session("A"), &[BOOKINGS]);
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
    ok(&db.session("A"), &["ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED CASCADE", "ALTER TABLE BOOKINGS DROP COLUMN S CASCADE"]);
    db.close().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.16.1]
/// INFORMATION_SCHEMA.PERIODS shows each period of a table the current
/// user holds a privilege on: its name, its columns and its constraint;
/// another user without one sees none, and a dropped period is gone.
#[test]
fn the_information_schema_shows_periods() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let s = db.session("A");
    ok(&s, &[BOOKINGS]);
    let periods = "SELECT TABLE_SCHEMA, TABLE_NAME, PERIOD_NAME, START_COLUMN_NAME, END_COLUMN_NAME, CONSTRAINT_SCHEMA, CONSTRAINT_NAME \
                   FROM INFORMATION_SCHEMA.PERIODS";
    assert_eq!(query(&s, periods), ["A,BOOKINGS,BOOKED,S,E,A,BOOKINGS_BOOKED_PERIOD"]);
    assert_eq!(query(&db.session("B"), periods), Vec::<String>::new(), "B holds no privilege on BOOKINGS");
    ok(&s, &["GRANT SELECT ON BOOKINGS TO B"]);
    assert_eq!(query(&db.session("B"), periods).len(), 1);
    ok(&s, &["ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED CASCADE"]);
    assert_eq!(query(&s, periods), Vec::<String>::new());
}
