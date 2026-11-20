//! \[DESIGN\] Top-N and sorts given by batches (RULES.md D379, D380): a
//! FETCH FIRST of the column store reads its first keys' columns alone,
//! gathers its other columns of the rows it keeps, and tells the scan the
//! bound of its first key, so groups past it are not read; a whole sort
//! gives its rows as batches to what reads batches; a sort run in
//! parallel spills its runs past a work area; a sort held looks for a
//! cancel as it gives its rows. Each gives what the row store gives row
//! by row.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};
use crate::spilling::{SMALL, bytes};

/// Rows of each table: past four times the most rows a FETCH FIRST asks
/// for, so its other columns are read late.
const ROWS: i64 = 20_000;

/// The values of row `k`: K its key; A an integer, null every 9th; D a
/// decimal; F a date; T one of 40 texts, null every 13th; U a text of each
/// row; N the rows in order.
fn row(k: i64) -> String {
    let c = (k * 7919) % ROWS;
    let a = if k % 9 == 0 { "NULL".to_owned() } else { (c % 500 - 250).to_string() };
    let t = if k % 13 == 0 { "NULL".to_owned() } else { format!("'name {}'", c % 40) };
    let date = format!("DATE '{}-{:02}-{:02}'", 1992 + c % 7, 1 + k % 12, 1 + c % 28);
    format!("({k}, {a}, {}.{:02}, {date}, {t}, 'customer{c}@mail.example.com', {k})", c % 97, c % 100)
}

