//! \[DESIGN\] Expressions evaluated a vector at a time (RULES.md D362,
//! D363, D1320): projections, filters and set functions of arithmetic of
//! every scale, CASE, CAST, comparisons and three-valued logic, and calls
//! of functions with bodies over vectors, the fields and casts of
//! datetimes among them, give by batches what they give a row at a time,
//! of either engine; one that raises an error raises the error a row at
//! a time raises, after the rows before it.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// The rows written to each table.
const ROWS: i64 = 5000;

/// The values of row `k`: K, N an integer null every 11th, E a decimal of
/// scale 2, F an approximate number, S a string null every 7th, D a date,
/// B a truth value null every 5th.
fn row(k: i64) -> String {
    let number = if k % 11 == 0 { "NULL".to_owned() } else { (k * 7919 % 2000 - 1000).to_string() };
    let decimal = format!("{}.{:02}", k % 300 - 150, k % 100);
    let word = ["ab", "Cd", "", "eFg"][usize::try_from(k % 4).unwrap()];
    let string = if k % 7 == 0 { "NULL".to_owned() } else { format!("'{word}x{}'", k % 9) };
    let date = format!("DATE '{}-{:02}-{:02}'", 2000 + k % 20, 1 + k % 12, 1 + k % 28);
    let truth = ["TRUE", "FALSE", "TRUE", "FALSE", "NULL"][usize::try_from(k % 5).unwrap()];
    format!("({k}, {number}, {decimal}, {}E-1, {string}, {date}, {truth})", k % 77 - 30)
}

/// Tables R, of the row store, and C, of the column store, alike: written,
/// frozen, then changed.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, N INTEGER, E DECIMAL(9,2), F DOUBLE PRECISION, S VARCHAR(12), D DATE, B BOOLEAN";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R ({columns}) CREATE TABLE C ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(row).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET E = E * 2, N = NULL WHERE K BETWEEN 200 AND 260")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3050")).unwrap();
        a.commit().unwrap();
    }
    (db, a)
}

/// Projections, filters and set functions of expressions.
const QUERIES: [&str; 25] = [
    "SELECT K, N * 3 - E, E / 4, -N, F * 2 + N FROM {t} WHERE K < 300",
    "SELECT SUM(E * (1 - F / 100)), SUM(E * 0.95 * (1 + 0.08)), COUNT(*) FROM {t} WHERE N > -500",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE N * 2 + 1 > E AND E - N < 100",
    "SELECT K, CASE WHEN N > 0 THEN N * 10 WHEN N < -500 THEN E ELSE 0 END FROM {t} WHERE K < 400",
    "SELECT K, CASE WHEN N <> 0 THEN 1000 / N ELSE -1 END FROM {t} WHERE K < 600",
    "SELECT SUM(CASE WHEN B THEN E ELSE 0 END), SUM(CASE WHEN S LIKE 'a%' THEN 1 END) FROM {t}",
    "SELECT K, COALESCE(N, -1), NULLIF(N, 0) FROM {t} WHERE K < 300",
    "SELECT K, CAST(E AS INTEGER), CAST(N AS DECIMAL(12,3)), CAST(E AS DOUBLE PRECISION) FROM {t} WHERE K < 300",
    "SELECT K, ABS(N), ABS(F), ABS(E) FROM {t} WHERE K < 300",
    "SELECT K, UPPER(S), LOWER(S), CHAR_LENGTH(S), OCTET_LENGTH(S) FROM {t} WHERE K < 300",
    "SELECT K, EXTRACT(YEAR FROM D), EXTRACT(MONTH FROM D), EXTRACT(DAY FROM D) FROM {t} WHERE K < 300",
    "SELECT COUNT(*) FROM {t} WHERE EXTRACT(YEAR FROM D) = 2005",
    "SELECT K, EXTRACT(SECOND FROM CAST(D AS TIMESTAMP) + INTERVAL '7.5' SECOND), EXTRACT(HOUR FROM CAST(D AS TIMESTAMP) + INTERVAL '30' HOUR) FROM {t} WHERE K < 200",
    "SELECT K, D + INTERVAL '3' DAY, D - INTERVAL '1' MONTH FROM {t} WHERE K < 200",
    "SELECT COUNT(*) FROM {t} WHERE D + INTERVAL '10' DAY > DATE '2010-01-01'",
    "SELECT COUNT(*) FROM {t} WHERE (N > 0 OR B) AND NOT (E < 0 AND B IS NOT TRUE)",
    "SELECT K, N > 0 OR B, N > 0 AND B, B IS UNKNOWN FROM {t} WHERE K < 300",
    "SELECT COUNT(*) FROM {t} WHERE UPPER(S) = 'ABX1' OR CHAR_LENGTH(S) > 3",
    "SELECT COUNT(*) FROM {t} WHERE N IS DISTINCT FROM E OR N IN (1, 2, 3)",
    "SELECT MIN(N * E), MAX(N * E), AVG(F * N) FROM {t}",
    "SELECT B, SUM(E * 2), COUNT(N + 1) FROM {t} GROUP BY B",
    "SELECT Y, SUM(V), MAX(M) FROM (SELECT EXTRACT(YEAR FROM D) AS Y, E AS V, ABS(N) AS M FROM {t}) AS Q GROUP BY Y",
    "SELECT K FROM {t} WHERE S LIKE UPPER('%x1') OR S LIKE LOWER(S)",
    "SELECT K, N / 7, E / 3, N / E FROM {t} WHERE K < 300 AND E <> 0",
    "SELECT K, 1 + 2 * 3, N + 999999999999999 FROM {t} WHERE K < 100",
];

