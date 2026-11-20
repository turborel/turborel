//! How fast groupings run on one thread (RULES.md D31, D356): on GR, a row
//! store table of 1,000,000 rows, and GRC, a column store one of the same
//! rows, grouped by 10, 1,000 and 200,000 values of an integer, by two
//! columns, by a text of 1,000 values, by a column one row in eight of
//! which is null, each with COUNT(*) and SUM; by 1,000 groups with every
//! built-in set function on integers, decimals, doubles and texts; and
//! DISTINCT of 200,000 values and of two columns. Run alone with `ONLY_VAR`
//! of `analytics.rs` set to `grouping`, at EXECUTOR.DEGREE 1.

use super::harness::{Bench, Case};
use turborel::db::{Database, Session};

/// Rows of GR and of GRC.
const ROWS: u64 = 1_000_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// Queries of `{t}`, each with the name it is reported by.
const QUERIES: [(&str, &str); 11] = [
    ("GROUP BY 10 groups", "SELECT L, COUNT(*), SUM(A) FROM {t} GROUP BY L"),
    ("GROUP BY 1,000 groups", "SELECT M, COUNT(*), SUM(A) FROM {t} GROUP BY M"),
    ("GROUP BY 200,000 groups", "SELECT H, COUNT(*), SUM(A) FROM {t} GROUP BY H"),
    ("GROUP BY two columns, 37,000 groups", "SELECT M, J, COUNT(*), SUM(A) FROM {t} GROUP BY M, J"),
    ("GROUP BY a text of 1,000 values", "SELECT T, COUNT(*), SUM(A) FROM {t} GROUP BY T"),
    ("GROUP BY a column of nulls, 101 groups", "SELECT N, COUNT(*), SUM(N) FROM {t} GROUP BY N"),
    (
        "GROUP BY 1,000 groups, every set function",
        "SELECT M, COUNT(*), COUNT(N), SUM(A), SUM(E), SUM(X), AVG(A), AVG(E), AVG(X), MIN(A), MAX(A), MIN(X), MAX(E), MIN(T), MAX(T) FROM {t} GROUP BY M",
    ),
    ("DISTINCT of 200,000 values", "SELECT DISTINCT H FROM {t}"),
    ("DISTINCT of two columns, 37,000 rows", "SELECT DISTINCT M, J FROM {t}"),
    ("COUNT(DISTINCT) of 200,000 values", "SELECT COUNT(DISTINCT H) FROM {t}"),
    ("GROUP BY 200,000 groups, past 4 MiB", "SELECT H, COUNT(*), SUM(A) FROM {t} GROUP BY H"),
];

/// GR and GRC, filled: K the key, L of 10 values, M of 1,000, J of 37, H
/// of 200,000, A an integer, E a decimal, X a double, T a text of 1,000
/// values, N of 100 values, null one row in eight.
fn database() -> (Database, Session) {
    let db = Database::new();
    let columns = "K INTEGER PRIMARY KEY, L INTEGER, M INTEGER, J INTEGER, H INTEGER, A INTEGER, E DECIMAL(9,2), X DOUBLE PRECISION, T VARCHAR(12), N INTEGER";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION B CREATE TABLE GR ({columns}) CREATE TABLE GRC ({columns}) USING columnstore"))
        .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let values: Vec<String> = (first..first + PER_INSERT)
            .map(|k| {
                let n = if k % 8 == 0 { "NULL".to_owned() } else { (k % 100).to_string() };
                format!(
                    "({k}, {}, {}, {}, {}, {}, {}.{:02}, {}.5, 'name {}', {n})",
                    k % 10,
                    k % 1_000,
                    k % 37,
                    k * 7_919 % 200_000,
                    k % 97,
                    k % 1_000,
                    k % 100,
                    k % 333,
                    k * 31 % 1_000
                )
            })
            .collect();
        for table in ["GR", "GRC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        s.commit().unwrap();
    }
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

/// The groupings of each table, on one thread.
pub fn workloads(bench: &Bench) {
    let (_db, s) = database();
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    for (table, label) in [("GR", ""), ("GRC", " (columnstore)")] {
        for (name, sql) in QUERIES {
            let sql = sql.replace("{t}", table);
            if name.ends_with("4 MiB") {
                s.run("SET MEMORY.WORK_BYTES = 4194304").unwrap();
            }
            bench.measure(Case::new(format!("{name}{label}"), "rows"), || {
                fetched(&s, &sql);
                ROWS
            });
            s.run("SET MEMORY.WORK_BYTES TO DEFAULT").unwrap();
        }
    }
    s.run("SET EXECUTOR.DEGREE TO DEFAULT").unwrap();
}
