//! \[DESIGN\] Index nested-loop joins over a part run in parallel probe on
//! its threads (RULES.md D1114): the rows they give, their order and the
//! errors they raise are those of the joins run alone.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables B, of the column store, and R, of the row store, alike, of 6000
/// rows: K, a key; G, one of 40; N, one of 100; and D, of 30000 rows, K
/// its primary key, V one of 7; an index's probes priced low.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    // Probes made cheap, as of a table too large to scan for a few rows.
    db.set_setting("PLANNER.INDEX_DESCENT_COST", &Value::Approx(0.01_f64)).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE B (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, N INTEGER)
           CREATE TABLE D (K INTEGER PRIMARY KEY, V INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["B", "R"] {
        let values: Vec<String> = (0_i64..6000).map(|k| format!("({}, {}, {})", k * 5, k % 40, k * 7919 % 100)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    let values: Vec<String> = (0_i64..30000).map(|k| format!("({k}, {})", k % 7)).collect();
    a.run(&format!("INSERT INTO D VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of a query, or the code it failed with.
type Rows = Result<Vec<String>, turborel::error::Code>;

/// The rows of `sql` run alone on one thread, its rows run on 4 threads
/// in morsels of 256 rows, and its plan then.
fn both(s: &Session, sql: &str) -> (Rows, Rows, String) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    let alone = rows(s, sql);
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
    let shown = s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    (alone, rows(s, sql), shown)
}

/// Whether `shown`, a plan, has an index nested-loop join with a part run
/// in parallel below it.
fn looped(shown: &str) -> bool {
    shown.find("IndexNestedLoop").is_some_and(|at| shown.get(at..).is_some_and(|below| below.contains("Parallel")))
}

/// Verifies: [X3.135-1986 §5.20 GR2b] (RULES.md D1114) an inner join, a
/// LEFT join, a semi join and an antijoin by an index, and a chain of
/// them, over a part run in parallel give the rows they give run alone,
/// in their order, under a grouping and as they come; and over a scan of
/// the column store, which stays on the statement's thread, alike.
#[test]
fn index_joins_over_a_part_give_their_rows_in_order() {
    let (_db, a) = tables();
    for table in ["B", "R"] {
        for shape in [
            "SELECT {t}.K, D.V FROM {t} JOIN D ON D.K = {t}.K WHERE {t}.N < 30",
            "SELECT {t}.G, COUNT(*), SUM(D.V) FROM {t} JOIN D ON D.K = {t}.K WHERE {t}.N < 30 GROUP BY {t}.G",
            "SELECT {t}.K, D.V FROM {t} LEFT JOIN D ON D.K = {t}.K AND D.V > 2 WHERE {t}.N < 30",
            "SELECT {t}.K FROM {t} WHERE {t}.N < 30 AND EXISTS (SELECT * FROM D WHERE D.K = {t}.K AND D.V <> {t}.G)",
            "SELECT {t}.K FROM {t} WHERE {t}.N < 30 AND NOT EXISTS (SELECT * FROM D WHERE D.K = {t}.K AND D.V = 3)",
            "SELECT {t}.K, D.V FROM {t} JOIN D ON D.K = {t}.K WHERE {t}.N < 30 AND EXISTS (SELECT * FROM D AS E WHERE E.K = D.K AND E.V <> {t}.G)",
        ] {
            let sql = shape.replace("{t}", table);
            let (alone, together, shown) = both(&a, &sql);
            // A scan by batch of the column store stays on the statement's
            // thread (RULES.md D291), so its joins do too.
            assert_eq!(looped(&shown), table == "R", "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [X3.135-1986 §6.3 GR4] (RULES.md D1114, D15) an index join
/// over a part whose left operand raises for some row raises what it
/// raises run alone, after the rows before it.
#[test]
fn an_index_join_over_a_part_raises_as_alone() {
    let (_db, a) = tables();
    for sql in ["SELECT L.K, L.X, D.V FROM (SELECT K, 10 / (K - 15000) AS X FROM R WHERE N < 30) AS L JOIN D ON D.K = L.K"] {
        let (alone, together, shown) = both(&a, sql);
        assert!(looped(&shown), "{sql}\n{shown}");
        assert!(alone.is_err(), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}");
    }
}
