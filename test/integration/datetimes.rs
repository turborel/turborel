//! Datetimes [ISO/IEC 9075:1992 §4.5.1, §6.1]: DATE, TIME and TIMESTAMP,
//! with time zone or not, their literals, the datetime value functions,
//! assignment, comparison, CAST, EXTRACT and defaults.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, sorted};

/// 2024-02-29 12:34:56.789012 UTC, in microseconds from 1970-01-01.
const NOW: i64 = 1_709_210_096_789_012;

/// A database whose clock stands at `NOW`, of schema A with table T of a
/// DATE D, a TIME(2) H, a TIMESTAMP(3) S and a TIMESTAMP WITH TIME ZONE Z.
fn datetimes() -> Database {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (D DATE, H TIME(2), S TIMESTAMP(3), Z TIMESTAMP WITH TIME ZONE)").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1 SR25, SR26; §21.3.5] DATE,
/// TIME and TIMESTAMP, WITH TIME ZONE or not, of a precision written or 0
/// for TIME and 6 for TIMESTAMP, described with it; one of more digits
/// than this implementation's 6 is valid but not processed (SQLCODE -900,
/// RULES.md D18).
#[test]
fn the_datetime_types_6_1() {
    let db = datetimes();
    let a = db.session("A");
    a.run("CREATE TABLE U (A DATE, B TIME, C TIME(3) WITH TIME ZONE, D TIMESTAMP, E TIMESTAMP(0) WITH TIME ZONE)").unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, DATETIME_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U' ORDER BY 1";
    assert_eq!(
        squeezed(&a, described),
        ["A,DATE,0", "B,TIME,0", "C,TIME WITH TIME ZONE,3", "D,TIMESTAMP,6", "E,TIMESTAMP WITH TIME ZONE,0"]
    );
    assert_eq!(exec(&a, "CREATE TABLE W (A TIME(7))").err(), Some(Code::Unsupported), "more than 6 digits");
    assert_eq!(exec(&a, "CREATE TABLE W (A TIME WITH ZONE)").err(), Some(Code::Syntax));
    assert_eq!(exec(&a, "CREATE TABLE W (A DATE(1))").err(), Some(Code::Syntax), "DATE has no precision");
    assert_eq!(exec(&a, "CREATE TABLE W (A TIMESTAMP(9) WITH TIME ZONE)").err(), Some(Code::Unsupported));
    for bad in
        ["SELECT EXTRACT(WEEK FROM D) FROM T", "SELECT EXTRACT(DAY D) FROM T", "SELECT CURRENT_TIMESTAMP(A) FROM T", "SELECT DATE 5 FROM T"]
    {
        assert_eq!(code(&a, bad), Code::Syntax, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR15-SR18, SR21, SR22,
/// SR25; §6.1 GR6] a datetime literal is DATE, TIME or TIMESTAMP, then its
/// string, of four digits of year and two of each other field, valid in
/// the calendar, its displacement from -12:59 to +13:00, of as many
/// digits of fraction as written; one invalid is refused (SQLCODE -103);
/// one whose instant leaves the years 0001 to 9999 is refused with
/// SQLSTATE 22007.
#[test]
fn datetime_literals_5_3() {
    let db = datetimes();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-02-29')").unwrap();
    assert_eq!(
        rows(&a, "SELECT DATE '2024-02-29', TIME '10:00:00.5', TIMESTAMP '2024-01-01 00:00:00+05:30' FROM T").unwrap(),
        ["2024-02-29,10:00:00.5,2024-01-01 00:00:00+05:30"]
    );
    assert_eq!(
        rows(&a, "SELECT TIME '23:59:59-12:59', TIMESTAMP '9999-12-31 23:59:59.999999' FROM T").unwrap(),
        ["23:59:59-12:59,9999-12-31 23:59:59.999999"]
    );
    for bad in
        ["DATE '2023-02-29'", "DATE '24-01-01'", "TIME '24:00:00'", "TIME '12:00:60'", "TIME '12:00:00+13:01'", "TIMESTAMP '2024-01-01'"]
    {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    assert_eq!(code(&a, "SELECT TIME '12:00:00.1234567' FROM T"), Code::Unsupported, "7 digits");
    assert_eq!(code(&a, "SELECT TIMESTAMP '0001-01-01 00:00:00+01:00' FROM T"), Code::Data, "22007: year 0 in UTC");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.2 SR2, GR3l] a datetime is
/// assigned to a column of its fields, its fraction cut to the column's
/// precision; one of other fields, or a value not a datetime, is refused
/// (SQLCODE -103).
#[test]
fn datetimes_are_assigned_9_2() {
    let db = datetimes();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (DATE '2024-01-02', TIME '10:00:00.129', TIMESTAMP '2024-01-02 03:04:05.6789', TIMESTAMP '2024-01-02 03:04:05-05:00')")
        .unwrap();
    assert_eq!(
        rows(&a, "SELECT D, H, S, Z FROM T").unwrap(),
        ["2024-01-02,10:00:00.12,2024-01-02 03:04:05.678,2024-01-02 03:04:05.000000-05:00"]
    );
    assert_eq!(exec(&a, "INSERT INTO T (S) VALUES (DATE '2024-01-02')").err(), Some(Code::SyntaxRule), "a date to a timestamp");
    assert_eq!(exec(&a, "INSERT INTO T (D) VALUES ('2024-01-02')").err(), Some(Code::SyntaxRule), "a character string");
    assert_eq!(exec(&a, "INSERT INTO T (H) VALUES (1)").err(), Some(Code::SyntaxRule), "a number");
    a.run("INSERT INTO T (Z) VALUES (TIMESTAMP '2024-06-01 00:00:00')").unwrap();
    assert_eq!(rows(&a, "SELECT Z FROM T WHERE D IS NULL").unwrap(), ["2024-06-01 00:00:00.000000+00:00"], "of the session's displacement");
    // [§6.1 GR5] one read from a table too, in the session's displacement.
    a.run("SET TIME_ZONE = '+02:00'").unwrap();
    a.run("INSERT INTO T (D, Z) SELECT DATE '2000-01-01', S FROM T WHERE S IS NOT NULL").unwrap();
    a.run("UPDATE T SET Z = S WHERE D = DATE '2024-01-02'").unwrap();
    assert_eq!(
        sorted(&a, "SELECT Z FROM T WHERE S IS NOT NULL OR D = DATE '2000-01-01'"),
        ["2024-01-02 05:04:05.678000+02:00", "2024-01-02 05:04:05.678000+02:00"]
    );
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR5, §4.5.1, §9.3 SR3e]
/// datetimes compare by their instants, whatever their displacements, and
/// with datetimes of their fields alone; they order, group and are
/// distinct so; a union is of their one type, of its greatest precision.
#[test]
fn datetimes_compare_8_2() {
    let db = datetimes();
    let a = db.session("A");
    a.run(
        "INSERT INTO T (D, Z) VALUES (DATE '2024-01-02', TIMESTAMP '2024-01-01 05:30:00+05:30'), \
         (DATE '2023-12-31', TIMESTAMP '2024-01-01 00:00:00+00:00'), (DATE '2024-01-01', TIMESTAMP '2023-12-31 23:00:00-02:00')",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT D FROM T ORDER BY D").unwrap(), ["2023-12-31", "2024-01-01", "2024-01-02"]);
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT Z) FROM T").unwrap(), ["2"], "05:30+05:30 is 00:00 UTC");
    assert_eq!(
        rows(&a, "SELECT D FROM T WHERE Z = TIMESTAMP '2024-01-01 01:00:00+01:00' ORDER BY D").unwrap(),
        ["2023-12-31", "2024-01-02"]
    );
    assert_eq!(rows(&a, "SELECT MAX(Z), MIN(D) FROM T").unwrap(), ["2023-12-31 23:00:00.000000-02:00,2023-12-31"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE D BETWEEN DATE '2024-01-01' AND DATE '2024-12-31'").unwrap(), ["2"]);
    assert_eq!(code(&a, "SELECT D FROM T WHERE D = TIMESTAMP '2024-01-01 00:00:00'"), Code::SyntaxRule, "other fields");
    assert_eq!(code(&a, "SELECT D FROM T WHERE D = '2024-01-01'"), Code::SyntaxRule, "a character string");
    a.run("CREATE VIEW V (X) AS SELECT TIME '01:00:00' FROM T UNION SELECT TIME '01:00:00.25' FROM T").unwrap();
    let described = "SELECT DATA_TYPE, DATETIME_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V'";
    assert_eq!(squeezed(&a, described), ["TIME,2"]);
    assert_eq!(code(&a, "SELECT D FROM T UNION SELECT S FROM T"), Code::SyntaxRule, "§9.3 SR3e");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.8 GR1-GR3, SR1; §4.5.1]
/// CURRENT_DATE, CURRENT_TIME and CURRENT_TIMESTAMP are the clock's
/// instant, of a type WITH TIME ZONE, in the session's displacement,
/// TIME_ZONE, of the precision written; a time or timestamp without time
/// zone is kept in UTC, so it shows in the session's displacement [§6.1
/// SR5]; TIME_ZONE takes a displacement from -12:59 to +13:00.
#[test]
fn current_datetimes_6_8() {
    let db = datetimes();
    let a = db.session("A");
    a.run("INSERT INTO T (D, S) VALUES (CURRENT_DATE, TIMESTAMP '2024-01-01 10:00:00')").unwrap();
    assert_eq!(
        rows(&a, "SELECT CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP(3), CURRENT_TIMESTAMP FROM T").unwrap(),
        ["2024-02-29,12:34:56+00:00,2024-02-29 12:34:56.789+00:00,2024-02-29 12:34:56.789012+00:00"]
    );
    a.run("SET TIME_ZONE = '-12:59'").unwrap();
    assert_eq!(
        rows(&a, "SELECT CURRENT_DATE, CURRENT_TIME(1), S FROM T").unwrap(),
        ["2024-02-28,23:35:56.7-12:59,2023-12-31 21:01:00.000"]
    );
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE S = TIMESTAMP '2023-12-31 21:01:00'").unwrap(), ["1"], "a literal in it too");
    assert_eq!(exec(&a, "SET TIME_ZONE = '+13:01'").err(), Some(Code::Data));
    assert_eq!(code(&a, "SELECT CURRENT_TIME(7) FROM T"), Code::Unsupported);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 SR4, GR5e, GR6e,
/// GR9-GR11] CAST takes a character string, its spaces trimmed, to the
/// datetime it writes (SQLSTATE 22018 if none); a timestamp to its date
/// or time; a date or a time to a timestamp, at its midnight or on the
/// current date; a datetime to its literal's string (SQLSTATE 22001 if
/// longer than the string); no number to or from one, nor a date to a time
/// (SQLCODE -103).
#[test]
fn datetimes_are_cast_6_10() {
    let db = datetimes();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-01-02')").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(' 2024-01-02 ' AS DATE), CAST('10:11:12.5' AS TIME(1)), CAST('2024-01-02 03:04:05+01:00' AS TIMESTAMP(0) WITH TIME ZONE) FROM T").unwrap(), [
        "2024-01-02,10:11:12.5,2024-01-02 03:04:05+01:00"
    ]);
    assert_eq!(rows(&a, "SELECT CAST(TIMESTAMP '2024-01-02 03:04:05' AS DATE), CAST(TIMESTAMP '2024-01-02 03:04:05' AS TIME), CAST(D AS TIMESTAMP(0)), CAST(TIME '01:02:03' AS TIMESTAMP(0)) FROM T").unwrap(), [
        "2024-01-02,03:04:05,2024-01-02 00:00:00,2024-02-29 01:02:03"
    ]);
    assert_eq!(
        rows(&a, "SELECT CAST(D AS CHAR(12)), CAST(D AS VARCHAR(20)), CAST(TIMESTAMP '2024-01-02 03:04:05+05:00' AS TIMESTAMP(0)) FROM T")
            .unwrap(),
        ["2024-01-02  ,2024-01-02,2024-01-01 22:04:05"]
    );
    assert_eq!(code(&a, "SELECT CAST(D AS CHAR(5)) FROM T"), Code::Data, "22001");
    assert_eq!(code(&a, "SELECT CAST('2024-13-01' AS DATE) FROM T"), Code::Data, "22018");
    for bad in ["CAST(D AS TIME)", "CAST(TIME '01:00:00' AS DATE)", "CAST(1 AS DATE)", "CAST(D AS INTEGER)", "CAST(D AS BIT(8))"] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.6 SR3, SR4, GR3] EXTRACT
/// gives a field of a datetime as it shows, SECOND of its scale the
/// datetime's precision, TIMEZONE_HOUR and TIMEZONE_MINUTE of a datetime
/// WITH TIME ZONE its displacement's, of its sign; a field it has not is
/// refused (SQLCODE -103); of null, null.
#[test]
fn extract_6_6() {
    let db = datetimes();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (DATE '2024-03-04', NULL, TIMESTAMP '2024-05-06 07:08:09.125', TIMESTAMP '2024-01-01 00:00:00-05:30')")
        .unwrap();
    assert_eq!(rows(&a, "SELECT EXTRACT(YEAR FROM D), EXTRACT(MONTH FROM D), EXTRACT(DAY FROM D), EXTRACT(HOUR FROM S), EXTRACT(MINUTE FROM S), EXTRACT(SECOND FROM S) FROM T").unwrap(), [
        "2024,3,4,7,8,9.125"
    ]);
    assert_eq!(
        rows(
            &a,
            "SELECT EXTRACT(TIMEZONE_HOUR FROM Z), EXTRACT(TIMEZONE_MINUTE FROM Z), EXTRACT(HOUR FROM Z), EXTRACT(SECOND FROM H) FROM T"
        )
        .unwrap(),
        ["-5,-30,0,NULL"]
    );
    a.run("SET TIME_ZONE = '+02:00'").unwrap();
    assert_eq!(rows(&a, "SELECT EXTRACT(HOUR FROM S), EXTRACT(DAY FROM CURRENT_TIMESTAMP) FROM T").unwrap(), ["9,29"], "as shown");
    for bad in ["EXTRACT(HOUR FROM D)", "EXTRACT(YEAR FROM H)", "EXTRACT(TIMEZONE_HOUR FROM S)", "EXTRACT(DAY FROM 1)"] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5 SR3a v, SR3c, GR1b vi,
/// GR1d] a datetime column's default is a datetime literal of its fields,
/// or a datetime value function of them, taken as the statement that
/// takes it runs; one of other fields is refused (SQLCODE -103).
#[test]
fn datetime_defaults_11_5() {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (N INTEGER, D DATE DEFAULT CURRENT_DATE, \
         S TIMESTAMP(0) DEFAULT CURRENT_TIMESTAMP(0), L DATE DEFAULT DATE '2000-01-01', H TIME DEFAULT CURRENT_TIME)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (N) VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT D, S, L, H FROM T").unwrap(), ["2024-02-29,2024-02-29 12:34:56,2000-01-01,12:34:56"]);
    let described = "SELECT COLUMN_NAME, COLUMN_DEFAULT FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME IN ('D', 'S') ORDER BY 1";
    assert_eq!(squeezed(&a, described), ["D,CURRENT_DATE", "S,CURRENT_TIMESTAMP(0)"]);
    assert_eq!(exec(&a, "CREATE TABLE U (H TIME DEFAULT CURRENT_DATE)").err(), Some(Code::SyntaxRule), "SR3c");
    assert_eq!(exec(&a, "CREATE TABLE U (S TIMESTAMP DEFAULT DATE '2000-01-01')").err(), Some(Code::SyntaxRule), "SR3a v");
    assert_eq!(exec(&a, "CREATE TABLE U (S TIMESTAMP DEFAULT '2000-01-01')").err(), Some(Code::SyntaxRule), "SR3a v");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.12, §6.14, §6.15; §6.5]
/// arithmetic on a datetime is with an interval: a datetime with a number,
/// a datetime negated, and a difference of datetimes with no interval
/// qualifier are refused (SQLCODE -103), and SUM and AVG take no datetime.
#[test]
fn datetimes_are_no_numbers_6_14() {
    let db = datetimes();
    let a = db.session("A");
    for bad in ["D + 1", "-S", "S - S", "SUM(D)", "AVG(H)"] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.7] RULES.md D25: datetimes
/// of every type, a unique index on a timestamp with time zone, and a
/// default of CURRENT_DATE are kept: after a crash and after a clean
/// close; one instant in two displacements is one value of the index.
#[test]
fn datetimes_are_kept() {
    let scratch = Scratch::new("datetimes");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (D DATE DEFAULT CURRENT_DATE, H TIME(3) WITH TIME ZONE, Z TIMESTAMP WITH TIME ZONE UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (H, Z) VALUES (TIME '01:02:03.456+04:00', TIMESTAMP '2024-01-01 05:30:00+05:30')").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT D, H, Z FROM T"), ["2024-02-29,01:02:03.456+04:00,2024-01-01 05:30:00.000000+05:30"]);
        assert_eq!(
            exec(&a, "INSERT INTO T (Z) VALUES (TIMESTAMP '2024-01-01 00:00:00+00:00')").err(),
            Some(Code::Constraint),
            "one instant"
        );
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Schema A, defined in `dir`, by a session of displacement +02:00:
/// table T, its column S's default written without a displacement, its
/// column U's added by ALTER TABLE, its column V of domain W's; and schema
/// Z, by the operator, the database's TIME_ZONE +03:00, its table R's
/// column S's default so too.
fn defaults_in(dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for sql in [
        "SET TIME ZONE INTERVAL '02:00' HOUR TO MINUTE",
        "CREATE TABLE T (N INTEGER, S TIMESTAMP WITH TIME ZONE DEFAULT TIMESTAMP '2026-01-01 12:00:00')",
        "ALTER TABLE T ADD COLUMN U TIMESTAMP WITH TIME ZONE DEFAULT TIMESTAMP '2026-01-01 13:00:00'",
        "CREATE DOMAIN W AS TIMESTAMP WITH TIME ZONE DEFAULT TIMESTAMP '2026-01-01 14:00:00'",
        "ALTER TABLE T ADD COLUMN V W",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.commit().unwrap();
    db.set_setting("TIME_ZONE", &turborel::value::Value::Char("+03:00".into())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Z CREATE TABLE R (S TIMESTAMP WITH TIME ZONE DEFAULT TIMESTAMP '2026-01-01 12:00:00') \
         GRANT INSERT, SELECT ON R TO A",
    )
    .unwrap();
    db
}

/// That the defaults of `defaults_in` are of the displacements they were
/// defined in, read by a session of another.
fn defaults_kept(db: &Database) {
    let a = db.session("A");
    a.run("SET TIME ZONE INTERVAL -'05:00' HOUR TO MINUTE").unwrap();
    a.run("INSERT INTO T (N) VALUES (1)").unwrap();
    a.run("INSERT INTO Z.R VALUES (DEFAULT)").unwrap_or_else(|e| panic!("{e}"));
    let utc = |sql: &str| rows(&a, sql).unwrap();
    let at = "SELECT COUNT(*) FROM T WHERE S = TIMESTAMP '2026-01-01 10:00:00+00:00' AND U = TIMESTAMP '2026-01-01 11:00:00+00:00' \
              AND V = TIMESTAMP '2026-01-01 12:00:00+00:00'";
    assert_eq!(utc(at), ["1"], "of +02:00");
    assert_eq!(utc("SELECT COUNT(*) FROM Z.R WHERE S = TIMESTAMP '2026-01-01 09:00:00+00:00'"), ["1"], "of +03:00");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR18, §11.5 SR3a v]: a
/// default's datetime literal written without a displacement is of the
/// defining session's, or, a schema defined by the operator, of the
/// database's TIME_ZONE; so it stays after recovery and a checkpoint,
/// whatever the session reading it.
#[test]
fn a_default_takes_the_defining_displacement_5_3() {
    let scratch = Scratch::new("datetime-defaults");
    let dir = scratch.path("db");
    let db = defaults_in(&dir);
    defaults_kept(&db);
    drop(db);
    let recovered = Database::new();
    recovered.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    defaults_kept(&recovered);
    recovered.close().unwrap();
    drop(recovered);
    let reopened = Database::new();
    reopened.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    defaults_kept(&reopened);
}
