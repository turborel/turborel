//! \[DESIGN\] A grouping computed alike by a filter's input and by a
//! subquery of its condition is computed once (RULES.md D1129): the
//! statement gives the rows it gives with each computed apart, of either
//! engine, by batch and a row at a time, alone and in parallel.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// Tables R, of the row store, and C, of the column store: K a key, G one
/// of 40 groups, null for every 17th row, N a number, null for every
/// 13th; E alike and empty; and views of their groups' totals.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let columns = "K INTEGER PRIMARY KEY, G INTEGER, N INTEGER";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R ({columns}) CREATE TABLE C ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE E ({columns})
           CREATE VIEW VR (G, TOTAL, ROWS_OF) AS SELECT G, SUM(N), COUNT(*) FROM R WHERE K >= 100 GROUP BY G
           CREATE VIEW VC (G, TOTAL, ROWS_OF) AS SELECT G, SUM(N), COUNT(*) FROM C WHERE K >= 100 GROUP BY G
           CREATE VIEW VE (G, TOTAL, ROWS_OF) AS SELECT G, SUM(N), COUNT(*) FROM E WHERE K >= 100 GROUP BY G"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..6000_i64).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| {
                    let g = if k % 17 == 0 { "NULL".to_owned() } else { (k % 40).to_string() };
                    let n = if k % 13 == 0 { "NULL".to_owned() } else { (k * 7919 % 1000).to_string() };
                    format!("({k}, {g}, {n})")
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET N = N + 1 WHERE K BETWEEN 200 AND 260")).unwrap();
        a.commit().unwrap();
    }
    (db, a)
}

/// The rows of `sql`, sorted, or its error.
fn sorted(a: &Session, sql: &str) -> Result<Vec<String>, Code> {
    rows(a, sql).map(|mut r| {
        r.sort();
        r
    })
}

/// Queries of a view `{v}` of table `{t}`, each paired with one that
/// gives the same rows and holds no grouping twice.
const PAIRS: [(&str, &str); 6] = [
    // The view in the statement and in a subquery of its filter.
    (
        "SELECT G, TOTAL FROM {v} WHERE TOTAL = (SELECT MAX(TOTAL) FROM {v})",
        "SELECT G, TOTAL FROM {v} WHERE TOTAL >= ALL (SELECT SUM(N) FROM {t} WHERE K >= 100 AND N IS NOT NULL GROUP BY G)",
    ),
    // In a join under the filter, as a supplier's of its revenue.
    (
        "SELECT T.K, V.TOTAL FROM {t} T, {v} V WHERE T.K = V.ROWS_OF AND V.TOTAL = (SELECT MAX(TOTAL) FROM {v})",
        "SELECT T.K, V.TOTAL FROM {t} T, {v} V WHERE T.K = V.ROWS_OF AND V.TOTAL >= ALL (SELECT SUM(N) FROM {t} WHERE K >= 100 AND N IS NOT NULL GROUP BY G)",
    ),
    // The group of null among them, and a subquery of the rows counted.
    (
        "SELECT G, ROWS_OF FROM {v} WHERE ROWS_OF > (SELECT AVG(ROWS_OF) FROM {v})",
        "SELECT G, ROWS_OF FROM {v} WHERE ROWS_OF * (SELECT COUNT(*) FROM (SELECT DISTINCT G FROM {t} WHERE K >= 100) AS D) > (SELECT COUNT(*) FROM {t} WHERE K >= 100)",
    ),
    // HAVING, the grouping written out in both places.
    (
        "SELECT G, SUM(N) FROM {t} WHERE K >= 100 GROUP BY G HAVING SUM(N) > (SELECT AVG(S) FROM (SELECT G, SUM(N) AS S FROM {t} WHERE K >= 100 GROUP BY G) AS Q)",
        "SELECT G, SUM(N) FROM {t} WHERE K >= 100 GROUP BY G HAVING SUM(N) * (SELECT COUNT(*) FROM (SELECT DISTINCT G FROM {t} WHERE K >= 100) AS D) > (SELECT SUM(N) FROM {t} WHERE K >= 100)",
    ),
    // The same grouping under another filter is another grouping.
    (
        "SELECT G, TOTAL FROM {v} WHERE TOTAL > (SELECT MAX(S) FROM (SELECT G, SUM(N) AS S FROM {t} WHERE K >= 3000 GROUP BY G) AS Q)",
        "SELECT G, TOTAL FROM {v} WHERE TOTAL > ALL (SELECT SUM(N) FROM {t} WHERE K >= 3000 AND N IS NOT NULL GROUP BY G)",
    ),
    // Two subqueries of the view, and the view.
    (
        "SELECT G FROM {v} WHERE TOTAL < (SELECT MAX(TOTAL) FROM {v}) AND TOTAL > (SELECT MIN(TOTAL) FROM {v})",
        "SELECT G FROM {v} WHERE TOTAL < ANY (SELECT SUM(N) FROM {t} WHERE K >= 100 GROUP BY G) AND TOTAL > ANY (SELECT SUM(N) FROM {t} WHERE K >= 100 GROUP BY G)",
    ),
];