/// Verifies: [DESIGN] RULES.md D362, D363 [X3.135-1986 §5.9, §5.11, §5.18;
/// ISO/IEC 9075:1992 §6.6, §6.7, §6.9, §6.10, §6.14, §6.15; ISO/IEC
/// 9075-2:2023 §6.31]: projections, filters and set functions of
/// expressions, evaluated a vector at a time, give the rows a row at a
/// time gives, of either engine: arithmetic of scales unlike, approximate
/// numbers, CASE, COALESCE and NULLIF, CAST, ABS, UPPER, LOWER, lengths,
/// EXTRACT, datetime arithmetic, three-valued logic and IS DISTINCT.
#[test]
fn expressions_give_by_batch_what_rows_give() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in QUERIES {
            let sql = query.replace("{t}", table);
            let (batched, by_rows) = both(&a, &sql);
            assert!(by_rows.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {by_rows:?}");
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D362, D15 [X3.135-1986 §5.9 GR5, GR6a]: an
/// expression that overflows, passes 18 digits or divides by 0 of some
/// row raises, by batch, the error a row at a time raises, after the same
/// rows; a CASE whose untaken branch would raise raises nothing.
#[test]
fn raising_expressions_raise_as_rows_do() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in [
            "SELECT K, 10 / (N + 1000) FROM {t} WHERE K < 2000",
            "SELECT K, N * 900000000000000000 FROM {t}",
            "SELECT SUM(E * 99999999999999999) FROM {t}",
            "SELECT COUNT(*) FROM {t} WHERE N * 100000000000000000 > 0",
            "SELECT K FROM {t} WHERE K > 4000 AND 10 / (K - 4500) > 0",
            "SELECT K, CAST(E * 1000 AS DECIMAL(4,0)) FROM {t}",
            "SELECT K, F / (K - 1000) FROM {t} WHERE K < 1500",
            "SELECT K, CASE WHEN K <> 1000 THEN 10 / (K - 1000) END FROM {t} WHERE K < 1500",
        ] {
            let sql = query.replace("{t}", table);
            let (batched, by_rows) = both(&a, &sql);
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §6.7 GR1; X3.135-1986 §5.13 GR2] RULES.md
/// D1126: SUBSTRING of a constant start and length, and IN a list of
/// strings of it, give by batch what they give a row at a time, of either
/// engine: starts before, in and past the string, a length of 0, a list
/// with a null and with a string padded, NOT of it, and a grouping by the
/// substring. A negative length, a constant or a column's, raises by
/// batch the error a row at a time raises.
#[test]
fn substrings_and_lists_of_them_give_by_batch_what_rows_give() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in [
            "SELECT K, SUBSTRING(S FROM 1 FOR 2), SUBSTRING(S FROM 2), SUBSTRING(S FROM 0 FOR 3), SUBSTRING(S FROM 3 FOR 0) FROM {t} WHERE K < 300",
            "SELECT K, SUBSTRING(S FROM 9), SUBSTRING(S FROM -2 FOR 4), SUBSTRING(CAST(S AS CHAR(6)) FROM 3 FOR 9) FROM {t} WHERE K < 300",
            "SELECT COUNT(*), SUM(N) FROM {t} WHERE SUBSTRING(S FROM 1 FOR 2) IN ('ab', 'Cd')",
            "SELECT COUNT(*) FROM {t} WHERE SUBSTRING(S FROM 1 FOR 2) IN ('ab', 'x1', NULL)",
            "SELECT COUNT(*) FROM {t} WHERE NOT (SUBSTRING(S FROM 1 FOR 1) IN ('a', 'x ', NULL))",
            "SELECT COUNT(*) FROM {t} WHERE NOT (SUBSTRING(S FROM 1 FOR 1) IN ('a', 'x '))",
            "SELECT SUBSTRING(S FROM 1 FOR 2), COUNT(*), SUM(E) FROM {t} GROUP BY SUBSTRING(S FROM 1 FOR 2)",
            "SELECT K, SUBSTRING(S FROM 2 FOR N) FROM {t} WHERE K < 300",
            "SELECT K, SUBSTRING(S FROM 1 FOR -1) FROM {t} WHERE K < 300",
            "SELECT COUNT(*) FROM {t} WHERE SUBSTRING(S FROM 1 FOR -1) IN ('a', 'b')",
        ] {
            let sql = query.replace("{t}", table);
            let (batched, by_rows) = both(&a, &sql);
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
    let counted = rows(&a, "SELECT COUNT(*) FROM C WHERE SUBSTRING(S FROM 1 FOR 2) IN ('ab', 'Cd')").unwrap();
    assert_ne!(counted, ["0"], "some rows are kept");
}

/// Verifies: [ISO/IEC 9075:1992 §7.11 GR1; X3.135-1986 §5.18] RULES.md
/// D1127, D15: a filter holding a scalar subquery with no outer reference
/// gives by batch, the subquery run once and its value a constant, the
/// rows it gives a row at a time, of either engine: under a comparison,
/// arithmetic, NOT, OR and IS DISTINCT FROM, and of a subquery of no row,
/// a null. A subquery of two rows raises its error by batch as a row at a
/// time; after the error of an operand before it of the first row; and
/// not at all where no row is judged, or under a CASE no row takes.
#[test]
fn a_filter_s_subquery_of_no_outer_reference_is_run_once_as_rows_run_it() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in [
            "SELECT COUNT(*), SUM(E) FROM {t} WHERE E > (SELECT AVG(E) FROM {t} WHERE N > 0) AND SUBSTRING(S FROM 1 FOR 2) IN ('ab', 'Cd')",
            "SELECT COUNT(*) FROM {t} WHERE N > (SELECT MIN(N) FROM {t}) + 5 OR -N = (SELECT MAX(K) FROM {t} WHERE K < 0)",
            "SELECT COUNT(*) FROM {t} WHERE NOT (N > -(SELECT MAX(N) FROM {t}) / 2)",
            "SELECT COUNT(*) FROM {t} WHERE N IS DISTINCT FROM (SELECT MAX(N) FROM {t})",
            "SELECT COUNT(*) FROM {t} WHERE (SELECT MAX(K) FROM {t} WHERE K < 0) IS NULL AND K < 10",
            "SELECT K FROM {t} WHERE N > (SELECT N FROM {t} WHERE K < 3)",
            "SELECT K FROM {t} WHERE 10 / (K - K) > 0 AND N > (SELECT N FROM {t} WHERE K < 3)",
            "SELECT K FROM {t} WHERE K < -5 AND N > (SELECT N FROM {t} WHERE K < 3)",
            "SELECT K FROM (SELECT K, N FROM {t} WHERE K < -5) AS Q WHERE N + K > (SELECT N FROM {t} WHERE K < 3)",
            "SELECT COUNT(*) FROM {t} WHERE CASE WHEN K < 0 THEN (SELECT N FROM {t} WHERE K < 3) ELSE 1 END = 1",
        ] {
            let sql = query.replace("{t}", table);
            let (batched, by_rows) = both(&a, &sql);
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
    let counted = rows(&a, "SELECT COUNT(*) FROM C WHERE NOT (N > -(SELECT MAX(N) FROM C) / 2)").unwrap();
    assert_ne!(counted, ["0"], "some rows are kept");
    assert_eq!(rows(&a, "SELECT K FROM C WHERE N > (SELECT N FROM C WHERE K < 3)"), Err(Code::Cardinality), "two rows raise");
    // Whichever row is judged first divides by 0 before it runs the subquery.
    let before = "SELECT K FROM C WHERE 10 / (K - K) > 0 AND N > (SELECT N FROM C WHERE K < 3)";
    assert_eq!(rows(&a, before), Err(Code::Data), "the operand before the subquery raises first");
}

/// The datetimes of row `k`: K, TS a timestamp of six digits null every
/// 9th, T0 one of none, TZ one WITH TIME ZONE of five displacements, TM a
/// time of three digits, D a date.
fn moments(k: i64) -> String {
    let date = format!("{}-{:02}-{:02}", 1990 + k % 40, 1 + k % 12, 1 + k % 28);
    let time = format!("{:02}:{:02}:{:02}", k % 24, k * 7 % 60, k * 13 % 60);
    let stamp = if k % 9 == 0 { "NULL".to_owned() } else { format!("TIMESTAMP '{date} {time}.{:06}'", k * 104_729 % 1_000_000) };
    let zone = ["+00:00", "+05:30", "-08:00", "+14:00", "-12:59"][usize::try_from(k % 5).unwrap()];
    format!(
        "({k}, {stamp}, TIMESTAMP '{date} {time}', TIMESTAMP '{date} {time}.{:03}{zone}', TIME '{time}.{:03}', DATE '{date}')",
        k % 1000,
        k * 31 % 1000
    )
}

/// Tables RT, of the row store, and CT, of the column store, alike, of
/// datetimes written in UTC: frozen, then changed.
fn moment_tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, TS TIMESTAMP(6), T0 TIMESTAMP(0), TZ TIMESTAMP(3) WITH TIME ZONE, TM TIME(3), D DATE";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE RT ({columns}) CREATE TABLE CT ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["RT", "CT"] {
        for first in (0..3000).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(moments).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET TS = TS + INTERVAL '90' MINUTE, T0 = NULL WHERE K BETWEEN 700 AND 760")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 2000 AND 2040")).unwrap();
        a.commit().unwrap();
    }
    (db, a)
}

