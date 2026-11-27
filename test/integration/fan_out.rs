//! \[DESIGN\] Joins whose keys each match many rows, run in parallel in a
//! small work area (RULES.md D1150, D1151): a morsel's rows, many of each
//! of its table's, outgrow the part's grant, so each such morsel is given
//! up and read again as it is given, by the stream, the sort, the window's
//! partitions and a join above it, of its keys or of an expression of its
//! columns; every answer is the
//! one the plan gives alone, in its order, on both stores, by batch and
//! row by row.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, sorted};

/// Schema A: F, of the column store, and R, of the row store, alike, of
/// `rows` rows of keys 0 to 2; B of `built` rows of the same keys, so each
/// row of F or R matches a third of B.
fn tables(rows: usize, built: usize) -> (Database, Session) {
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
    let rows: Vec<String> = (0..rows).map(|k| format!("({k}, {})", k % 3)).collect();
    for table in ["F", "R"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
    }
    let built: Vec<String> = (0..built).map(|t| format!("({}, {t})", t % 3)).collect();
    a.run(&format!("INSERT INTO B VALUES {}", built.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Queries of `{t}`, F or R, over its join with B: the join's rows, a
/// grouping of them, a sort, a window, a join of them, an EXCEPT.
const QUERIES: [&str; 6] = [
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G",
    "SELECT B.TAG, COUNT(*), SUM({t}.K) FROM {t} JOIN B ON {t}.G = B.G GROUP BY B.TAG",
    "SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G ORDER BY B.TAG DESC, {t}.K",
    "SELECT {t}.K, B.TAG, ROW_NUMBER() OVER (PARTITION BY B.TAG ORDER BY {t}.K DESC) FROM {t} JOIN B ON {t}.G = B.G",
    "SELECT COUNT(*), SUM(X.K) FROM R AS Q JOIN (SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G) AS X ON Q.K = X.TAG",
    "SELECT {t}.K + B.TAG FROM {t} JOIN B ON {t}.G = B.G EXCEPT SELECT K FROM R",
];

/// A join of R with the join of `{t}` and B on an expression of its
/// columns.
const ON_EXPRESSION: [&str; 1] =
    ["SELECT COUNT(*), SUM(X.K) FROM R AS Q JOIN (SELECT {t}.K, B.TAG FROM {t} JOIN B ON {t}.G = B.G) AS X ON Q.K = X.TAG + X.K"];

/// The answers of `queries` of F and R, each with its query: of those
/// that end in an ORDER BY of their own, in its order. A window's ORDER
/// BY orders no result (RULES.md D1245), so such a query's rows are
/// compared sorted.
fn answers(a: &Session, queries: &[&str]) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for query in queries {
        for table in ["F", "R"] {
            let sql = query.replace("{t}", table);
            let ordered = sql.rsplit_once("ORDER BY").is_some_and(|(_, tail)| !tail.contains(')'));
            let rows = if ordered { rows(a, &sql).unwrap() } else { sorted(a, &sql) };
            out.push((sql, rows));
        }
    }
    out
}

/// Of `queries` over `tables`, the answers at degree 4 in morsels of 256
/// rows in a work area of `bytes` are those at degree 1 in one that holds
/// them whole, by batch and row by row; a difference names its query, its
/// table's store, the degree and the work area, and its first row apart.
fn answer_as_alone(a: &Session, queries: &[&str], bytes: usize) {
    exec(a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let mut alone = Vec::new();
    for batches in ["ON", "OFF"] {
        exec(a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        alone.push((batches, answers(a, queries)));
    }
    let work = format!("SET MEMORY.WORK_BYTES = {bytes}");
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0", work.as_str()] {
        exec(a, set).unwrap();
    }
    for (batches, want) in alone {
        exec(a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for ((sql, got), (_, rows)) in answers(a, queries).into_iter().zip(want) {
            let store = if sql.contains(" F ") || sql.contains("F.") { "column store" } else { "row store" };
            let apart = got.iter().zip(&rows).position(|(g, r)| g != r).unwrap_or(got.len().min(rows.len()));
            assert!(
                got == rows,
                "{sql}: {store}, batches {batches}, degree 4, MEMORY.WORK_BYTES {bytes}: {} rows, not {}, apart from row {apart}: {:?}, not {:?}",
                got.len(),
                rows.len(),
                got.get(apart),
                rows.get(apart)
            );
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1150, D1151, D38 (R-TEST-1): a join
/// whose every row matches a hundred, run at degree 4 in morsels of 256
/// rows in a work area of 256 KiB, each morsel's 25,600 rows far past its
/// grant, gives what it gives alone at degree 1 in a work area that holds
/// it whole, by batch and row by row: the stream, its grouping, its sort
/// in order, its window, a join of it and its EXCEPT. Before, each
/// morsel's rows were held whole outside the grant.
#[test]
fn a_join_past_its_grant_answers_as_alone() {
    let (_db, a) = tables(1_200, 300);
    answer_as_alone(&a, &QUERIES, 262_144);
}

/// Verifies: [DESIGN] RULES.md D1150, D38 (R-TEST-1): a join on an
/// expression of the columns of a join whose every row matches twenty,
/// its morsels of 256 rows past a work area of 64 KiB, gives what it
/// gives alone, by batch and row by row.
#[test]
fn a_join_on_an_expression_past_its_grant_answers_as_alone() {
    let (_db, a) = tables(120, 60);
    answer_as_alone(&a, &ON_EXPRESSION, 65_536);
}
