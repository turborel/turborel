//! \[DESIGN\] A grouping over another whose grouping columns hold its
//! own, as DISTINCT set functions make them (RULES.md D340, D1113), run
//! as one part: each partition's thread makes the second grouping's
//! groups of its own, and the part gives the groups the plan run alone
//! gives, in its order.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 50, null for every 41st; H, one of
/// 7; N, one of 10 apart from G, null for every 13th: few groups of G and
/// N beside the rows, so a grouping by both runs in parallel.
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
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {}, {})", null_or(k % 41 == 0, k * 7 % 50), k % 7, null_or(k % 13 == 0, k / 7 % 10)))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The settings a query runs alone under, a row at a time on one thread.
const ALONE: [&str; 2] = ["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"];

/// The settings a query runs in parallel under, on 4 threads in morsels
/// of 256 rows.
const TOGETHER: [&str; 4] =
    ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"];

/// The rows of `sql` run alone, its rows run in parallel, and its plan
/// then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    for set in ALONE {
        exec(s, set).unwrap();
    }
    let alone = rows(s, sql);
    for set in TOGETHER {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is two
/// groupings, one over the other, under a filter or not.
fn regrouped_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).filter(|l| !l.starts_with("Filter")).collect();
    lines.windows(3).any(|w| {
        let starts = |at: usize, with: &str| w.get(at).is_some_and(|l| l.starts_with(with));
        starts(0, "Parallel") && starts(1, "Aggregate") && starts(2, "Aggregate")
    })
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i] (RULES.md D1113) a
/// grouping with DISTINCT set functions, made by two groupings, runs as
/// one part, as EXPLAIN shows, and gives the groups it gives run alone,
/// in their order: of one DISTINCT function, of one beside functions
/// computed in parts, of two of different arguments, of a grouping by two
/// columns, under a HAVING condition, and of rows a WHERE clause leaves.
#[test]
fn distinct_set_functions_are_regrouped_by_their_part() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G",
            "SELECT G, COUNT(DISTINCT N), SUM(N), COUNT(*), MAX(K) FROM {t} GROUP BY G",
            "SELECT H, COUNT(DISTINCT N), SUM(DISTINCT G) FROM {t} GROUP BY H",
            "SELECT H, N, COUNT(DISTINCT K / 1000) FROM {t} GROUP BY H, N",
            "SELECT G, COUNT(DISTINCT N) FROM {t} GROUP BY G HAVING COUNT(DISTINCT N) > 8",
            "SELECT H, AVG(DISTINCT N) FROM {t} WHERE K < 5000 GROUP BY H",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(regrouped_part(&shown), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1] (RULES.md D1113, D1063) a join
/// over such a part, whose filters are of the second grouping's columns,
/// tells the first's pipeline none, and gives the rows it gives alone.
#[test]
fn a_join_over_a_regrouped_part_gives_its_rows() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        let sql = format!(
            "SELECT COUNT(*), SUM(X.D), SUM({table}.N) FROM {table} JOIN (SELECT G, COUNT(DISTINCT N) AS D FROM {table} GROUP BY G) AS X ON X.G = {table}.K"
        );
        let (alone, together, _) = both(&a, &sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1113) a pair the part does not hold
/// whole, of a grouping by no column, whose one group would be one
/// partition's, or with groups of empty grouping sets, keeps the second
/// grouping above the part and gives the rows it gives alone.
#[test]
fn a_pair_of_no_grouping_column_or_of_grouping_sets_stays_two() {
    let (_db, a) = tables();
    for sql in ["SELECT COUNT(DISTINCT N), SUM(N) FROM C", "SELECT H, COUNT(DISTINCT N) FROM C GROUP BY ROLLUP (H)"] {
        let (alone, together, shown) = both(&a, sql);
        assert!(!regrouped_part(&shown), "{sql}\n{shown}");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [EXTENSION] (RULES.md D1113, D378) EXPLAIN ANALYZE of such
/// a part counts the groups of the first grouping its partitions made,
/// as many as the grouping by both columns makes.
#[test]
fn explain_analyze_counts_the_first_grouping_of_a_part() {
    let (_db, a) = tables();
    for set in TOGETHER {
        exec(&a, set).unwrap();
    }
    let sql = "SELECT G, COUNT(DISTINCT N) FROM C GROUP BY G";
    let shown = a.physical_plan(sql, &Params::new()).unwrap();
    assert!(regrouped_part(&shown), "{shown}");
    let analyzed = a.analyzed(sql, &Params::new()).unwrap();
    let firsts: Vec<u64> =
        analyzed.operators.iter().filter(|o| o.label.starts_with("Aggregate group by [C.G, C.N]")).map(|o| o.rows).collect();
    let expected = rows(&a, "SELECT COUNT(*) FROM (SELECT G, N FROM C GROUP BY G, N) AS X").unwrap();
    assert_eq!(
        firsts.iter().map(u64::to_string).collect::<Vec<_>>(),
        expected,
        "{:?}",
        analyzed.operators.iter().map(|o| &o.label).collect::<Vec<_>>()
    );
}
