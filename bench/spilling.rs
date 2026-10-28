//! How fast operators run that spill (RULES.md D38): a sort, grouping,
//! DISTINCT, a hash join, an outer join and EXCEPT, each over 50,000 rows
//! of a table, run in memory and spilled, so the cost of spilling shows.
//! Run with `cargo xtask bench`, on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use harness::{Bench, Case};
use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

/// Rows in each of T and U.
const ROWS: u64 = 50_000;
/// WORK_MEMORY and SPILL_BATCH: a work area that holds every operator's
/// rows, then 1 MiB, and the least, 64 KiB, both of which spill, in the
/// default batch, 16 KiB, and the least, 4 KiB.
const MEMORIES: &[(i128, i128, &str)] = &[
    (256 << 20, 16 << 10, "in memory"),
    (1 << 20, 16 << 10, "spilled, 1 MiB"),
    (64 << 10, 16 << 10, "spilled, 64 KiB"),
    (64 << 10, 4 << 10, "spilled, 64 KiB, 4 KiB batches"),
];

/// The operators measured, each a query, and the rows it reads.
const QUERIES: &[(&str, &str, u64)] = &[
    ("sort", "SELECT S, K FROM T ORDER BY S", ROWS),
    ("grouping", "SELECT G, COUNT(*), SUM(K) FROM T GROUP BY G", ROWS),
    ("DISTINCT", "SELECT DISTINCT S FROM T", ROWS),
    ("hash join", "SELECT T.K, U.K FROM T, U WHERE T.K = U.K", 2 * ROWS),
    ("outer join", "SELECT T.K, U.K FROM T FULL OUTER JOIN U ON T.S = U.S", 2 * ROWS),
    ("EXCEPT", "SELECT S FROM T EXCEPT SELECT S FROM U", 2 * ROWS),
];

fn main() {
    let bench = Bench::new("spilling");
    let db = database();
    let s = db.session("B");
    for &(memory, batch, how) in MEMORIES {
        db.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(memory, 0))).unwrap();
        db.set_setting("SPILL_BATCH", &Value::Exact(Decimal::new(batch, 0))).unwrap();
        for &(operator, sql, read) in QUERIES {
            bench.measure(Case::new(format!("{operator}, {how}"), "rows read"), || {
                fetched(&s, sql);
                read
            });
        }
    }
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

/// Schema B with tables T and U of `ROWS` rows each: `K` every key once, in
/// U from the other end, `G` 5,000 groups, `S` 20,000 strings in T, half
/// of them in U.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER NOT NULL, S CHAR(20))
           CREATE TABLE U (K INTEGER NOT NULL, G INTEGER NOT NULL, S CHAR(20))",
    )
    .unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        let other = ROWS - 1 - k;
        s.run(&format!("INSERT INTO T VALUES ({k}, {}, 's {}')", k % 5_000, (k * 7) % 20_000)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({other}, {}, 's {}')", other % 5_000, other % 20_000 + 10_000)).unwrap();
    }
    s.commit().unwrap();
    db
}
