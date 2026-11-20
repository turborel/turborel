//! \[DESIGN\] Batches of datetimes and truth values, filters of an OR of
//! comparisons, and morsels read by batch (RULES.md D354): each query gives
//! by batches what it gives a row at a time, of either engine, frozen rows
//! changed among them; a raising filter raises the error a row at a time
//! raises, after the rows before it; and a part run in parallel by batch
//! gives the rows it gives alone, in its order.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{both_ways, exec, rows};

/// The rows written to each table.
const ROWS: i64 = 6000;

/// The values of row `k`: K, D a date, T a timestamp, B a truth value, null
/// every third, F an approximate number, N an integer, null every 13th.
fn row(k: i64) -> String {
    let date = format!("DATE '{}-{:02}-{:02}'", 2000 + k % 20, 1 + k % 12, 1 + k % 28);
    let time = format!("TIMESTAMP '2021-03-04 {:02}:{:02}:{:02}'", k % 24, k % 60, (k * 7) % 60);
    let truth = ["TRUE", "FALSE", "NULL"][usize::try_from(k % 3).unwrap()];
    let n = if k % 13 == 0 { "NULL".to_owned() } else { (k * 7919 % 1000).to_string() };
    format!("({k}, {date}, {time}, {truth}, {}E0, {n})", k % 50)
}

/// Tables R, of the row store, and C, of the column store, alike: written,
/// frozen, then changed.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, D DATE, T TIMESTAMP, B BOOLEAN, F DOUBLE PRECISION, N INTEGER)
           CREATE TABLE C (K INTEGER PRIMARY KEY, D DATE, T TIMESTAMP, B BOOLEAN, F DOUBLE PRECISION, N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(row).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET B = NOT B, D = DATE '1999-12-31' WHERE K BETWEEN 100 AND 150")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100")).unwrap();
        a.commit().unwrap();
    }
    (db, a)
}

/// Filters of datetimes, truth values and ORs of comparisons.
const FILTERS: [&str; 14] = [
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE D >= DATE '2010-01-01'",
    "SELECT COUNT(*) FROM {t} WHERE D = DATE '1999-12-31'",
    "SELECT COUNT(*), MIN(K) FROM {t} WHERE D < DATE '2005-06-15' AND N > 100",
    "SELECT COUNT(*) FROM {t} WHERE T > TIMESTAMP '2021-03-04 12:00:00'",
    "SELECT COUNT(*) FROM {t} WHERE T <> TIMESTAMP '2021-03-04 00:00:00'",
    "SELECT COUNT(*) FROM {t} WHERE B = TRUE",
    "SELECT COUNT(*) FROM {t} WHERE B <> TRUE",
    "SELECT COUNT(*) FROM {t} WHERE B IS NULL",
    "SELECT COUNT(*) FROM {t} WHERE B > FALSE OR N < 10",
    "SELECT COUNT(*), SUM(K) FROM {t} WHERE N < 100 OR N > 900",
    "SELECT COUNT(*) FROM {t} WHERE F = 7E0 OR F >= 45E0 OR D IS NULL",
    "SELECT COUNT(*) FROM {t} WHERE (N = 1 OR K < 20) AND D > DATE '2001-01-01'",
    "SELECT K, D, T, B FROM {t} WHERE K < 40 AND (B = FALSE OR T < TIMESTAMP '2021-03-04 03:00:00')",
    "SELECT D, COUNT(*) FROM {t} WHERE B = TRUE GROUP BY D",
];

/// Verifies: [DESIGN] RULES.md D354 [ISO/IEC 9075:1992 §8.2 GR5,
/// ISO/IEC 9075-2:2023 §4.6.1, X3.135-1986 §5.20 GR4] comparisons of
/// datetimes and of truth values, IS NULL of them, and ORs of comparisons,
/// judged over a batch's vectors, keep the rows a row at a time keeps, of
/// either engine.
#[test]
fn datetimes_truth_values_and_ors_filter_as_rows_do() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in FILTERS {
            let sql = query.replace("{t}", table);
            let (batched, by_rows) = both_ways(&a, &sql);
            assert!(!by_rows.is_empty(), "{sql}");
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D354, D15 a filter one of whose conjuncts
/// may raise an error is judged by masks ANDed, so the error a row raises
/// is the one a row at a time raises, with the rows before it.
#[test]
fn raising_filters_raise_as_rows_do() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in [
            "SELECT K FROM {t} WHERE K > 4000 AND 10 / (N - 500) > 0",
            "SELECT COUNT(*) FROM {t} WHERE D > DATE '2001-01-01' AND 10 / (K - 4000) > 0",
            "SELECT COUNT(*) FROM {t} WHERE 10 / (K - 7000) > 0 OR B = TRUE",
        ] {
            let sql = query.replace("{t}", table);
            let batched = rows(&a, &sql);
            exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            let by_rows = rows(&a, &sql);
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            assert_eq!(batched, by_rows, "{sql}");
        }
    }
}

