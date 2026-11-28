//! \[DESIGN\] (RULES.md D1325) A grouping run in parallel whose rows of
//! one key come one after another, each run a key its morsel has not
//! held, hands each run on as one entry of all its rows. Its groups,
//! their order and each set function's value are those the plan run
//! alone, a row at a time, gives.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::alike_runs::three_ways;
use crate::common::{exec, rows};

/// Rows of each table.
const ROWS: i64 = 6_000;

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike, their rows in the order of K: B a value of its own
/// every five rows; G in runs of 40 rows, null in every 11th run; U in
/// runs of 2, null in every 7th row's run and in every fifth stretch of
/// five rows, so whole runs of B hold no U; S in runs of 3, each second
/// run the text of the run before it with a trailing space; D, a date,
/// in runs of 7; and F, half of K.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "K INTEGER PRIMARY KEY, B INTEGER, G INTEGER, U INTEGER, S VARCHAR(8), D DATE, F DOUBLE PRECISION";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R ({columns})"
    ))
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: String| if null { "NULL".to_owned() } else { v };
    for table in ["C", "R"] {
        let values: Vec<String> = (0..ROWS)
            .map(|k| {
                let (b, g, u, s) = (k.div_euclid(5), k.div_euclid(40), k.div_euclid(2), k.div_euclid(3));
                let text = format!("'s{}{}'", s.div_euclid(2) % 50, if s % 2 == 1 { " " } else { "" });
                let day = format!("DATE '2020-01-{:02}'", 1 + k.div_euclid(7) % 28);
                let (g, u) = (null_or(g % 11 == 5, (g % 37).to_string()), null_or(u % 7 == 3 || b % 5 == 2, (u % 97).to_string()));
                format!("({k}, {b}, {g}, {u}, {text}, {day}, {k}.5E0)")
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Verifies: [X3.135-1986 §5.22 GR2] [ISO/IEC 9075-2:2023 §7.13 GR2,
/// §10.9 GR4, GR7] (RULES.md D1325) a grouping whose rows come in runs
/// of one key, each run a key of its own, gives, by batch on one thread
/// and in parallel in morsels that hand their runs on, the groups, their
/// order and the values the plan gives a row at a time. Counts of rows,
/// sums, means, extremes and counts of a column with nulls, whole runs
/// of them among them; sums and means of approximate numbers and of an
/// expression; a count of distinct values beside other set functions,
/// alone, and of no grouping; a key of an integer and a text whose runs
/// end apart, under a WHERE clause that takes rows out of the middle of
/// runs; and under HAVING. Of the column store and of the row store.
#[test]
fn groups_of_runs_handed_on_are_the_plans() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT B, G, COUNT(*), SUM(K), MIN(K), MAX(U), AVG(U), COUNT(U), SUM(U) FROM {t} GROUP BY B, G",
            "SELECT B, COUNT(*), MIN(U), MAX(D), SUM(F), AVG(F), SUM(K + U) FROM {t} GROUP BY B",
            "SELECT G, COUNT(DISTINCT B), COUNT(*), SUM(K), AVG(U) FROM {t} GROUP BY G",
            "SELECT G, COUNT(DISTINCT B) FROM {t} GROUP BY G",
            "SELECT COUNT(DISTINCT B) FROM {t}",
            "SELECT B, CHAR_LENGTH(S), COUNT(*), MIN(K), SUM(U) FROM {t} WHERE MOD(K, 3) <> 1 GROUP BY B, S",
            "SELECT B, G, D, COUNT(*), SUM(U) FROM {t} GROUP BY B, G, D HAVING COUNT(*) > 2",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, by_batch, together) = three_ways(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, by_batch, "{sql} by batch");
            assert_eq!(alone, together, "{sql} in parallel");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1325) [X3.135-1986 §5.25 GR1] the
/// values themselves, not only their agreement, in parallel in morsels
/// that hand their runs on: each of the 1,200 runs of B is a group of
/// its five rows, its sum of K the five numbers' and its least the
/// first; a run all of nulls of U counts no U and has no sum of it; and
/// every row is counted once.
#[test]
fn every_row_of_a_run_handed_on_is_taken_once() {
    let (_db, a) = tables();
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(&a, set).unwrap();
    }
    let one = |sql: &str| rows(&a, sql).unwrap();
    let by_run = "(SELECT B, G, COUNT(*) AS N, SUM(K) AS T, MIN(K) AS L, COUNT(U) AS C, SUM(U) AS V FROM C GROUP BY B, G) AS Q";
    assert_eq!(one(&format!("SELECT COUNT(*), SUM(N) FROM {by_run}")), ["1200, 6000"]);
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_run} WHERE N = 5 AND L = B * 5 AND T = 5 * L + 10")), ["1200"]);
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_run} WHERE C = 0 AND V IS NULL")), ["240"]);
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_run} WHERE C > 0 AND V IS NOT NULL")), ["960"]);
}
