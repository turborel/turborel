//! Set functions and groups by batch over a column of the column store
//! kept as runs of one value (RULES.md D31, D38): the same results as the
//! row store's row by row.

use turborel::db::Database;
use turborel::threads::JobPolicy;

use super::{ROWS, off_alike};
use crate::common::{exec, rows, sorted};

/// Queries of a table `{t}` of runs: V of runs with no null, W of runs
/// with nulls among them.
const OF_RUNS: [&str; 13] = [
    "SELECT COUNT(*), SUM(V), AVG(V), MIN(V), MAX(V) FROM {t}",
    "SELECT COUNT(*), SUM(V), MIN(V), MAX(V) FROM {t} WHERE V >= 7",
    "SELECT COUNT(V), SUM(V) FROM {t} WHERE V < 3 OR V = 15",
    "SELECT COUNT(*), SUM(V) FROM {t} WHERE V IN (2, 4, 19)",
    "SELECT COUNT(*), SUM(V) FROM {t} WHERE V NOT IN (2, 4, 19)",
    "SELECT COUNT(*), MAX(V) FROM {t} WHERE V BETWEEN 5 AND 9 AND K > 100",
    "SELECT COUNT(W), SUM(W), MIN(W), MAX(W) FROM {t} WHERE W > 40",
    "SELECT COUNT(*), SUM(W) FROM {t} WHERE W IS NULL OR V = 1",
    "SELECT SUM(V) FROM {t} WHERE V > 100",
    "SELECT COUNT(*) FROM {t} WHERE V <> 3",
    "SELECT SUM(V * 2 - 1), SUM(W * 2), COUNT(*) FROM {t} WHERE V + 1 > 4 AND W - 1 < 90",
    "SELECT COUNT(*), SUM(V) FROM {t} WHERE V > W - 100 AND W < V * 10",
    "SELECT MIN(W + V), MAX(-W) FROM {t} WHERE W >= 3",
];

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.8 GR1, §5.11, §5.13]:
/// a column of the column store kept as runs of one value, its groups
/// sorted by it, judged and summed once a run, gives by batch what the
/// same rows of the row store give row by row: of a column of runs with
/// no null and of one with nulls, under comparisons, IN, NOT IN and
/// BETWEEN, of expressions of them, the two compared, and grouped by the
/// one with nulls, none matched, frozen and with rows changed, deleted and
/// added.
#[test]
fn columns_of_runs_answer_as_row_by_row() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER NOT NULL, V INTEGER NOT NULL, W INTEGER)
           CREATE TABLE C (K INTEGER NOT NULL, V INTEGER NOT NULL, W INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'V')",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        let values: Vec<String> = (0..ROWS)
            .map(|k| {
                let w = if k % 13 == 0 { "NULL".to_owned() } else { k.checked_div(50).unwrap().to_string() };
                format!("({k}, {}, {w})", k.checked_div(300).unwrap())
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let alike = |when: &str| {
        for q in OF_RUNS {
            assert_eq!(rows(&a, &q.replace("{t}", "C")), rows(&a, &q.replace("{t}", "R")), "{when}: {q}");
        }
        off_alike(&a, &OF_RUNS);
        let by_w = "SELECT W, COUNT(*), SUM(V) FROM {t} GROUP BY W";
        assert_eq!(
            sorted(&a, &by_w.replace("{t}", "C")),
            sorted(&a, &by_w.replace("{t}", "R")),
            "{when}: grouped by W, nulls among its runs"
        );
    };
    alike("frozen");
    for table in ["R", "C"] {
        a.run(&format!("UPDATE {table} SET V = 3 WHERE K BETWEEN 2100 AND 2200")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 4000 AND 4100")).unwrap();
        a.run(&format!("INSERT INTO {table} VALUES (90001, 7, NULL), (90002, 25, 1)")).unwrap();
    }
    a.commit().unwrap();
    alike("changed");
}

/// Verifies: [DESIGN] RULES.md D31, D38 [X3.135-1986 §5.22, §5.8]: grouped
/// by a column of the column store kept as runs, a run's rows each taken
/// into the group of its first, the groups and their set functions are
/// those of the row store read a row at a time: filtered or not, held in
/// the work area or past it, their rows written and grouped again.
#[test]
fn groups_of_runs_answer_as_row_by_row() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER NOT NULL, V INTEGER NOT NULL)
           CREATE TABLE C (K INTEGER NOT NULL, V INTEGER NOT NULL)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 8192, COLUMNSTORE.ORDER = 'V')",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..40_000_i64).step_by(1_000) {
            let values: Vec<String> = (first..first + 1_000).map(|k| format!("({k}, {})", k.checked_div(20).unwrap())).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let grouped = [
        "SELECT V, COUNT(*), SUM(K), MIN(K) FROM {t} GROUP BY V",
        "SELECT V, COUNT(*), MAX(K) FROM {t} WHERE K > 5005 AND K < 37013 GROUP BY V",
        "SELECT V, SUM(K) FROM {t} WHERE V BETWEEN 100 AND 900 GROUP BY V",
    ];
    let alike = |when: &str| {
        for q in grouped {
            exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            let by_rows = sorted(&a, &q.replace("{t}", "R"));
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            assert_eq!(sorted(&a, &q.replace("{t}", "C")), by_rows, "{when}: {q}");
        }
    };
    alike("held");
    assert_eq!(sorted(&a, "SELECT V FROM C GROUP BY V").len(), 2_000);
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    alike("past the work area");
}
