//! How fast joins run (RULES.md D35): an equality of a column of each of
//! two tables, the same tables joined on two keys, three tables written in
//! an order that pairs the two large ones first, a small table's rows each
//! probing an index of a large one (`exec/join/indexed.rs`), inner and
//! LEFT, IN and < ALL over a subquery (`exec/subquery/quantified.rs`), a
//! correlated EXISTS subquery, decorrelated (`exec/subquery.rs`), and IN
//! lists of constants of several lengths, judged one after another or by
//! a set (`logical/sets.rs`). Run with `cargo xtask bench`, on the harness
//! (RULES.md R-BENCH-1).

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

/// Rows in each of the large tables T and U, and in the small one S.
const ROWS: u64 = 2_000;
const SMALL: u64 = 20;

fn main() {
    let bench = Bench::new("joins");
    let db = database();
    let s = db.session("B");
    bench.measure(Case::new("join on one key, 2,000 x 2,000 rows", "rows"), || joined(&s, "SELECT T.V, U.W FROM T, U WHERE T.K = U.K"));
    bench.measure(Case::new("join on two keys, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V, U.W FROM T, U WHERE T.K = U.K AND T.G = U.G")
    });
    bench.measure(Case::new("join 2,000 x 2,000 x 20 rows, large tables first", "rows"), || {
        joined(&s, "SELECT T.V, U.W FROM T, U, S WHERE T.K = S.K AND U.K = S.K")
    });
    bench.measure(Case::new("join 20 x 2,000 rows, the large one indexed", "rows"), || {
        joined(&s, "SELECT S.K, I.W FROM S, I WHERE S.K = I.K")
    });
    bench.measure(Case::new("LEFT join 20 x 2,000 rows, the large one indexed", "rows"), || {
        joined(&s, "SELECT S.K, I.W FROM S LEFT JOIN I ON S.K = I.K")
    });
    for length in [4_u64, 8, 16, 64] {
        let list = (0..length).map(|n| (n * 31).to_string()).collect::<Vec<_>>().join(", ");
        let query = format!("SELECT T.V FROM T WHERE T.K IN ({list})");
        bench.measure(Case::new(format!("IN a list of {length} constants, 2,000 rows"), "rows"), || joined(&s, &query));
    }
    bench.measure(Case::new("IN a subquery, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V FROM T WHERE T.K IN (SELECT U.K FROM U WHERE U.G < 6)")
    });
    bench.measure(Case::new("< ALL a subquery, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V FROM T WHERE T.K < ALL (SELECT U.K FROM U WHERE U.G = 3)")
    });
    bench.measure(Case::new("IN a subquery, a row with a null, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V FROM T WHERE (T.K, CASE WHEN T.G = 3 THEN NULL ELSE T.G END) IN (SELECT U.K, U.G FROM U)")
    });
    bench.measure(Case::new("< ALL a subquery, its rows with nulls, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V FROM T WHERE (T.G, T.K) < ALL (SELECT U.G, CASE WHEN U.K > 100 THEN NULL ELSE U.K END FROM U WHERE U.G > 3)")
    });
    bench.measure(Case::new("correlated EXISTS, 2,000 x 2,000 rows", "rows"), || {
        joined(&s, "SELECT T.V FROM T WHERE EXISTS (SELECT * FROM U WHERE U.K = T.K AND U.G = 3)")
    });
}

/// The rows of `sql`, each fetched, counted.
fn joined(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// Schema B with tables T and U of `ROWS` rows each, every key of T in U,
/// I of U's rows with an index on K, and S of `SMALL` of their keys.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER NOT NULL, V CHAR(10))
           CREATE TABLE U (K INTEGER NOT NULL, G INTEGER NOT NULL, W CHAR(10))
           CREATE TABLE S (K INTEGER NOT NULL)
           CREATE TABLE I (K INTEGER NOT NULL, G INTEGER NOT NULL, W CHAR(10))",
    )
    .unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, {}, 'v {k}')", k % 7)).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({}, {}, 'w {k}')", ROWS - 1 - k, (ROWS - 1 - k) % 7)).unwrap();
    }
    for k in 0..SMALL {
        s.run(&format!("INSERT INTO S VALUES ({})", k * 97)).unwrap();
    }
    s.run("INSERT INTO I SELECT * FROM U").unwrap();
    s.commit().unwrap();
    db.create_index("I_K", "B.I", &["K"], "btree").unwrap();
    db
}
