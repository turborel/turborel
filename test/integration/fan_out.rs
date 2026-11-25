//! \[DESIGN\] Joins whose keys each match many rows, run in parallel in a
//! small work area (RULES.md D1150, D1151): a morsel's rows, fifty of each
//! of its table's, outgrow the part's grant, so each such morsel is
//! given up and read again as it is given, by the stream, the sort, the
//! window's partitions and a join's build above it; every answer is the
//! one the plan gives alone, in its order, on both stores, by batch and
//! row by row.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, sorted};

/// Schema A: F, of the column store, and R, of the row store, alike, of
/// 600 rows of keys 0 to 2; B of 150 rows of the same keys, so each row
/// of F or R matches 50 of B.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER) USING columnstore
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER)
           CREATE TABLE B (G INTEGER, TAG INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let rows: Vec<String> = (0..1_200).map(|k| format!("({k}, {})", k % 3)).collect();
    for table in ["F", "R"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
    }
    let built: Vec<String> = (0..300).map(|t| format!("({}, {t})", t % 3)).collect();
    a.run(&format!("INSERT INTO B VALUES {}", built.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Queries of `{t}`, F or R, over its join with B: the join's rows, a
/// grouping of them, a sort, a window, a join built of them, an EXCEPT.
const QUERIES: [&str; 6] = [
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G",
    "SELECT B.TAG, COUNT(*), SUM({t}.K) FROM {t} JOIN B ON {t}.G = B.G GROUP BY B.TAG",
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G ORDER BY B.TAG DESC, {t}.K",
    "SELECT {t}.K, B.TAG, ROW_NUMBER() OVER (PARTITION BY B.TAG ORDER BY {t}.K DESC) FROM {t} JOIN B ON {t}.G = B.G",
    "SELECT COUNT(*), SUM(X.K) FROM R AS Q JOIN (SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G) AS X ON Q.K = X.TAG",
    "SELECT {t}.K + B.TAG FROM {t} JOIN B ON {t}.G = B.G EXCEPT SELECT K FROM R",
];

/// The answers of `QUERIES` of F and R: of those of an order, in it.
fn answers(a: &Session) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for query in QUERIES {
        for table in ["F", "R"] {
            let sql = query.replace("{t}", table);
            let rows = if sql.contains("ORDER BY") { rows(a, &sql).unwrap() } else { sorted(a, &sql) };
            out.push((sql, rows));
        }
    }
    out
}

/// Verifies: [DESIGN] RULES.md D1150, D1151, D38 (R-TEST-1): a join
/// whose every row matches fifty, run at degree 4 in morsels of 256 rows
/// in a work area of 256 KiB, each morsel's 12,800 rows far past its
/// grant, gives what it gives alone at degree 1 in a work area that holds
/// it whole, by batch and row by row: the stream, its grouping, its sort
/// in order, its window, the join built of it and its EXCEPT. Before,
/// each morsel's rows were held whole outside the grant.
#[test]
fn a_join_past_its_grant_answers_as_alone() {
    let (_db, a) = tables();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let mut alone = Vec::new();
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        alone.push((batches, answers(&a)));
    }
    for set in [
        "SET EXECUTOR.DEGREE = 4",
        "SET EXECUTOR.MORSEL_ROWS = 256",
        "SET EXECUTOR.PARALLEL_MIN_ROWS = 0",
        "SET MEMORY.WORK_BYTES = 262144",
    ] {
        exec(&a, set).unwrap();
    }
    for (batches, want) in alone {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for ((sql, got), (_, rows)) in answers(&a).into_iter().zip(want) {
            assert!(got == rows, "{sql}, batches {batches}: {} rows, not {}", got.len(), rows.len());
        }
    }
}
