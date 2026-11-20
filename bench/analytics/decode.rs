//! How fast a column store table's chunks are decoded (RULES.md D352,
//! D353): of DC, of 400,000 rows frozen, the sum of a column of prices,
//! decimals of two digits (ALP), of one of integers in a frame of
//! reference of 21 bits, of one whose 1,024 values are a dictionary, and
//! of one of runs; a count by a condition on a dictionary of texts, and
//! on texts coded by a table of symbols, so every row of each is decoded
//! (RULES.md R-BENCH-1).

use super::harness::{Bench, Case};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows of DC.
const ROWS: u64 = 400_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;
/// Times a measured run reads its query, so a run lasts long enough to
/// be measured apart from the host's other work.
const REPEATS: u64 = 20;

/// Schema B with DC, in the column store: K its key; P a price of two
/// digits; F an integer of 21 bits; D one of 1,024 values far apart; R
/// runs of 5,000 rows; T a text of 300 values; U a text of each row,
/// alike in its parts. Background work runs only when asked (JOBS.POLICY
/// MANUAL), so no freeze or collection runs while a case is measured.
fn database() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE DC (K INTEGER, P DOUBLE PRECISION, F INTEGER, D BIGINT, R INTEGER, T VARCHAR(20), U VARCHAR(60))
             USING columnstore",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| {
                let c = super::harness::scatter(k, ROWS);
                format!(
                    "({k}, {}.{:02}, {}, {}, {}, 'name {}', 'customer{k}@mail{}.example.com')",
                    c % 5_000,
                    c % 100,
                    c % 2_000_000,
                    (c % 1_024) * 1_000_000_007,
                    k / 5_000,
                    c % 300,
                    k % 7
                )
            })
            .collect();
        s.run(&format!("INSERT INTO DC VALUES {}", rows.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The rows of `sql`, each fetched, counted.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// Measures each decoding of DC.
pub fn workloads(bench: &Bench) {
    let db = database();
    let s = db.session("B");
    // On one thread, so a case measures decoding, not the threads a query
    // runs on.
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    for (name, sql) in [
        ("SUM of prices as decimals", "SELECT SUM(P) FROM DC"),
        ("SUM of a frame of 21 bits", "SELECT SUM(F) FROM DC"),
        ("SUM of a dictionary of 1,024 integers", "SELECT SUM(D) FROM DC"),
        ("SUM of runs of 5,000 rows", "SELECT SUM(R) FROM DC"),
        ("SUMs of prices, a frame and a dictionary", "SELECT SUM(P), SUM(F), SUM(D) FROM DC"),
        ("COUNT by a dictionary of texts", "SELECT COUNT(*) FROM DC WHERE T = 'name 7'"),
        ("COUNT by texts coded by symbols", "SELECT COUNT(*) FROM DC WHERE U LIKE 'customer1%'"),
    ] {
        bench.measure(Case::new(format!("{name} (decode)"), "rows"), || {
            (0..REPEATS).for_each(|_| {
                fetched(&s, sql);
            });
            ROWS * REPEATS
        });
    }
}
