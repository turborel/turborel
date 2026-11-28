//! \[DESIGN\] (RULES.md D1323) A grouping whose rows of one key come one
//! after another looks up the first row of each run alone. Its groups,
//! their order and each set function's value are those the plan run
//! alone, a row at a time, gives.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Rows of each table.
const ROWS: i64 = 6_000;

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike, their rows in the order of K: G in runs of 40 rows, null
/// in every 11th run; U in runs of 5, null in every 13th; S in runs of
/// 3, each second run the text of the run before it with a trailing
/// space, null in every 17th; D, a date, in runs of 7.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, U INTEGER, S VARCHAR(8), D DATE)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, U INTEGER, S VARCHAR(8), D DATE)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: String| if null { "NULL".to_owned() } else { v };
    for table in ["C", "R"] {
        let values: Vec<String> = (0..ROWS)
            .map(|k| {
                let (g, u, s) = (k.div_euclid(40), k.div_euclid(5), k.div_euclid(3));
                let text = format!("'s{}{}'", s.div_euclid(2) % 50, if s % 2 == 1 { " " } else { "" });
                let day = format!("DATE '2020-01-{:02}'", 1 + k.div_euclid(7) % 28);
                format!(
                    "({k}, {}, {}, {}, {day})",
                    null_or(g % 11 == 5, (g % 37).to_string()),
                    null_or(u % 13 == 3, (u % 97).to_string()),
                    null_or(s % 17 == 9, text)
                )
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run a row at a time on one thread, by batch on one
/// thread, and by batch on 4 threads in morsels of 256 rows.
fn three_ways(s: &Session, sql: &str) -> (Rows, Rows, Rows) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let by_batch = rows(s, sql);
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
    (alone, by_batch, rows(s, sql))
}

/// Verifies: [X3.135-1986 §5.22 GR2] [ISO/IEC 9075-2:2023 §7.13 GR2,
/// §10.9 GR4, GR7] (RULES.md D1323) a grouping whose rows of one key
/// come in runs gives, by batch on one thread and in parallel, the
/// groups, their order and the counts, sums and counts of distinct
/// values the plan gives a row at a time: by two columns of integers
/// whose runs end apart, nulls in both; a count of distinct values
/// beside other set functions, and alone, and under HAVING; by a text
/// whose runs differ from the run before them by a trailing space
/// alone, one group as padded strings compare; by a date and an integer
/// under a WHERE clause that takes rows out of the middle of runs; by a
/// column whose kept rows are nulls and the values after them; and by
/// three columns. Of the column store and of the row store.
#[test]
fn groups_of_rows_in_runs_are_the_plans() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, U, COUNT(*), SUM(K), MIN(K) FROM {t} GROUP BY G, U",
            "SELECT G, COUNT(DISTINCT U), COUNT(*), SUM(K), AVG(U) FROM {t} GROUP BY G",
            "SELECT G, COUNT(DISTINCT U) FROM {t} GROUP BY G",
            "SELECT G, COUNT(DISTINCT U), COUNT(DISTINCT D) FROM {t} GROUP BY G HAVING COUNT(DISTINCT U) > 7",
            "SELECT CHAR_LENGTH(S), COUNT(*), MIN(K), MAX(K) FROM {t} GROUP BY S",
            "SELECT S, COUNT(DISTINCT U) FROM {t} WHERE S <> '' GROUP BY S",
            "SELECT D, G, COUNT(*), SUM(U) FROM {t} WHERE MOD(K, 3) <> 1 GROUP BY D, G",
            "SELECT U, COUNT(*), SUM(K) FROM {t} WHERE U IS NULL OR U > 50 GROUP BY U",
            "SELECT G, U, S, COUNT(*) FROM {t} GROUP BY G, U, S HAVING COUNT(*) > 1",
            "SELECT COUNT(DISTINCT U), COUNT(DISTINCT S) FROM {t}",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, by_batch, together) = three_ways(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, by_batch, "{sql} by batch");
            assert_eq!(alone, together, "{sql} in parallel");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1323) [X3.135-1986 §5.25 GR1] the
/// counts themselves, not only their agreement: 150 runs of G of 40 rows
/// each fall in 38 groups, the null group among them, and each run of U
/// of 5 rows counts 5 whichever group of G it falls in.
#[test]
fn rows_of_a_run_are_all_counted() {
    let (_db, a) = tables();
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(&a, set).unwrap();
    }
    let one = |sql: &str| rows(&a, sql).unwrap();
    let by_g = "(SELECT G, COUNT(*) AS N FROM C GROUP BY G) AS T";
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_g}")), ["38"]);
    assert_eq!(one(&format!("SELECT SUM(N) FROM {by_g}")), ["6000"]);
    assert_eq!(one(&format!("SELECT N FROM {by_g} WHERE G IS NULL")), ["560"]);
    let by_run = "(SELECT K - MOD(K, 5) AS B, G, U, COUNT(*) AS N FROM C GROUP BY K - MOD(K, 5), G, U) AS T";
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_run}")), ["1200"]);
    assert_eq!(one(&format!("SELECT COUNT(*) FROM {by_run} WHERE N = 5")), ["1200"]);
    let by_pair = "(SELECT G, U, COUNT(*) AS N FROM C GROUP BY G, U) AS T";
    assert_eq!(one(&format!("SELECT SUM(N) FROM {by_pair}")), ["6000"]);
}
