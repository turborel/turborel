//! \[DESIGN\] (RULES.md D1324) A grouping with a DISTINCT set function
//! beside set functions without, made by two groupings run as one part,
//! whose plain functions are kept by the grouping's own columns alone:
//! the part gives the groups the plan run alone gives, in its order, by
//! batch and by rows, of either engine, under HAVING, a filter and a
//! sort's first rows, and past the grant.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 50 in runs of 20 rows, null in
/// every 9th run; H, one of 7; N, one of 23, null for every 13th row.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..9000)
            .map(|k| {
                format!(
                    "({k}, {}, {}, {})",
                    null_or(k.div_euclid(20) % 9 == 0, k.div_euclid(20) % 50),
                    k % 7,
                    null_or(k % 13 == 0, k * 5 % 23)
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

/// The rows of `sql` run alone, a row at a time on one thread, and run
/// in parallel on 4 threads in morsels of 256 rows by batch, with its
/// plan then; the rows run in parallel by rows are those by batch.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    for set in ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"] {
        exec(s, set).unwrap();
    }
    let alone = rows(s, sql);
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let together = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!(rows(s, sql), together, "{sql}, its morsels read by rows");
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (alone, together, shown)
}

/// Groupings with one DISTINCT set function beside counts and exact
/// totals: by a column in runs with nulls, by two columns, under HAVING
/// of both kinds of function, of the rows a filter leaves, of an
/// expression, and of values all null in some groups.
const PAIRS: [&str; 7] = [
    "SELECT G, COUNT(DISTINCT N), SUM(N), COUNT(*), AVG(K), MAX(K), COUNT(N) FROM {t} GROUP BY G",
    "SELECT H, G, COUNT(*), SUM(K), COUNT(DISTINCT N) FROM {t} GROUP BY H, G",
    "SELECT G, COUNT(DISTINCT N), SUM(K) FROM {t} GROUP BY G HAVING SUM(K) > 100000 AND COUNT(DISTINCT N) > 20",
    "SELECT H, MIN(N), COUNT(DISTINCT G), COUNT(*) FROM {t} WHERE K < 5000 AND N <> 4 GROUP BY H",
    "SELECT G, SUM(K * 2 - N), SUM(DISTINCT N), AVG(N) FROM {t} GROUP BY G",
    "SELECT N, COUNT(DISTINCT G), SUM(CASE WHEN H = 9 THEN K END), COUNT(*) FROM {t} GROUP BY N",
    "SELECT K2, COUNT(DISTINCT N), COUNT(*), SUM(K) FROM (SELECT K - MOD(K, 3) AS K2, N, K FROM {t}) AS Q GROUP BY K2",
];

/// Verifies: [DESIGN] RULES.md D1324, D1280 [ISO/IEC 9075-2:2023 §10.9
/// GR7b i]: a grouping with a DISTINCT set function beside counts, sums,
/// averages and extremes of integers, run as one part of two groupings,
/// gives the groups it gives run alone, in their order, read by batch
/// and by rows, of either engine: by a column with a null group, by two
/// columns, under HAVING, of filtered rows, of sums whose values are all
/// null, and of more groups than a morsel has rows.
#[test]
fn plain_functions_beside_a_distinct_one_give_the_plans_groups() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in PAIRS {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            // The first runs as one part of two groupings, whatever the planner makes of the others.
            assert!(shape != PAIRS[0] || shown.contains("Parallel"), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| r.len() > 5), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// The plan of `sql` run and analyzed on 4 threads by batch.
fn analyzed(s: &Session, sql: &str) -> String {
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    rows(s, &format!("EXPLAIN ANALYZE {sql}")).unwrap_or_else(|e| panic!("{sql}: {e:?}")).join("\n")
}

/// Verifies: [EXTENSION] RULES.md D1324, D620: the analyzed plan of a
/// grouping whose plain set functions are kept by its own columns says
/// so on its first grouping, with the groups its first morsel made by
/// them; one whose DISTINCT argument is fixed by the grouping column, so
/// that its first morsel makes a group of the first grouping for each of
/// those, says nothing of a side, and gives the plan's rows.
#[test]
fn the_analyzed_plan_says_whether_a_side_was_kept() {
    let (_db, a) = tables();
    let kept = analyzed(&a, &PAIRS[0].replace("{t}", "C"));
    assert!(kept.contains("side groups="), "{kept}");
    let fixed = "SELECT G, COUNT(DISTINCT H2), COUNT(*), SUM(K) FROM (SELECT G, G + 1 AS H2, K FROM C) AS Q GROUP BY G";
    let refused = analyzed(&a, fixed);
    assert!(refused.contains("Parallel") && !refused.contains("side groups="), "{refused}");
    let (alone, together, _) = both(&a, fixed);
    assert!(alone.as_ref().is_ok_and(|r| r.len() > 5) && alone == together, "{fixed}: {alone:?}");
}

/// Verifies: [DESIGN] RULES.md D1324, D1281 [ISO/IEC 9075-2:2023 §7.17
/// GR4]: the first rows of a sort over such a grouping, by a plain set
/// function, by the DISTINCT one and by a grouping column, are the rows
/// the plan run alone gives.
#[test]
fn the_first_rows_of_a_sort_over_such_a_grouping_are_the_plans() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for order in ["S DESC, G", "D DESC, S", "G DESC"] {
            let sql = format!(
                "SELECT G, COUNT(DISTINCT N) AS D, SUM(K) AS S, COUNT(*) FROM {table} GROUP BY G ORDER BY {order} FETCH FIRST 7 ROWS ONLY"
            );
            let (alone, together, _) = both(&a, &sql);
            assert_eq!(alone.as_ref().map(Vec::len), Ok(7), "{sql}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1324, D1226, D298: at the smallest
/// MEMORY.WORK_BYTES such a grouping gives the rows it gives run alone,
/// of few groups, whose first grouping's partitions go to the spill, and
/// of a group every three rows, whose plain functions' states alone pass
/// the grant, so the part gives way to the plan run alone.
#[test]
fn such_a_grouping_past_the_grant_gives_its_rows() {
    let (db, a) = tables();
    let within: Vec<Rows> = [PAIRS[0], PAIRS[6]].iter().map(|shape| both(&a, &shape.replace("{t}", "C")).1).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    for (shape, held) in [PAIRS[0], PAIRS[6]].iter().zip(within) {
        let sql = shape.replace("{t}", "C");
        let (alone, together, _) = both(&a, &sql);
        assert!(held.as_ref().is_ok_and(|r| r.len() > 5), "{sql}: {held:?}");
        assert_eq!((&alone, &together), (&held, &held), "{sql}");
    }
}
