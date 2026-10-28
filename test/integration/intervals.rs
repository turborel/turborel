//! Intervals [ISO/IEC 9075:1992 §4.5.2, §6.1, §10.1]: year-month and
//! day-time, their literals, arithmetic with datetimes and numbers, the
//! difference of datetimes, CAST, EXTRACT, `<time zone>`, SET TIME ZONE
//! and OVERLAPS.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, sorted};

/// 2024-02-29 12:34:56.789012 UTC, in microseconds from 1970-01-01.
const NOW: i64 = 1_709_210_096_789_012;

/// A database whose clock stands at `NOW`, of schema A with table T of an
/// INTERVAL YEAR TO MONTH M, an INTERVAL DAY TO SECOND(2) S, and a DATE D.
fn intervals() -> Database {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (M INTERVAL YEAR TO MONTH, S INTERVAL DAY TO SECOND(2), D DATE)").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// The first row of `SELECT values FROM T`, T holding one row.
fn one(a: &Session, values: &str) -> String {
    rows(a, &format!("SELECT {values} FROM T")).unwrap().join("|")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1, §10.1 SR2-SR6; §21.3.5]
/// INTERVAL of a qualifier, its start more significant than its end, not
/// MONTH, YEAR only TO MONTH; a leading precision of 2 and a fraction's of
/// 6 if none is written; one past this implementation's 9 and 6 is valid
/// but not processed (SQLCODE -900, RULES.md D18).
#[test]
fn interval_types_10_1() {
    let db = intervals();
    let a = db.session("A");
    a.run("CREATE TABLE U (A INTERVAL YEAR(4) TO MONTH, B INTERVAL DAY(3) TO SECOND(2), C INTERVAL SECOND(4, 1), E INTERVAL HOUR)")
        .unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, DATETIME_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U' ORDER BY 1";
    assert_eq!(squeezed(&a, described), ["A,INTERVAL,0", "B,INTERVAL,2", "C,INTERVAL,1", "E,INTERVAL,0"]);
    for bad in ["MONTH TO YEAR", "MONTH TO DAY", "YEAR TO DAY", "HOUR TO DAY", "DAY(0)"] {
        assert_eq!(exec(&a, &format!("CREATE TABLE W (A INTERVAL {bad})")).err(), Some(Code::SyntaxRule), "{bad}");
    }
    for big in ["DAY(10)", "SECOND(2, 7)", "HOUR TO SECOND(9)"] {
        assert_eq!(exec(&a, &format!("CREATE TABLE W (A INTERVAL {big})")).err(), Some(Code::Unsupported), "{big}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR20, SR23, GR5; §4.5.2
/// Table 7; §10.1 SR5] an interval literal is INTERVAL, a sign, a string
/// of the qualifier's fields, each but the leading in its range, the
/// leading of no more digits than its precision; one otherwise is refused
/// (SQLCODE -103); it is shown as written.
#[test]
fn interval_literals_5_3() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-01-01')").unwrap();
    assert_eq!(
        one(
            &a,
            "INTERVAL '1-2' YEAR TO MONTH, INTERVAL -'3 04:05:06.5' DAY TO SECOND, INTERVAL +'90' MINUTE, INTERVAL '1.25' SECOND(2, 2)"
        ),
        "1-02,-3 04:05:06.500000,90,1.25"
    );
    for bad in ["'100' DAY", "'1 24' DAY TO HOUR", "'1-12' YEAR TO MONTH", "'1:60' MINUTE TO SECOND", "'1 2:3' DAY TO HOUR", "'x' DAY"] {
        assert_eq!(code(&a, &format!("SELECT INTERVAL {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    assert_eq!(one(&a, "INTERVAL '100' DAY(3)"), "100");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.14 SR3, GR3; §6.15 SR3, SR4,
/// GR1-GR5] a datetime plus or minus an interval of its fields, its year
/// and month moved and its day kept (SQLSTATE 22008 if no date), a time
/// modulo 24 hours; intervals of one class added, of the fields of both;
/// one times or divided by a number (SQLSTATE 22012 by 0); any other
/// operation is refused (SQLCODE -103).
#[test]
fn interval_arithmetic_6_14() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (INTERVAL '1-6' YEAR TO MONTH, INTERVAL '1 12:00:00' DAY TO SECOND, DATE '2024-01-31')").unwrap();
    assert_eq!(
        one(&a, "D + INTERVAL '1' DAY, D - INTERVAL '1-1' YEAR TO MONTH, INTERVAL '29' DAY + D, D + M"),
        "2024-02-01,2022-12-31,2024-02-29,2025-07-31"
    );
    assert_eq!(code(&a, "SELECT D + INTERVAL '1' MONTH FROM T"), Code::Data, "22008: 2024-02-31");
    let said = a.open_cursor("SELECT D + INTERVAL '1' MONTH FROM T").err().map(|e| e.msg).unwrap_or_default();
    assert!(said.contains("+ INTERVAL '1' MONTH(2) is no valid datetime"), "the operation named: {said}");
    assert_eq!(code(&a, "SELECT D + INTERVAL '9' YEAR(4) * 1000 FROM T"), Code::Data, "22008: year 11024");
    assert_eq!(
        one(&a, "TIMESTAMP '2024-02-29 23:00:00' + S, TIME '23:30:00' + INTERVAL '45' MINUTE, TIME '00:10:00' - INTERVAL '20' MINUTE"),
        "2024-03-02 11:00:00,00:15:00,23:50:00"
    );
    assert_eq!(
        one(&a, "M + INTERVAL '6' MONTH, S - INTERVAL '36' HOUR, M * 2, 3 * S, S / 4, -M, M * 0.5"),
        "2-00,0 00:00:00.00,3-00,4 12:00:00.00,0 09:00:00.00,-1-06,0-09"
    );
    assert_eq!(code(&a, "SELECT S / 0 FROM T"), Code::Data, "22012");
    for bad in
        ["M + S", "D + INTERVAL '1' HOUR", "TIME '01:00:00' + INTERVAL '1' DAY", "D - D", "INTERVAL '1' DAY - D", "M * M", "2 / M", "M + 1"]
    {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.15 SR5, GR6] (A - B) and an
/// interval qualifier: the difference of two datetimes of the same fields,
/// of years and months between their years and months, else of their
/// instants, truncated to its least field (SQLSTATE 22015 if its leading
/// field cannot hold it).
#[test]
fn datetime_differences_6_15() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2023-01-15')").unwrap();
    assert_eq!(
        one(&a, "(TIMESTAMP '2024-03-01 00:00:00' - TIMESTAMP '2024-02-28 12:30:00') DAY TO HOUR, (DATE '2024-05-01' - D) YEAR TO MONTH"),
        "1 11,1-04"
    );
    assert_eq!(one(&a, "(D - DATE '2023-01-01') DAY, (TIME '01:00:00' - TIME '03:30:00') MINUTE(3), (D - D) SECOND"), "14,-150,0.000000");
    assert_eq!(code(&a, "SELECT (D - DATE '2020-01-01') DAY FROM T"), Code::Data, "22015: 1110 days in DAY(2)");
    assert_eq!(code(&a, "SELECT (TIME '01:00:00' - D) DAY FROM T"), Code::SyntaxRule, "not of the same fields");
    assert_eq!(code(&a, "SELECT (TIME '01:00:00' - TIME '00:00:00') YEAR FROM T"), Code::SyntaxRule, "a time has no years");
    assert_eq!(code(&a, "SELECT (D + D) DAY FROM T"), Code::Syntax, "a difference");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR6, §9.2 SR2, GR3m, §6.5
/// SR9e, §9.3 SR3f] intervals compare by their units, with intervals of
/// their class alone; assigned to a column of their class, in its fields
/// (SQLSTATE 22015 if they leave it); SUM and AVG of them are intervals;
/// a union is of the fields of both.
#[test]
fn intervals_compare_and_are_assigned_8_2() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (M, S) VALUES (INTERVAL '2' YEAR, INTERVAL '36' HOUR), (INTERVAL '23' MONTH, INTERVAL '1 12' DAY TO HOUR), (INTERVAL '0' MONTH, INTERVAL '90' SECOND)").unwrap();
    assert_eq!(rows(&a, "SELECT M FROM T ORDER BY M").unwrap(), ["0-00", "1-11", "2-00"]);
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT S) FROM T").unwrap(), ["2"], "36 hours is 1 day 12 hours");
    assert_eq!(one(&a, "SUM(M), AVG(S), MAX(S), MIN(M)").replace('|', ","), "3-11,1 00:00:30.00,1 12:00:00.00,0-00");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE S > INTERVAL '1' DAY AND M < INTERVAL '2' YEAR").unwrap(), ["1"]);
    assert_eq!(code(&a, "SELECT M FROM T WHERE M = S"), Code::SyntaxRule, "two classes");
    assert_eq!(code(&a, "SELECT M FROM T WHERE M = 1"), Code::SyntaxRule, "a number");
    assert_eq!(exec(&a, "INSERT INTO T (M) VALUES (INTERVAL '100' YEAR(3))").err(), Some(Code::Data), "22015: YEAR(2)");
    assert_eq!(exec(&a, "INSERT INTO T (M) VALUES (INTERVAL '1' DAY)").err(), Some(Code::SyntaxRule), "a day-time interval");
    a.run("CREATE VIEW V (X) AS SELECT INTERVAL '1' DAY FROM T UNION SELECT INTERVAL '1:02' HOUR TO MINUTE FROM T").unwrap();
    assert_eq!(rows(&a, "SELECT X FROM V ORDER BY X").unwrap(), ["0 01:02", "1 00:00"]);
    assert_eq!(code(&a, "SELECT M FROM T UNION SELECT S FROM T"), Code::SyntaxRule, "§9.3 SR3f");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 SR4-SR6, GR3c, GR5e,
/// GR12] CAST of an interval to a string is its literal's string; of a
/// string to an interval, the one it writes (SQLSTATE 22018 if none); of
/// an exact number to and from an interval of one field; of an interval
/// to another of its class, in its fields (SQLSTATE 22015 if they cannot
/// hold it); no other (SQLCODE -103).
#[test]
fn intervals_are_cast_6_10() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-01-01')").unwrap();
    assert_eq!(
        one(
            &a,
            "CAST(INTERVAL '1-2' YEAR TO MONTH AS CHAR(6)), CAST(INTERVAL -'3' DAY AS VARCHAR(5)), CAST(' 3 04 ' AS INTERVAL DAY TO HOUR)"
        ),
        "1-02  ,-3,3 04"
    );
    assert_eq!(
        one(
            &a,
            "CAST(90 AS INTERVAL MINUTE(3)), CAST(1.5 AS INTERVAL SECOND), CAST(INTERVAL '2' HOUR AS INTEGER), CAST(INTERVAL '2.5' SECOND AS NUMERIC(3, 1))"
        ),
        "90,1.500000,2,2.5"
    );
    assert_eq!(
        one(&a, "CAST(INTERVAL '1 02' DAY TO HOUR AS INTERVAL HOUR(3)), CAST(INTERVAL '90' MINUTE AS INTERVAL HOUR TO MINUTE)"),
        "26,1:30"
    );
    assert_eq!(code(&a, "SELECT CAST(INTERVAL '100' HOUR(3) AS INTERVAL HOUR) FROM T"), Code::Data, "22015");
    assert_eq!(code(&a, "SELECT CAST(100 AS INTERVAL HOUR) FROM T"), Code::Data, "22015");
    assert_eq!(code(&a, "SELECT CAST('x' AS INTERVAL DAY) FROM T"), Code::Data, "22018");
    for bad in [
        "CAST(INTERVAL '1' YEAR AS INTERVAL DAY)",
        "CAST(1 AS INTERVAL DAY TO HOUR)",
        "CAST(INTERVAL '1' DAY TO HOUR AS INTEGER)",
        "CAST(D AS INTERVAL DAY)",
        "CAST(1E0 AS INTERVAL DAY)",
    ] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.6 SR3, SR4, GR3a i] EXTRACT
/// of a field of an interval, of the interval's sign, SECOND of the scale
/// of its fraction; of a field it has not, or of a time zone's, refused
/// (SQLCODE -103), the field named by its key word.
#[test]
fn extract_of_intervals_6_6() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (M, S) VALUES (INTERVAL -'2-3' YEAR TO MONTH, INTERVAL '1 05:30:07.25' DAY TO SECOND)").unwrap();
    assert_eq!(
        one(
            &a,
            "EXTRACT(YEAR FROM M), EXTRACT(MONTH FROM M), EXTRACT(DAY FROM S), EXTRACT(HOUR FROM S), EXTRACT(MINUTE FROM S), EXTRACT(SECOND FROM S)"
        ),
        "-2,-3,1,5,30,7.25"
    );
    for bad in ["EXTRACT(DAY FROM M)", "EXTRACT(YEAR FROM S)", "EXTRACT(TIMEZONE_HOUR FROM S)"] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    let said = |sql: &str| a.open_cursor(sql).err().map(|e| e.msg).unwrap_or_default();
    assert!(said("SELECT EXTRACT(DAY FROM M) FROM T").contains("has no field DAY"), "the field's key word");
    assert!(said("SELECT EXTRACT(TIMEZONE_HOUR FROM TIME '10:00:00') FROM T").contains("has no field TIMEZONE_HOUR"));
    assert!(said("SELECT M + S FROM T").contains(" + INTERVAL"), "the operator's symbol");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.14 SR4, SR5, GR2, GR4; §16.5]
/// AT TIME ZONE an INTERVAL HOUR TO MINUTE gives the instant WITH TIME
/// ZONE in that displacement (SQLSTATE 22009 out of -12:59 to +13:00), AT
/// LOCAL in the session's; SET TIME ZONE sets the session's, LOCAL the
/// database's again; a date has no time zone, nor another interval is one
/// (SQLCODE -103).
#[test]
fn time_zones_6_14() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-01-01')").unwrap();
    assert_eq!(
        one(
            &a,
            "TIMESTAMP '2024-01-01 12:00:00+00:00' AT TIME ZONE INTERVAL '05:30' HOUR TO MINUTE, TIME '12:00:00' AT TIME ZONE INTERVAL -'01:00' HOUR TO MINUTE"
        ),
        "2024-01-01 17:30:00+05:30,11:00:00-01:00"
    );
    a.run("SET TIME ZONE INTERVAL '05:30' HOUR TO MINUTE").unwrap();
    assert_eq!(
        one(&a, "CURRENT_TIME, TIMESTAMP '2024-01-01 12:00:00+00:00' AT LOCAL, EXTRACT(TIMEZONE_MINUTE FROM CURRENT_TIMESTAMP)"),
        "18:04:56+05:30,2024-01-01 17:30:00+05:30,30"
    );
    a.run("SET TIME ZONE LOCAL").unwrap();
    assert_eq!(one(&a, "CURRENT_TIME"), "12:34:56+00:00");
    assert_eq!(code(&a, "SELECT TIME '12:00:00' AT TIME ZONE INTERVAL '14:00' HOUR TO MINUTE FROM T"), Code::Data, "22009");
    assert_eq!(exec(&a, "SET TIME ZONE INTERVAL -'13:00' HOUR TO MINUTE").err(), Some(Code::Data), "22009");
    assert_eq!(exec(&a, "SET TIME ZONE INTERVAL '1' DAY").err(), Some(Code::SyntaxRule), "SR1");
    for bad in ["D AT LOCAL", "TIME '12:00:00' AT TIME ZONE INTERVAL '1' HOUR"] {
        assert_eq!(code(&a, &format!("SELECT {bad} FROM T")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.11] OVERLAPS of two periods,
/// each a datetime and another or an interval: true if they share an
/// instant, a period of no length at the start of the other too; false if
/// one ends where the other begins; unknown with a null; rows not of two,
/// or of datetimes not comparable, refused (SQLCODE -103).
#[test]
fn overlaps_8_11() {
    let db = intervals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (DATE '2024-01-05')").unwrap();
    let count = |cond: &str| rows(&a, &format!("SELECT COUNT(*) FROM T WHERE {cond}")).unwrap().join("");
    assert_eq!(count("(DATE '2024-01-01', DATE '2024-01-10') OVERLAPS (D, INTERVAL '10' DAY)"), "1");
    assert_eq!(count("(DATE '2024-01-10', DATE '2024-01-01') OVERLAPS (D, D)"), "1", "reversed; of no length");
    assert_eq!(count("(DATE '2024-01-01', D) OVERLAPS (D, DATE '2024-01-09')"), "0", "touching");
    assert_eq!(count("(D, D) OVERLAPS (D, DATE '2024-01-09')"), "1", "the same start");
    assert_eq!(count("(DATE '2024-01-01', CAST(NULL AS DATE)) OVERLAPS (D, D)"), "0", "unknown");
    assert_eq!(count("NOT ((DATE '2024-01-01', DATE '2024-01-02') OVERLAPS (D, D))"), "1", "false, the later start");
    assert_eq!(count("NOT ((DATE '2024-01-01', CAST(NULL AS DATE)) OVERLAPS (DATE '2024-02-01', D))"), "0", "unknown");
    for bad in [
        "(D) OVERLAPS (D, D)",
        "(D, D) OVERLAPS (TIME '01:00:00', TIME '02:00:00')",
        "(D, 1) OVERLAPS (D, D)",
        "(D, INTERVAL '1' HOUR) OVERLAPS (D, D)",
        "(1, 2) OVERLAPS (D, D)",
    ] {
        assert_eq!(code(&a, &format!("SELECT D FROM T WHERE {bad}")), Code::SyntaxRule, "{bad}");
    }
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5 SR3a vi; §11.7] RULES.md
/// D25: intervals of both classes, a default of an interval literal and a
/// unique index on one are kept: after a crash and after a clean close;
/// 36 hours and 1 day 12 hours are one value of the index.
#[test]
fn intervals_are_kept() {
    let scratch = Scratch::new("intervals");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (M INTERVAL YEAR(3) TO MONTH DEFAULT INTERVAL '1-1' YEAR TO MONTH, S INTERVAL HOUR(3) TO SECOND(1) UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (S) VALUES (INTERVAL '36:00:00.5' HOUR(2) TO SECOND), (INTERVAL -'1' HOUR)").unwrap();
    assert_eq!(exec(&a, "CREATE TABLE U (M INTERVAL YEAR DEFAULT INTERVAL '1' DAY)").err(), Some(Code::SyntaxRule), "SR3a vi");
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT M, S FROM T"), ["1-01,-1:00:00.0", "1-01,36:00:00.5"]);
        assert_eq!(exec(&a, "INSERT INTO T (S) VALUES (INTERVAL '1 12:00:00.5' DAY TO SECOND)").err(), Some(Code::Constraint), "one value");
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}
