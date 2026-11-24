//! \[DESIGN\] A grouping run in parallel under its HAVING condition, and a
//! grouping read by a join (RULES.md D1072, D1073): the groups a part
//! keeps are those the plan run alone keeps, in its order, and a join
//! over a grouping gives the rows it gives row by row.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 500, null for every 41st; N, a
/// number, null for every 13th.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {})", null_or(k % 41 == 0, k * 7 % 500), null_or(k % 13 == 0, k * 7919 % 100)))
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run alone a row at a time, its rows run in parallel
/// on 4 threads in morsels of 256 rows, and its plan then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is a
/// filter: the line after a `Parallel` line begins with `Filter`.
fn filtered_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).collect();
    lines
        .windows(2)
        .any(|pair| pair.first().is_some_and(|l| l.starts_with("Parallel")) && pair.get(1).is_some_and(|l| l.starts_with("Filter")))
}

/// Verifies: [X3.135-1986 §5.23 GR1] (RULES.md D1073) a HAVING condition
/// that cannot raise over a grouping run in parallel is a part with it,
/// as EXPLAIN shows, and keeps the groups it keeps run alone, in their
/// order: of some groups, of none, of every group, of a null sum, which
/// is not true of it, and of a grouping by no column, of one group even
/// of no rows.
#[test]
fn a_having_condition_is_judged_by_its_part() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT G, SUM(N), COUNT(*) FROM {t} GROUP BY G HAVING SUM(N) > 700",
            "SELECT G, COUNT(N) FROM {t} GROUP BY G HAVING SUM(N) > 1000000",
            "SELECT G, MAX(N) FROM {t} GROUP BY G HAVING COUNT(*) > 0",
            "SELECT G, MIN(K) FROM {t} WHERE K < 3000 GROUP BY G HAVING COUNT(N) < 5 AND MAX(N) > 90",
            "SELECT COUNT(*), SUM(N) FROM {t} HAVING COUNT(*) > 10",
            "SELECT COUNT(*) FROM {t} WHERE N < 0 HAVING COUNT(*) = 0",
            "SELECT COUNT(*) FROM {t} WHERE N < 0 HAVING COUNT(*) > 0",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(filtered_part(&shown), "{sql}\n{shown}");
            assert!(alone.is_ok(), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1073, D15) a HAVING condition that can
/// raise stays above its grouping's part, judged in the groups' order,
/// and raises the error it raises run alone.
#[test]
fn a_having_condition_that_can_raise_stays_above_its_part() {
    let (_db, a) = tables();
    for sql in [
        "SELECT G, SUM(N) FROM C GROUP BY G HAVING 1000 / SUM(N) > 1",
        "SELECT G, COUNT(N) FROM C GROUP BY G HAVING 10 / (COUNT(N) - 12) > 1",
    ] {
        let (alone, together, shown) = both(&a, sql);
        assert!(!filtered_part(&shown) && shown.contains("Parallel"), "{sql}\n{shown}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1] (RULES.md D1072) a join over a
/// grouping that takes no runtime filter, of SUM, alone or run in
/// parallel under its HAVING condition, gives the rows it gives row by
/// row on one thread: joined as a derived table, and as the subquery of
/// IN.
#[test]
fn a_join_over_a_grouping_reads_it_by_batch() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT COUNT(*), SUM(X.S), SUM({t}.N) FROM {t} JOIN (SELECT G, SUM(N) AS S FROM {t} GROUP BY G) AS X ON X.G = {t}.K",
            "SELECT COUNT(*), SUM(X.S), MIN({t}.K) FROM {t} JOIN (SELECT G, SUM(N) AS S FROM {t} GROUP BY G HAVING SUM(N) > 700) AS X ON X.G = {t}.K",
            "SELECT COUNT(*), SUM(N) FROM {t} WHERE K IN (SELECT G FROM {t} GROUP BY G HAVING COUNT(*) > 13 AND MAX(N) > 95)",
            "SELECT COUNT(*), SUM(N) FROM {t} WHERE K NOT IN (SELECT G FROM {t} WHERE G IS NOT NULL GROUP BY G HAVING COUNT(*) > 13)",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, _) = both(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
            exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
            assert_eq!(rows(&a, &sql), alone, "by batch on one thread: {sql}");
        }
    }
}