/// Verifies: [DESIGN] RULES.md D1129 [ISO/IEC 9075:1992 §7.11 GR1; X3.135-1986
/// §5.21 GR2, §5.23]: a view read in a statement and in a subquery of its
/// filter, or of its HAVING condition, gives the rows of a statement that
/// computes nothing twice, of either engine, by batch and a row at a
/// time, alone and in parallel: the groups of a null grouping column and
/// of null totals among them, under a join, with two subqueries, and
/// where the subquery's grouping is under another filter.
#[test]
fn a_view_read_again_in_a_subquery_gives_the_rows_of_each_computed_apart() {
    let (_db, a) = tables();
    for (table, view) in [("R", "VR"), ("C", "VC")] {
        for (shared, apart) in PAIRS {
            let (shared, apart) = (shared.replace("{t}", table).replace("{v}", view), apart.replace("{t}", table).replace("{v}", view));
            let wanted = sorted(&a, &apart);
            assert!(wanted.as_ref().is_ok_and(|r| !r.is_empty()), "{apart}: {wanted:?}");
            for sets in [
                ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.PARALLEL_MIN_ROWS = 100000"],
                ["SET EXECUTOR.BATCHES = 'OFF'", "SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.PARALLEL_MIN_ROWS = 100000"],
                ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
                ["SET EXECUTOR.BATCHES = 'OFF'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"],
            ] {
                for set in sets {
                    exec(&a, set).unwrap();
                }
                assert_eq!(sorted(&a, &shared), wanted, "{shared} under {sets:?}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1129, D15: of an empty table the view
/// has no group, the filter judges no row, and the statement gives none;
/// a view read twice sees its transaction's own writes in both places;
/// and a grouping that raises an error raises it, read once or twice.
#[test]
fn a_shared_grouping_of_no_rows_of_new_rows_and_of_an_error() {
    let (_db, a) = tables();
    assert_eq!(rows(&a, "SELECT G, TOTAL FROM VE WHERE TOTAL = (SELECT MAX(TOTAL) FROM VE)"), Ok(Vec::new()));
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM VE WHERE ROWS_OF >= (SELECT COUNT(*) FROM VE)"), Ok(vec!["0".to_owned()]));
    let top = "SELECT G, TOTAL FROM VC WHERE TOTAL = (SELECT MAX(TOTAL) FROM VC)";
    let before = rows(&a, top).unwrap();
    a.run("INSERT INTO C VALUES (9000, 77, 100000000)").unwrap();
    assert_eq!(rows(&a, top), Ok(vec!["77,100000000".to_owned()]), "its own write is in the view and in the subquery");
    a.rollback().unwrap();
    assert_eq!(rows(&a, top).unwrap(), before);
    let raising = "SELECT G, S FROM (SELECT G, SUM(N / (G - G)) AS S FROM C GROUP BY G) AS Q
                   WHERE S = (SELECT MAX(S) FROM (SELECT G, SUM(N / (G - G)) AS S FROM C GROUP BY G) AS P)";
    assert_eq!(rows(&a, raising), Err(Code::Data), "the grouping divides by 0");
}
