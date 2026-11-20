//! LOCALTIME and LOCALTIMESTAMP [ISO/IEC 9075-2:2023 §6.43] (Feature
//! F051-07, F051-08): the statement's instant CAST to a time or timestamp
//! WITHOUT TIME ZONE, in the session's time zone displacement, of the
//! precision written; defaults of them [§11.5]; and the words still names
//! of columns where a period follows, or delimited.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, exec, rows};

/// 2024-02-29 12:34:56.789012 UTC, in microseconds from 1970-01-01.
const NOW: i64 = 1_709_210_096_789_012;

/// A database whose clock stands at `NOW`, of schema A with table T of
/// one row, and a session of A.
fn clocked() -> (Database, Session) {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.43 SR2, SR3, GR1 to GR3] LOCALTIME
/// and LOCALTIMESTAMP are CURRENT_TIME and CURRENT_TIMESTAMP CAST to a
/// time and a timestamp WITHOUT TIME ZONE: the same instant in the
/// statement, its fields in the session's displacement, of the precision
/// written, else 0 for LOCALTIME and 6 for LOCALTIMESTAMP [§6.1 SR38];
/// standing alone, as VALUES (LOCALTIMESTAMP) writes it, or with
/// a precision, as VALUES (LOCALTIMESTAMP(0)); described WITHOUT TIME
/// ZONE.
#[test]
fn local_datetimes_6_43() {
    let (_db, a) = clocked();
    assert_eq!(
        rows(&a, "VALUES (LOCALTIME, LOCALTIME(3), LOCALTIMESTAMP, LOCALTIMESTAMP(0))").unwrap(),
        ["12:34:56,12:34:56.789,2024-02-29 12:34:56.789012,2024-02-29 12:34:56"]
    );
    a.run("SET SESSION.TIME_ZONE = '-05:00'").unwrap();
    assert_eq!(rows(&a, "VALUES (localtime(1), localtimestamp(2))").unwrap(), ["07:34:56.7,2024-02-29 07:34:56.78"]);
    let same = "SELECT COUNT(*) FROM T WHERE LOCALTIMESTAMP = CAST(CURRENT_TIMESTAMP AS TIMESTAMP WITHOUT TIME ZONE) \
                AND LOCALTIME(2) = CAST(CURRENT_TIME(2) AS TIME(2) WITHOUT TIME ZONE)";
    assert_eq!(rows(&a, same).unwrap(), ["1"], "SR2, SR3");
    a.run("CREATE VIEW V AS SELECT LOCALTIME AS L, LOCALTIMESTAMP(3) AS S FROM T").unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, DATETIME_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V' ORDER BY 1";
    assert_eq!(rows(&a, described).unwrap(), ["L,TIME,0", "S,TIMESTAMP,3"]);
    assert_eq!(code(&a, "VALUES (LOCALTIME(7))"), Code::Unsupported);
    assert_eq!(code(&a, "VALUES (LOCALTIMESTAMP('A'))"), Code::Syntax);
    a.commit().unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.5 SR4d, GR2c; §6.43] a column's
/// default LOCALTIME or LOCALTIMESTAMP is taken as the statement that
/// takes it runs, in its session's displacement, and is shown as written,
/// of its precision; one of other fields than the column's is refused
/// (SQLCODE -103); none is in a constraint [§11.9 SR5].
#[test]
fn local_datetime_defaults_11_5() {
    let (_db, a) = clocked();
    a.run("CREATE TABLE U (N INTEGER, H TIME DEFAULT LOCALTIME, S TIMESTAMP(2) DEFAULT LOCALTIMESTAMP(2))").unwrap();
    a.run("SET SESSION.TIME_ZONE = '+01:00'").unwrap();
    a.run("INSERT INTO U (N) VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT H, S FROM U").unwrap(), ["13:34:56,2024-02-29 13:34:56.78"]);
    let described =
        "SELECT COLUMN_NAME, COLUMN_DEFAULT FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U' AND COLUMN_NAME <> 'N' ORDER BY 1";
    assert_eq!(rows(&a, described).unwrap(), ["H,LOCALTIME(0)", "S,LOCALTIMESTAMP(2)"]);
    assert_eq!(exec(&a, "CREATE TABLE W (D DATE DEFAULT LOCALTIME)").err(), Some(Code::SyntaxRule));
    assert_eq!(exec(&a, "CREATE TABLE W (S TIMESTAMP, CHECK (S < LOCALTIMESTAMP))").err(), Some(Code::SyntaxRule));
    a.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D330: LOCALTIME and LOCALTIMESTAMP are
/// regular identifiers of a column where a period follows, or delimited,
/// so a column named so before keeps its meaning written that way.
#[test]
fn local_datetime_words_stay_names_6_43() {
    let (_db, a) = clocked();
    a.run("CREATE TABLE W (\"LOCALTIME\" INTEGER, \"LOCALTIMESTAMP\" INTEGER)").unwrap();
    a.run("INSERT INTO W VALUES (1, 2)").unwrap();
    assert_eq!(rows(&a, "SELECT W.LOCALTIME, \"LOCALTIMESTAMP\" FROM W").unwrap(), ["1,2"]);
    assert_eq!(rows(&a, "SELECT \"LOCALTIME\" + 1 FROM W").unwrap(), ["2"]);
}
