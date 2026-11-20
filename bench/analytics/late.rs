//! How fast a column store table's selective queries run, the columns a
//! filter or a join keeps few rows of read late (RULES.md D360, D361): of
//! LC, of 400,000 rows frozen, sums and fetches of other columns of 1% and
//! 0.1% of the rows by a scattered column, of half of them, a count of 1%
//! alone, rows found by a dictionary of texts and by texts coded by
//! symbols, a query shaped as TPC-H's Q6 (a year of dates, a range of
//! discounts and of quantities), a grouping of 1%, a filter of an
//! expression the scan is not told, and joins with DIM, a table of 100
//! keys that match 1% of LC's rows (RULES.md R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

/// Rows of LC.
const ROWS: u64 = 400_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;
/// Times a measured run reads its query, so a run lasts long enough to
/// be measured apart from the host's other work.
const REPEATS: u64 = 10;
/// The keys of H, of which DIM holds the first 100.
const KEYS: u64 = 10_000;

/// Schema B with LC, in the column store: K its key, in order; A, B
/// integers; C scattered, each row's own; D a price of two digits; E an
/// approximate number; F a date of seven years; G one of 10; H one of
/// `KEYS`; T a text of 300 values; U a text of each row, alike in its
/// parts; S a CHARACTER string. And DIM, in the row store, H its first 100
/// keys and a name each. Background work runs only when asked (JOBS.POLICY
/// MANUAL), so no freeze runs while a case is measured.
fn database() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE LC (K INTEGER, A INTEGER, B INTEGER, C INTEGER, D DECIMAL(9,2), E DOUBLE PRECISION, F DATE,
             G INTEGER, H INTEGER, T VARCHAR(20), U VARCHAR(60), S CHAR(16)) USING columnstore
           CREATE TABLE DIM (H INTEGER, NAME VARCHAR(20))",
    )
    .unwrap();
    let s = db.session("B");
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT).map(row).collect();
        s.run(&format!("INSERT INTO LC VALUES {}", rows.join(", "))).unwrap();
    }
    let names: Vec<String> = (0_u32..100_u32).map(|h| format!("({h}, 'key {h}')")).collect();
    s.run(&format!("INSERT INTO DIM VALUES {}", names.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// The values of LC's row `k`.
fn row(k: u64) -> String {
    let c = scatter(k, ROWS);
    format!(
        "({k}, {}, {}, {c}, {}.{:02}, {}.5, DATE '{}-{:02}-{:02}', {}, {}, 'name {}', 'customer{k}@mail{}.example.com', 's {}')",
        c % 1_000_000,
        k % 50,
        c % 11,
        c % 100,
        c % 977,
        1992 + c % 7,
        1 + k % 12,
        1 + c % 28,
        k % 10,
        c % KEYS,
        c % 300,
        k % 7,
        k % 977
    )
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

/// The queries measured, each named.
const QUERIES: [(&str, &str); 14] = [
    ("COUNT of 1% by a scattered column", "SELECT COUNT(*) FROM LC WHERE C < 4000"),
    ("SUMs of 2 columns of 1%", "SELECT SUM(A), SUM(D) FROM LC WHERE C < 4000"),
    ("SUMs of 4 columns of 0.1%", "SELECT SUM(A), SUM(B), SUM(D), SUM(E) FROM LC WHERE C < 400"),
    ("SUMs of 2 columns of half", "SELECT SUM(A), SUM(D) FROM LC WHERE C < 200000"),
    ("fetch 5 columns of 0.1%", "SELECT K, A, D, T, U FROM LC WHERE C < 400"),
    ("fetch 3 columns by a dictionary text", "SELECT K, A, U FROM LC WHERE T = 'name 7'"),
    ("COUNT by a text coded by symbols", "SELECT COUNT(*) FROM LC WHERE U = 'customer1234@mail2.example.com'"),
    (
        "fetch 3 columns by texts coded by symbols",
        "SELECT K, A, D FROM LC WHERE U IN ('customer7@mail0.example.com', 'customer8@mail1.example.com')",
    ),
    (
        "Q6: SUM of a product by dates, discounts, quantities",
        "SELECT SUM(E * D) FROM LC WHERE F >= DATE '1995-01-01' AND F < DATE '1996-01-01' AND D BETWEEN 5 AND 7 AND B < 24",
    ),
    ("GROUP BY 10 groups of 1%", "SELECT G, COUNT(*), SUM(A), AVG(D) FROM LC WHERE C < 4000 GROUP BY G"),
    ("SUMs of 2 columns of 1% by an expression", "SELECT SUM(A), SUM(D) FROM LC WHERE C + 1 < 4001"),
    ("fetch 3 columns of 1% by an expression", "SELECT K, U, S FROM LC WHERE C * 2 < 8000"),
    ("SUMs of a join keeping 1%", "SELECT SUM(LC.A), SUM(LC.D) FROM LC JOIN DIM ON LC.H = DIM.H"),
    ("fetch texts of a join keeping 1%", "SELECT LC.U, LC.S, DIM.NAME FROM LC JOIN DIM ON LC.H = DIM.H"),
];

/// Measures each query of LC.
pub fn workloads(bench: &Bench) {
    let db = database();
    let s = db.session("B");
    // On one thread, so a case measures decoding, not the threads a query
    // runs on.
    s.run("SET EXECUTOR.DEGREE = 1").unwrap();
    for (name, sql) in QUERIES {
        bench.measure(Case::new(format!("{name} (late)"), "rows"), || {
            (0..REPEATS).for_each(|_| {
                fetched(&s, sql);
            });
            ROWS * REPEATS
        });
    }
}