/// Queries a part of which is run in parallel, its morsels read by batch.
const PARALLEL: [&str; 7] = [
    "SELECT F, COUNT(*), SUM(K), MIN(N) FROM {t} GROUP BY F",
    "SELECT DISTINCT F FROM {t}",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE D >= DATE '2010-01-01'",
    "SELECT B, COUNT(*) FROM {t} WHERE N < 300 OR N > 800 GROUP BY B",
    "SELECT K, N FROM {t} WHERE B = TRUE AND N > 900",
    "SELECT K, N + 1 FROM {t} WHERE T < TIMESTAMP '2021-03-04 02:00:00'",
    "SELECT D FROM {t} WHERE K < 500 UNION SELECT D FROM {t} WHERE K > 5500",
];

/// Verifies: [DESIGN] RULES.md D354, D290 a part run in parallel whose
/// morsels are read by batch, filtered and grouped over their vectors,
/// gives the rows the plan run alone a row at a time gives, in its order;
/// of the column store, kept serial by batch, the same rows.
#[test]
fn morsels_read_by_batch_give_the_rows_alone_gives() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in PARALLEL {
            let sql = query.replace("{t}", table);
            for set in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"] {
                exec(&a, set).unwrap();
            }
            let alone = rows(&a, &sql);
            for set in [
                "SET EXECUTOR.BATCHES = 'ON'",
                "SET EXECUTOR.DEGREE = 4",
                "SET EXECUTOR.MORSEL_ROWS = 256",
                "SET EXECUTOR.PARALLEL_MIN_ROWS = 0",
            ] {
                exec(&a, set).unwrap();
            }
            let plan = a.physical_plan(&sql, &Params::new()).unwrap();
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            if table == "R" {
                assert!(plan.contains("Parallel  (degree=4)"), "{sql}: {plan}");
                assert_eq!(alone, rows(&a, &sql), "{sql}");
            } else {
                // Kept serial, by batch, its rows in no order (RULES.md D31).
                let sorted = |r: Result<Vec<String>, _>| {
                    r.map(|mut r| {
                        r.sort();
                        r
                    })
                };
                assert_eq!(sorted(alone), sorted(rows(&a, &sql)), "{sql}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D355 the planner keeps serial a grouping or
/// DISTINCT whose groups, as the statistics estimate them, are so many
/// that each morsel grouped apart barely shrinks; of few groups, or of
/// groups it has no statistics of, it runs the part in parallel; and each
/// gives the same rows either way.
#[test]
fn groupings_of_many_groups_stay_serial() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE H (K INTEGER, G INTEGER, U INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0..40_000).step_by(1000) {
        let values: Vec<String> = (first..first + 1000).map(|k| format!("({k}, {}, {})", k % 10, k * 7919 % 40_000)).collect();
        a.run(&format!("INSERT INTO H VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&a, set).unwrap();
    }
    let marked = |sql: &str| a.physical_plan(sql, &Params::new()).unwrap().contains("Parallel");
    let (many, few) = ("SELECT U, COUNT(*) FROM H GROUP BY U", "SELECT G, COUNT(*) FROM H GROUP BY G");
    let (distinct_many, distinct_few) = ("SELECT DISTINCT U FROM H", "SELECT DISTINCT G FROM H");
    assert!(marked(many) && marked(distinct_many), "no statistics: no estimate of the groups");
    db.analyze("A.H").unwrap();
    assert!(!marked(many) && !marked(distinct_many), "40,000 groups of morsels of 256 rows");
    assert!(marked(few) && marked(distinct_few), "10 groups");
    let (batched, by_rows) = both_ways(&a, many);
    assert_eq!(batched, by_rows);
}