/// Tables R, of the row store, and C, of the column store in groups of
/// 2048 rows, alike, C frozen; background work done only when asked.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(16, 0))).unwrap();
    let columns = "(K INTEGER, A INTEGER, D DECIMAL(9,2), F DATE, T VARCHAR(20), U VARCHAR(60), N INTEGER)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let s = db.session("A");
    for first in (0..ROWS).step_by(1000) {
        let values: Vec<String> = (first..first + 1000).map(row).collect();
        for table in ["R", "C"] {
            exec(&s, &format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The queries each table answers alike, `{t}` its name.
const QUERIES: [&str; 18] = [
    "SELECT * FROM {t} ORDER BY N FETCH FIRST 10 ROWS ONLY",
    "SELECT * FROM {t} ORDER BY N DESC FETCH FIRST 10 ROWS ONLY",
    "SELECT K, U, T FROM {t} ORDER BY A FETCH FIRST 25 ROWS WITH TIES",
    "SELECT K, U FROM {t} ORDER BY A DESC NULLS LAST, K FETCH FIRST 7 ROWS ONLY",
    "SELECT K, U FROM {t} ORDER BY A NULLS FIRST, K FETCH FIRST 30 ROWS ONLY",
    "SELECT K, U, D FROM {t} ORDER BY D DESC, K FETCH FIRST 12 ROWS ONLY",
    "SELECT K, T FROM {t} ORDER BY F, K FETCH FIRST 9 ROWS ONLY",
    "SELECT K, A FROM {t} ORDER BY T DESC, K FETCH FIRST 11 ROWS ONLY",
    "SELECT K, U FROM {t} WHERE A > 100 ORDER BY N FETCH FIRST 5 ROWS ONLY",
    "SELECT K, U FROM {t} WHERE T = 'name 7' ORDER BY N DESC FETCH FIRST 5 ROWS ONLY",
    "SELECT K, U FROM {t} ORDER BY N OFFSET 15 ROWS FETCH FIRST 5 ROWS ONLY",
    "SELECT K, U FROM {t} ORDER BY N FETCH FIRST 5000 ROWS ONLY",
    "SELECT K + 1, U FROM {t} ORDER BY K",
    "SELECT K, U, D, F FROM {t} ORDER BY A DESC, K",
    "SELECT T, U, A FROM {t} ORDER BY F, K",
    "SELECT T, COUNT(*), MAX(U) FROM (SELECT * FROM {t} ORDER BY N FETCH FIRST 300 ROWS ONLY) X GROUP BY T",
    "SELECT DISTINCT T FROM (SELECT * FROM {t} ORDER BY K DESC FETCH FIRST 400 ROWS ONLY) X",
    "SELECT K, U FROM {t} WHERE A <> 7 ORDER BY A, K FETCH FIRST 3 ROWS ONLY",
];

/// The rows `s` reads of each query of `table`, in the order given, under
/// `set`.
fn answers(s: &Session, table: &str, set: &[&str]) -> Vec<Vec<String>> {
    for statement in set {
        exec(s, statement).unwrap();
    }
    let made = QUERIES.iter().map(|q| rows(s, &q.replace("{t}", table)).unwrap_or_else(|e| panic!("{q}: {e:?}"))).collect();
    for statement in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.PARALLEL_MIN_ROWS = 65536"] {
        exec(s, statement).unwrap();
    }
    made
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 GR5, GR7d] [DESIGN] RULES.md
/// D379, D380: a FETCH FIRST of the column store, its other columns read
/// late and its first key's bound told the scan, ONLY and WITH TIES, of
/// OFFSET, of each kind of key, each way and null ordering, under a filter
/// and one that could raise; a projection of a whole sort's batches; a
/// grouping and a DISTINCT of a top-N's batches. Each gives the rows the
/// row store gives row by row, in order, by batches, in parallel at a
/// degree of 16, and past a work area of 64 KiB.
#[test]
fn topped_sorts_give_what_rows_give() {
    let (db, s) = tables();
    let expected = answers(&s, "R", &["SET EXECUTOR.BATCHES = 'OFF'"]);
    assert_eq!(expected[0].len(), 10);
    assert_eq!(answers(&s, "C", &[]), expected, "by batches");
    assert_eq!(answers(&s, "R", &[]), expected, "the row store by batches");
    assert_eq!(answers(&s, "C", &["SET EXECUTOR.BATCHES = 'OFF'"]), expected, "a row at a time");
    let parallel = ["SET EXECUTOR.DEGREE = 16", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0", "SET EXECUTOR.MORSEL_ROWS = 512"];
    assert_eq!(answers(&s, "C", &parallel), expected, "in parallel");
    assert_eq!(answers(&s, "R", &parallel), expected, "the row store in parallel");
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    assert_eq!(answers(&s, "C", &[]), expected, "past 64 KiB");
    assert_eq!(answers(&s, "R", &parallel), expected, "in parallel past 64 KiB");
    assert_eq!(answers(&s, "C", &parallel), expected, "the column store in parallel past 64 KiB");
}

/// Verifies: [DESIGN] RULES.md D379, D361, D15: a top-N whose filter or
/// projection could raise tells the scan no bound, so it raises of a row
/// past its first rows as a row at a time raises it, a data exception; a top-N of rows changed after they
/// were frozen, updated past the bound and back within it, gives what the
/// row store gives.
#[test]
fn topped_sorts_raise_and_read_changes_as_rows() {
    let (_db, s) = tables();
    for q in [
        "SELECT K, 100 / (A - 7) FROM {t} ORDER BY N FETCH FIRST 5 ROWS ONLY",
        "SELECT K FROM {t} WHERE 100 / (A - 7) > 0 ORDER BY N FETCH FIRST 5 ROWS ONLY",
        "SELECT K FROM {t} WHERE 100 / (A - 7) > 0 ORDER BY N DESC FETCH FIRST 5 ROWS WITH TIES",
    ] {
        let of = |t: &str| rows(&s, &q.replace("{t}", t)).err();
        assert_eq!(of("C"), Some(Code::Data), "{q}: raises of a row past the first");
        assert_eq!(of("R"), Some(Code::Data), "{q}: the row store");
    }
    for t in ["R", "C"] {
        exec(&s, &format!("UPDATE {t} SET N = -N WHERE K BETWEEN 15000 AND 15004")).unwrap();
        exec(&s, &format!("UPDATE {t} SET N = N + 100000 WHERE K < 3")).unwrap();
        exec(&s, &format!("DELETE FROM {t} WHERE K BETWEEN 3 AND 5")).unwrap();
    }
    // Rows changed are in another place of each store, so peers come in
    // another order: each answer's rows compared as a set.
    let unordered = |mut answers: Vec<Vec<String>>| {
        for a in &mut answers {
            a.sort();
        }
        answers
    };
    let rows_of = |s: &Session| unordered(answers(s, "R", &["SET EXECUTOR.BATCHES = 'OFF'"]));
    assert_eq!(unordered(answers(&s, "C", &[])), rows_of(&s), "changed, not committed");
    s.commit().unwrap();
    assert_eq!(unordered(answers(&s, "C", &[])), rows_of(&s), "changed, committed");
}

/// Verifies: [DESIGN] RULES.md D380, D44: a sort held in memory looks for a
/// cancel as it gives its rows, its input read already. A cursor of a whole
/// sort of 200,000 rows, fetched from as another thread cancels, fails
/// before its last row (SQLCODE -920), and every fetch after it fails so
/// too.
#[test]
fn a_sort_held_looks_for_a_cancel() {
    let (_db, s) = tables();
    let mut cursor = s.open_cursor("SELECT R.K FROM R, (SELECT K FROM R WHERE K < 10) X ORDER BY R.K DESC").unwrap();
    assert!(cursor.fetch().unwrap().is_some());
    let done = Arc::new(AtomicBool::new(false));
    let (canceller, stop) = (s.canceller(), Arc::clone(&done));
    let cancelling = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            canceller.cancel();
            std::thread::yield_now();
        }
    });
    let failed = (1..200_000).find_map(|_| cursor.fetch().err());
    done.store(true, Ordering::Relaxed);
    cancelling.join().unwrap();
    assert_eq!(failed.map(|e| e.code), Some(Code::Cancelled), "before the last row");
    assert_eq!(cursor.fetch().err().map(|e| e.code), Some(Code::Cancelled), "cut short");
}
