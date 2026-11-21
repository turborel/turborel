//! How fast window functions run over batches (RULES.md D560), of every
//! frame type, on V, a row store table, and VC, a column store one, of the
//! same rows: 2,000,000 unless `ROWS_VAR` says otherwise. A rank of one
//! partition and of 1,000; set functions of frames that grow, of ROWS,
//! RANGE and GROUPS that move, with exclusions; LAG; a value function;
//! approximate numbers summed in order; COUNT of 200,000 small partitions.
//! Each window's values are summed by the query, so the rows are not
//! fetched one by one. Each runs at EXECUTOR.DEGREE 1 and 6. Run with
//! `cargo xtask bench`, on the harness (RULES.md R-BENCH-1); with
//! `ONLY_VAR` of `analytics.rs` set to `windows`, these alone.

use super::harness::{Bench, Case};
use turborel::db::{Database, Session};

/// The variable giving the rows of V and VC.
const ROWS_VAR: &str = "TURBOREL_BENCH_WINDOWS_ROWS";
/// Rows of V and VC unless `ROWS_VAR` gives them.
const ROWS: u64 = 2_000_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;
/// The parallel degrees each query runs at.
const DEGREES: [usize; 2] = [1, 6];

/// Queries of `{t}`, each with the name it is reported by.
const QUERIES: [(&str, &str); 11] = [
    ("ROW_NUMBER of one partition", "ROW_NUMBER() OVER (ORDER BY B)"),
    ("RANK of 1,000 partitions", "RANK() OVER (PARTITION BY G ORDER BY A)"),
    ("running SUM, RANGE that grows", "SUM(A) OVER (PARTITION BY G ORDER BY K)"),
    ("SUM over ROWS 10 each side", "SUM(A) OVER (PARTITION BY G ORDER BY K ROWS BETWEEN 10 PRECEDING AND 10 FOLLOWING)"),
    ("AVG over RANGE 1,000 back", "AVG(A) OVER (PARTITION BY G ORDER BY K RANGE BETWEEN 1000 PRECEDING AND CURRENT ROW)"),
    (
        "MIN over GROUPS 2 each side, EXCLUDE CURRENT ROW",
        "MIN(A) OVER (PARTITION BY G ORDER BY A GROUPS BETWEEN 2 PRECEDING AND 2 FOLLOWING EXCLUDE CURRENT ROW)",
    ),
    (
        "MAX over ROWS 100 back, EXCLUDE TIES",
        "MAX(A) OVER (PARTITION BY G ORDER BY K ROWS BETWEEN 100 PRECEDING AND CURRENT ROW EXCLUDE TIES)",
    ),
    ("LAG", "LAG(A) OVER (PARTITION BY G ORDER BY K)"),
    ("FIRST_VALUE over ROWS 5 back", "FIRST_VALUE(A) OVER (PARTITION BY G ORDER BY K ROWS BETWEEN 5 PRECEDING AND CURRENT ROW)"),
    ("approximate running SUM", "SUM(F) OVER (PARTITION BY G ORDER BY K ROWS UNBOUNDED PRECEDING)"),
    ("COUNT of 200,000 partitions", "COUNT(*) OVER (PARTITION BY H ORDER BY K)"),
];

/// The rows of V and VC.
fn rows() -> u64 {
    std::env::var(ROWS_VAR).ok().and_then(|v| v.parse().ok()).unwrap_or(ROWS)
}

/// Schema B: V and VC, filled with `n` rows.
fn database(n: u64) -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE V (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, A INTEGER, B INTEGER, F DOUBLE PRECISION)
           CREATE TABLE VC (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, A INTEGER, B INTEGER, F DOUBLE PRECISION) USING columnstore",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..n).step_by(PER_INSERT as usize) {
        let values: Vec<String> = (first..(first + PER_INSERT).min(n))
            .map(|k| format!("({k}, {}, {}, {}, {}, {}.25)", k % 1_000, k * 7_919 % 200_000, k % 97, k * 104_729 % n, k % 13))
            .collect();
        for table in ["V", "VC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        s.commit().unwrap();
    }
    db.maintain().unwrap();
    (db, s)
}

/// Runs `sql`, its rows each fetched, then commits: `n`, the rows read.
fn fetched(s: &Session, sql: &str, n: u64) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    while cursor.fetch().unwrap().is_some() {}
    drop(cursor);
    s.commit().unwrap();
    n
}

/// The workloads of each table at each degree.
pub fn workloads(bench: &Bench) {
    let n = rows();
    let (_db, s) = database(n);
    for (table, label) in [("V", ""), ("VC", " (columnstore)")] {
        for (name, call) in QUERIES {
            let sql = format!("SELECT COUNT(*), SUM(W) FROM (SELECT {call} AS W FROM {table}) AS Q");
            for degree in DEGREES {
                s.run(&format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
                bench.measure(Case::new(format!("window {name}{label}"), "rows").threads(degree), || fetched(&s, &sql, n));
            }
        }
    }
    s.run("SET EXECUTOR.DEGREE TO DEFAULT").unwrap();
}
