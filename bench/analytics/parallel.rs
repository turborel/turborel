//! How much faster queries run by morsels on several threads than on one
//! (RULES.md D290 to D297), and how much faster an operator granted more
//! of the memory budget runs than one held to 4 MiB (RULES.md D298), on
//! P, a row store table of 2,000,000 rows, and PC, a column store one of
//! the same rows: a count by a filter, groupings of 1,000 and of 200,000
//! groups, DISTINCT, a sort read whole and its first rows, a hash join with
//! Q, a table of 100,000 rows, window functions of 1,000 partitions,
//! EXCEPT, and a count of half the rows by an index's range. Each runs at
//! EXECUTOR.DEGREE 1 and at the system's threads, the harness's speedup
//! the A/B of the two; the groupings, the sort and the join run at
//! EXECUTOR.DEGREE 1 with MEMORY.WORK_BYTES of 4 MiB too. Run with `cargo
//! xtask bench`, on the harness (RULES.md R-BENCH-1); with `ONLY_VAR` of
//! `analytics.rs` set to `parallel`, these alone.

use super::harness::{Bench, Case};
use turborel::db::{Database, Session};

/// Rows of P and of PC.
const ROWS: u64 = 2_000_000;
/// Rows of Q.
const DIMENSION: u64 = 100_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// Queries of `{t}`, each with the name it is reported by.
const QUERIES: [(&str, &str); 10] = [
    ("COUNT by a filter of 2,000,000 rows", "SELECT COUNT(*), SUM(A) FROM {t} WHERE MOD(B, 10) < 3"),
    ("GROUP BY 1,000 groups", "SELECT G, COUNT(*), SUM(A), AVG(B) FROM {t} GROUP BY G"),
    ("GROUP BY 200,000 groups", "SELECT H, COUNT(*), SUM(A) FROM {t} GROUP BY H"),
    ("DISTINCT of 200,000 rows", "SELECT DISTINCT H FROM {t}"),
    ("ORDER BY, every row", "SELECT K, B FROM {t} ORDER BY B"),
    ("ORDER BY, first 10 rows", "SELECT K, B FROM {t} ORDER BY B, K FETCH FIRST 10 ROWS ONLY"),
    ("hash join with 100,000 rows", "SELECT COUNT(*), SUM(Q.V) FROM {t} JOIN Q ON {t}.H = Q.H"),
    ("window functions of 1,000 partitions", "SELECT K, SUM(A) OVER (PARTITION BY G ORDER BY K) FROM {t}"),
    ("EXCEPT of half the rows", "SELECT H, G FROM {t} EXCEPT SELECT H, G FROM {t} WHERE K < 1000000"),
    ("COUNT of half the rows by an index's range", "SELECT COUNT(*), SUM(A) FROM {t} WHERE K BETWEEN 500000 AND 1499999"),
];

/// Schema B: P, PC and Q, filled.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE P (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, A INTEGER, B INTEGER)
           CREATE TABLE PC (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, A INTEGER, B INTEGER) USING columnstore
           CREATE TABLE Q (H INTEGER, V INTEGER)",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let values: Vec<String> = (first..first + PER_INSERT)
            .map(|k| format!("({k}, {}, {}, {}, {})", k % 1_000, k * 7_919 % 200_000, k % 97, k * 104_729 % ROWS))
            .collect();
        for table in ["P", "PC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        s.commit().unwrap();
    }
    for first in (0..DIMENSION).step_by(PER_INSERT as usize) {
        let values: Vec<String> = (first..first + PER_INSERT).map(|h| format!("({}, {h})", h * 2)).collect();
        s.run(&format!("INSERT INTO Q VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// Rows `sql` gives, each fetched.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// The workloads, alone and in parallel, of each table.
pub fn workloads(bench: &Bench) {
    let (_db, s) = database();
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    for (table, label) in [("P", ""), ("PC", " (columnstore)")] {
        for (name, sql) in QUERIES {
            let sql = sql.replace("{t}", table);
            for degree in [1, threads] {
                s.run(&format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
                bench.measure(Case::new(format!("{name}{label}"), "rows").threads(degree), || {
                    fetched(&s, &sql);
                    ROWS
                });
            }
        }
        s.run("SET EXECUTOR.DEGREE = 1").unwrap();
        s.run("SET MEMORY.WORK_BYTES = 4194304").unwrap();
        for (name, sql) in
            QUERIES.iter().filter(|(n, _)| n.starts_with("GROUP") || n.starts_with("ORDER BY, every") || n.starts_with("hash"))
        {
            let sql = sql.replace("{t}", table);
            bench.measure(Case::new(format!("{name}{label}, 4 MiB work area"), "rows"), || {
                fetched(&s, &sql);
                ROWS
            });
        }
        s.run("SET MEMORY.WORK_BYTES TO DEFAULT").unwrap();
    }
    s.run("SET EXECUTOR.DEGREE TO DEFAULT").unwrap();
}