/// Fields and casts of datetimes, projected, filtered and grouped by.
const MOMENTS: [&str; 10] = [
    "SELECT K, EXTRACT(YEAR FROM TS), EXTRACT(MONTH FROM TS), EXTRACT(DAY FROM TS), EXTRACT(HOUR FROM TS), EXTRACT(MINUTE FROM TS), EXTRACT(SECOND FROM TS) FROM {t} WHERE K < 800",
    "SELECT K, EXTRACT(HOUR FROM T0), EXTRACT(SECOND FROM T0), EXTRACT(MINUTE FROM TM), EXTRACT(SECOND FROM TM), EXTRACT(DAY FROM D) FROM {t} WHERE K < 800",
    "SELECT K, EXTRACT(DAY FROM TZ), EXTRACT(HOUR FROM TZ), EXTRACT(MINUTE FROM TZ), EXTRACT(TIMEZONE_HOUR FROM TZ), EXTRACT(TIMEZONE_MINUTE FROM TZ) FROM {t} WHERE K < 800",
    "SELECT M, COUNT(*), MIN(K) FROM (SELECT EXTRACT(MINUTE FROM TS) AS M, K FROM {t}) AS Q GROUP BY M",
    "SELECT H, M, COUNT(*), MAX(K) FROM (SELECT EXTRACT(HOUR FROM TS) AS H, EXTRACT(MINUTE FROM T0) AS M, K FROM {t}) AS Q GROUP BY H, M",
    "SELECT COUNT(*), SUM(K) FROM {t} WHERE EXTRACT(HOUR FROM TS) < 7 AND EXTRACT(MINUTE FROM TS) <> 30",
    "SELECT K, CAST(TS AS DATE), CAST(TS AS TIME(2)), CAST(TS AS TIMESTAMP(1)), CAST(D AS TIMESTAMP(0)) FROM {t} WHERE K < 800",
    "SELECT K, CAST(TS AS TIMESTAMP(3) WITH TIME ZONE), CAST(TZ AS TIMESTAMP(0)), CAST(TZ AS DATE), CAST(TM AS TIME(0) WITH TIME ZONE) FROM {t} WHERE K < 800",
    "SELECT K, TS - EXTRACT(SECOND FROM TS) * INTERVAL '1' SECOND FROM {t} WHERE K < 800",
    "SELECT D2, COUNT(*) FROM (SELECT CAST(TS AS DATE) AS D2 FROM {t} WHERE EXTRACT(YEAR FROM TS) > 2020) AS Q GROUP BY D2",
];

