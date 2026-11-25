//! \[DESIGN\] A grouping over another that crosses its partitions
//! (RULES.md D1133): it groups by a set function's result of the first,
//! or by an expression a projection between the two computes, and counts.
//! Each partition's thread groups its own groups of the first again, the
//! statement's thread merges the counts, and the part gives the groups
//! the plan run alone gives, in its order, and the errors it raises.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, of the column store in groups of 2048, and R, of the row
/// store, alike: K, a key; G, one of 900; N, null for every 13th: so a
/// grouping by G makes many groups, of few counts each.
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
    for table in ["C", "R"] {
        let values: Vec<String> = (0_i64..7000)
            .map(|k| format!("({k}, {}, {})", k * 37 % 900, if k % 13 == 0 { "NULL".to_owned() } else { (k % 11).to_string() }))
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
/// then; the rows run in parallel by batch and by rows alike.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    for set in ALONE {
        exec(s, set).unwrap();
    }
    let alone = rows(s, sql);
    for set in TOGETHER {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let together = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert_eq!(rows(s, sql), together, "{sql}, its morsels read by rows");
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (alone, together, shown)
}

/// Whether `shown`, a plan, has a part run in parallel whose top is two
/// groupings, one over the other, projections between them or none.
fn crossed_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).filter(|l| !l.starts_with("Filter")).collect();
    lines.windows(2).enumerate().any(|(at, w)| {
        let part = w.first().is_some_and(|l| l.starts_with("Parallel")) && w.get(1).is_some_and(|l| l.starts_with("Aggregate"));
        let below = lines.iter().skip(at.saturating_add(2)).find(|l| !l.starts_with("Project"));
        part && below.is_some_and(|l| l.starts_with("Aggregate"))
    })
}

/// Verifies: [X3.135-1986 §5.22 GR2] (RULES.md D1133) a grouping by a
/// count of another's groups, or by an expression of it, counting, runs
/// as one part and gives the groups, in the order, it gives alone: of the
/// count itself, of a CAST and a CASE of it, as a LEFT join's count is
/// written, under a HAVING condition, and of COUNT of a column.
#[test]
fn a_count_of_counts_is_merged_across_partitions() {
    let (_db, a) = tables();
    for table in ["C", "R"] {
        for shape in [
            "SELECT X.M, COUNT(*) FROM (SELECT G, COUNT(N) AS M FROM {t} GROUP BY G) AS X GROUP BY X.M",
            "SELECT X.M, COUNT(*) FROM (SELECT G, CAST(CASE WHEN SUM(N) IS NULL THEN 0 ELSE SUM(N) END AS NUMERIC(18,0)) AS M FROM {t} GROUP BY G) AS X GROUP BY X.M",
            "SELECT X.M, COUNT(*) FROM (SELECT G, COUNT(N) AS M FROM {t} GROUP BY G) AS X GROUP BY X.M HAVING COUNT(*) > 40",
            "SELECT Y.P, COUNT(Y.G), COUNT(*) FROM (SELECT X.G, X.M + 1 AS P FROM (SELECT G, COUNT(*) AS M FROM {t} GROUP BY G) AS X) AS Y GROUP BY Y.P",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            assert!(crossed_part(&shown), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1133, D15) a second grouping of a set
/// function other than COUNT, whose states taken across partitions may
/// differ by order, is not run so; one whose projection raises gives way
/// to the serial plan and raises what it raises.
#[test]
fn other_set_functions_stay_above_and_a_raising_projection_gives_way() {
    let (_db, a) = tables();
    let summed = "SELECT X.M, SUM(X.G) FROM (SELECT G, COUNT(N) AS M FROM C GROUP BY G) AS X GROUP BY X.M";
    let (alone, together, shown) = both(&a, summed);
    assert!(!crossed_part(&shown), "{summed}\n{shown}");
    assert_eq!(alone, together, "{summed}");
    let raising = "SELECT X.M, COUNT(*) FROM (SELECT G, 100 / (COUNT(*) - 8) AS M FROM C GROUP BY G) AS X GROUP BY X.M";
    let (alone, together, _) = both(&a, raising);
    assert_eq!(alone, Err(turborel::error::Code::Data), "a group of 8 rows divides by zero");
    assert_eq!(alone, together, "{raising}");
}
