//! How fast FETCH FIRST and groupings run whose plans read their rows in
//! an order (RULES.md D550 to D552). O is a row-store table of 200,000
//! rows, keyed by K, indexed on G, of 2,000 groups, whose X is 1 in nine
//! rows of ten. P is a row-store table keyed by every tenth K. OC is a
//! column-store table of O's rows kept in the order of G. Measured are the
//! first 10 rows by the key of a filter that keeps most rows and of a join
//! of O and P, the first 10 groups of G by its index and by OC's order,
//! every group of G of each, and DISTINCT G of OC. Run alone with
//! `ONLY_VAR` set to `goals` (RULES.md R-BENCH-1).

use super::harness::{Bench, Case, scatter};
use turborel::db::Database;

/// Rows in the tables O and OC.
const ROWS: u64 = 200_000;
/// Rows each INSERT of the setup writes.
const PER_INSERT: u64 = 1_000;

/// Measures each query, each run counted as one.
pub fn workloads(bench: &Bench) {
    let db = database();
    let s = db.session("B");
    for (name, sql) in [
        ("FETCH FIRST 10 by the key of a filter of 9 rows in 10", "SELECT K, V FROM O WHERE X = 1 ORDER BY K FETCH FIRST 10 ROWS ONLY"),
        ("FETCH FIRST 10 of a join by its left key", "SELECT O.K, P.W FROM O, P WHERE O.K = P.K ORDER BY O.K FETCH FIRST 10 ROWS ONLY"),
        ("FETCH FIRST 10 groups by an index", "SELECT G, COUNT(*), MAX(K) FROM O GROUP BY G ORDER BY G FETCH FIRST 10 ROWS ONLY"),
        ("FETCH FIRST 10 groups (columnstore)", "SELECT G, COUNT(*), MAX(K) FROM OC GROUP BY G ORDER BY G FETCH FIRST 10 ROWS ONLY"),
        ("GROUP BY an indexed column", "SELECT G, COUNT(*), SUM(X) FROM O GROUP BY G"),
        ("GROUP BY the kept order (columnstore)", "SELECT G, COUNT(*), SUM(X) FROM OC GROUP BY G"),
        ("GROUP BY the kept order, ORDER BY it (columnstore)", "SELECT G, COUNT(*), SUM(X) FROM OC GROUP BY G ORDER BY G"),
        ("DISTINCT of the kept order (columnstore)", "SELECT DISTINCT G FROM OC"),
    ] {
        bench.measure(Case::new(name.to_owned(), "queries"), || {
            super::fetched(&s, sql);
            1
        });
    }
}

/// Schema B with O and P in the row store and OC in the column store, as
/// the module says, their statistics gathered.
fn database() -> Database {
    let db = Database::new();
    let columns = "K INTEGER NOT NULL, G INTEGER, X INTEGER, V VARCHAR(20)";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE O ({columns}, PRIMARY KEY (K)) \
         CREATE TABLE P (K INTEGER PRIMARY KEY, W INTEGER) \
         CREATE TABLE OC ({columns}) USING columnstore WITH (COLUMNSTORE.ORDER = 'G')"
    ))
    .unwrap();
    let s = db.session("B");
    s.run("CREATE INDEX O_G ON O (G)").unwrap();
    for first in (0..ROWS).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| format!("({k}, {}, {}, 'v{k}')", scatter(k, ROWS) % 2_000, if k % 10 == 0 { k } else { 1 }))
            .collect();
        for table in ["O", "OC"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
        let tenth: Vec<String> = (first..first + PER_INSERT).step_by(10).map(|k| format!("({k}, {})", k % 7)).collect();
        s.run(&format!("INSERT INTO P VALUES {}", tenth.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    for table in ["B.O", "B.P", "B.OC"] {
        db.analyze(table).unwrap();
    }
    db
}