/// Verifies: [DESIGN] RULES.md D1320, D363, D15 [ISO/IEC 9075:1992 §6.6
/// GR3, §6.10 GR9, GR10, GR11, §6.1 GR5, GR6]: every field of timestamps,
/// times and dates, with a time zone or without, and casts between the
/// datetime types, projected, filtered and grouped by, give by batch the
/// rows a row at a time gives, of either engine, in sessions of three
/// displacements, the fields of a type without time zone read as the
/// session shows them. A timestamp the session shows in the year before
/// 0001 raises SQLSTATE 22007 by batch as a row at a time, and nothing
/// where no row of its batch is read.
#[test]
fn fields_and_casts_of_datetimes_give_by_batch_what_rows_give() {
    let (_db, a) = moment_tables();
    for zone in ["INTERVAL '00:00' HOUR TO MINUTE", "INTERVAL '05:30' HOUR TO MINUTE", "INTERVAL -'08:00' HOUR TO MINUTE"] {
        exec(&a, &format!("SET TIME ZONE {zone}")).unwrap();
        for table in ["RT", "CT"] {
            for query in MOMENTS {
                let sql = query.replace("{t}", table);
                let (batched, by_rows) = both(&a, &sql);
                assert!(by_rows.as_ref().is_ok_and(|r| !r.is_empty()), "{sql} at {zone}: {by_rows:?}");
                assert_eq!(batched, by_rows, "{sql} at {zone}");
            }
        }
    }
    exec(&a, "SET TIME ZONE INTERVAL '05:30' HOUR TO MINUTE").unwrap();
    // Row 100 was written 04:40:40 in UTC.
    let shown = "SELECT EXTRACT(HOUR FROM TS), EXTRACT(MINUTE FROM TS), EXTRACT(SECOND FROM T0) FROM CT WHERE K = 100";
    assert_eq!(rows(&a, shown).unwrap(), ["10,10,40"]);
    exec(&a, "SET TIME ZONE INTERVAL '00:00' HOUR TO MINUTE").unwrap();
    for table in ["RT", "CT"] {
        exec(&a, &format!("INSERT INTO {table} (K, TS) VALUES (5000, TIMESTAMP '0001-01-01 00:30:00')")).unwrap();
    }
    a.commit().unwrap();
    exec(&a, "SET TIME ZONE INTERVAL -'01:00' HOUR TO MINUTE").unwrap();
    for table in ["RT", "CT"] {
        let (batched, by_rows) = both(&a, &format!("SELECT K, EXTRACT(MINUTE FROM TS) FROM {table}"));
        assert_eq!((batched, by_rows), (Err(Code::Data), Err(Code::Data)), "{table}: shown in the year before 0001");
        let (batched, by_rows) = both(&a, &format!("SELECT K, EXTRACT(MINUTE FROM TS) FROM {table} WHERE K < 600"));
        assert!(by_rows.is_ok() && batched == by_rows, "{table}: the rows before it");
    }
}

/// The rows of `sql`, sorted, or its error: read by batches where it can
/// be, and a row at a time.
fn both(a: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>) {
    let batched = sorted(rows(a, sql));
    exec(a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let by_rows = sorted(rows(a, sql));
    exec(a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (batched, by_rows)
}

/// `r`, its rows sorted.
fn sorted(r: Result<Vec<String>, Code>) -> Result<Vec<String>, Code> {
    r.map(|mut r| {
        r.sort();
        r
    })
}
