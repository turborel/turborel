//! \[DESIGN\] Expressions evaluated a vector at a time (RULES.md D362,
//! D363): projections, filters and set functions of arithmetic of every
//! scale, CASE, CAST, comparisons and three-valued logic, and calls of
//! functions with bodies over vectors give by batches what they give a
//! row at a time, of either engine; one that raises an error raises the
//! error a row at a time raises, after the rows before it.

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
    let n = if k % 11 == 0 { "NULL".to_owned() } else { (k * 7919 % 2000 - 1000).to_string() };
    let e = format!("{}.{:02}", k % 300 - 150, k % 100);
    let s =
        if k % 7 == 0 { "NULL".to_owned() } else { format!("'{}x{}'", ["ab", "Cd", "", "eFg"][usize::try_from(k % 4).unwrap()], k % 9) };
    let d = format!("DATE '{}-{:02}-{:02}'", 2000 + k % 20, 1 + k % 12, 1 + k % 28);
    let b = ["TRUE", "FALSE", "TRUE", "FALSE", "NULL"][usize::try_from(k % 5).unwrap()];
    format!("({k}, {n}, {e}, {}E-1, {s}, {d}, {b})", k % 77 - 30)
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
