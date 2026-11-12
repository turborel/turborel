//! CAST with FORMAT [ISO/IEC 9075-2:2023 §6.13, §9.50-§9.52; Feature
//! T839]: a datetime to a character string as a datetime template writes
//! it, and a character string to a datetime as one reads it.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, rows};

/// 2024-02-29 12:34:56.789012 UTC, in microseconds from 1970-01-01.
const NOW: i64 = 1_709_210_096_789_012;

/// A database whose clock stands at `NOW`, of schema A with table T of a
/// DATE D, a TIMESTAMP(3) S, a TIMESTAMP WITH TIME ZONE Z and a VARCHAR(30)
/// V.
fn formatted() -> Database {
    let db = Database::new();
    db.set_clock(Arc::new(FixedClock::at(NOW))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (D DATE, S TIMESTAMP(3), Z TIMESTAMP WITH TIME ZONE, V VARCHAR(30))").unwrap();
    db
}

/// The SQLSTATE `sql` fails with, or none if it is taken.
fn state(s: &Session, sql: &str) -> Option<String> {
    s.open_cursor(sql).and_then(|mut c| c.fetch()).err().map(|e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR5e, GR6e; §9.50
/// GR4, GR5] a datetime is written field by field as the template says:
/// the year's last digits, a 12-hour clock and P.M., the day of the year,
/// the second of the day, the fraction truncated to the template's digits,
/// the displacement signed; a value without time zone in the session's
/// displacement; padded to a fixed length, and null of null.
#[test]
fn datetimes_are_formatted_9_50() {
    let db = formatted();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (DATE '2024-03-05', TIMESTAMP '2024-03-05 14:07:09.987', TIMESTAMP '2024-03-05 14:07:09-05:30', 'x')")
        .unwrap();
    let q = "SELECT CAST(D AS VARCHAR(10) FORMAT 'DD/MM/YYYY'), CAST(S AS VARCHAR(30) FORMAT 'YY-MM-DD HH12:MI:SS.FF2 P.M.'), \
             CAST(D AS VARCHAR(8) FORMAT 'YYYY DDD'), CAST(S AS VARCHAR(9) FORMAT 'SSSSS.FF1') FROM T";
    assert_eq!(rows(&a, q).unwrap(), ["05/03/2024,24-03-05 02:07:09.98 P.M.,2024 065,50829.9"]);
    let q = "SELECT CAST(Z AS VARCHAR(20) FORMAT 'HH24:MI TZH:TZM'), CAST(D AS CHAR(12) FORMAT 'DD.MM.YYYY') || '|' FROM T";
    assert_eq!(rows(&a, q).unwrap(), ["14:07 -05:30,05.03.2024  |"]);
    a.run("SET SESSION.TIME_ZONE = '+02:00'").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(S AS VARCHAR(5) FORMAT 'HH24') FROM T").unwrap(), ["16"], "shown in +02:00");
    assert_eq!(rows(&a, "SELECT CAST(CAST(NULL AS DATE) AS VARCHAR(10) FORMAT 'YYYY') FROM T").unwrap(), ["NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR14-GR18; §9.51
/// GR4-GR12] a string is read as the template says: delimited fields of
/// fewer digits, a short year after the current year's first digits, the
/// current month and day 1 when unwritten, a day of the year, a 12-hour
/// clock, a displacement with a space for its sign, 0 if none is written;
/// a value without time zone in the session's displacement; null of null.
#[test]
fn strings_are_read_9_51() {
    let db = formatted();
    let a = db.session("A");
    let q = "VALUES (CAST('5/3/2024' AS DATE FORMAT 'DD/MM/YYYY'), CAST('24' AS DATE FORMAT 'YY'), CAST('15' AS DATE FORMAT 'DD'), \
             CAST('2024 366' AS DATE FORMAT 'YYYY DDD'), CAST('1999' AS DATE FORMAT 'YYYY'))";
    assert_eq!(rows(&a, q).unwrap(), ["2024-03-05,2024-02-01,2024-02-15,2024-12-31,1999-02-01"]);
    let q = "VALUES (CAST('12:05 A.M.' AS TIME FORMAT 'HH12:MI A.M.'), CAST('3661.5' AS TIME(1) FORMAT 'SSSSS.FF1'), \
             CAST('20240102 1530  5:30' AS TIMESTAMP(0) WITH TIME ZONE FORMAT 'YYYYMMDD HH24MI TZH:TZM'), \
             CAST('2024-01-02' AS TIMESTAMP(0) WITH TIME ZONE FORMAT 'YYYY-MM-DD'))";
    assert_eq!(rows(&a, q).unwrap(), ["00:05:00,01:01:01.5,2024-01-02 15:30:00+05:30,2024-01-02 00:00:00+00:00"]);
    let q = "VALUES CAST('2024-01-02 03:04:05.123456789' AS TIMESTAMP(3) FORMAT 'YYYY-MM-DD HH24:MI:SS.FF9')";
    assert_eq!(rows(&a, q).unwrap(), ["2024-01-02 03:04:05.123"], "truncated to the type's precision");
    a.run("SET SESSION.TIME_ZONE = '+02:00'").unwrap();
    let q = "VALUES CAST(CAST('2024-01-02 03:00' AS TIMESTAMP(0) FORMAT 'YYYY-MM-DD HH24:MI') AS TIMESTAMP(0) WITH TIME ZONE)";
    assert_eq!(rows(&a, q).unwrap(), ["2024-01-02 03:00:00+02:00"]);
    assert_eq!(rows(&a, "VALUES CAST(CAST(NULL AS VARCHAR(4)) AS DATE FORMAT 'YYYY')").unwrap(), ["NULL"]);
    a.run("INSERT INTO T (V) VALUES ('2024-02-29')").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(V AS DATE FORMAT 'YYYY-MM-DD') FROM T").unwrap(), ["2024-02-29"], "of a column");
    a.run("CREATE VIEW W (X) AS SELECT CAST(V AS DATE FORMAT 'YYYY-MM-DD') FROM T").unwrap();
    assert_eq!(rows(&a, "SELECT X + INTERVAL '1' DAY FROM W").unwrap(), ["2024-03-01"], "of a view");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.51 GR4, GR12; §6.13 GR5e
/// ii 4] a string the template does not match, untrimmed, or whose fields
/// are no datetime, is refused with SQLSTATE 22007; a formatted datetime
/// longer than its string type with 22001.
#[test]
fn strings_refused_9_51() {
    let db = formatted();
    let a = db.session("A");
    let refused = [
        "CAST('2024-1-2' AS DATE FORMAT 'YYYYMMDD')",
        "CAST(' 2024-01-02' AS DATE FORMAT 'YYYY-MM-DD')",
        "CAST('2023-02-29' AS DATE FORMAT 'YYYY-MM-DD')",
        "CAST('2024-13-01' AS DATE FORMAT 'YYYY-MM-DD')",
        "CAST('2023 366' AS DATE FORMAT 'YYYY DDD')",
        "CAST('13 P.M.' AS TIME FORMAT 'HH12 P.M.')",
        "CAST('24:00' AS TIME FORMAT 'HH24:MI')",
        "CAST('86400' AS TIME FORMAT 'SSSSS')",
        "CAST('2024 +13:01' AS TIMESTAMP WITH TIME ZONE FORMAT 'YYYY TZH:TZM')",
        "CAST('2024-01-٠٢' AS DATE FORMAT 'YYYY-MM-DD')",
        "CAST(CAST('2024-01-02' AS CHAR(12)) AS DATE FORMAT 'YYYY-MM-DD')",
    ];
    for sql in refused {
        assert_eq!(state(&a, &format!("VALUES {sql}")).as_deref(), Some("22007"), "{sql}");
    }
    assert_eq!(state(&a, "VALUES CAST(DATE '2024-03-05' AS VARCHAR(9) FORMAT 'DD.MM.YYYY')").as_deref(), Some("22001"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR4; §9.50 SR3; §9.51
/// SR3; §9.52 SR2-SR12] a FORMAT is only of a datetime to a character
/// string or back, so not of NULL nor a `?`; of a string type no shorter
/// than the template's minimum length; of a template whose fields are
/// written in upper case, go together, and are of the datetime type: each
/// refused as the statement is bound, SQLCODE -103; a template that is no
/// `<character string literal>` is a syntax error.
#[test]
fn formats_refused_6_13() {
    let db = formatted();
    let a = db.session("A");
    let refused = [
        "VALUES CAST(1 AS VARCHAR(10) FORMAT 'YYYY')",
        "VALUES CAST(DATE '2024-01-01' AS TIMESTAMP FORMAT 'YYYY')",
        "VALUES CAST(NULL AS DATE FORMAT 'YYYY')",
        "VALUES CAST(DATE '2024-01-01' AS VARCHAR(7) FORMAT 'YYYYMMDD')",
        "VALUES CAST('2024' AS DATE FORMAT 'yyyy')",
        "VALUES CAST('2024' AS DATE FORMAT 'YYYY--MM')",
        "VALUES CAST('2024' AS DATE FORMAT 'YYYY RR')",
        "VALUES CAST('10' AS TIME FORMAT 'HH12')",
        "VALUES CAST('10' AS TIME FORMAT 'HH24 TZH')",
        "VALUES CAST(DATE '2024-01-01' AS VARCHAR(10) FORMAT 'HH24')",
    ];
    for sql in refused {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(a.prepare("VALUES CAST(? AS DATE FORMAT 'YYYY')").err().map(|e| e.code), Some(Code::SyntaxRule), "a ? takes DATE");
    assert_eq!(code(&a, "VALUES CAST('2024' AS DATE FORMAT 1)"), Code::Syntax);
    assert!(a.prepare("VALUES CAST(CAST(? AS VARCHAR(4)) AS DATE FORMAT 'YYYY')").is_ok(), "a ? of a string type");
}
