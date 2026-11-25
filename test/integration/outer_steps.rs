//! \[DESIGN\] A LEFT outer join's probe as a step of a pipeline run in
//! parallel (RULES.md D1076): the join, and a grouping over it, give the
//! rows the plan run alone gives, in its order, of either engine.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Tables C, customers, and O, their orders, of the column store in
/// groups of 2048, and CW and OW, of the row store, alike. C has K, a
/// key, null for every 37th, and N. O has K, a key; CK, a customer's key
/// or one no customer has, null for every 29th, a third of the customers
/// having none; N; and S, a text.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE O (K INTEGER, CK INTEGER, N INTEGER, S VARCHAR(30)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE CW (K INTEGER, N INTEGER)
           CREATE TABLE OW (K INTEGER, CK INTEGER, N INTEGER, S VARCHAR(30))",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["C", "CW"] {
        let values: Vec<String> = (0_i64..5000).map(|k| format!("({}, {})", null_or(k % 37 == 0, k), k % 11)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    for table in ["O", "OW"] {
        let values: Vec<String> = (0_i64..9000)
            .map(|k| {
                let ck = null_or(k % 29 == 0, k * 7 % 1834 * 3);
                let s = ["special requests", "plain", "specially late requests", "requests special"][usize::try_from(k % 4).unwrap()];
                format!("({k}, {ck}, {}, '{s}')", k % 13)
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

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR4, GR5b] (RULES.md D1076) a LEFT
/// outer join on its keys alone is a step of its left operand's pipeline,
/// inside the part run in parallel as EXPLAIN shows: its rows are those
/// of the plan run alone, in its order, a left row no right row matches
/// given once with nulls, one of a null key too; so are a grouping over
/// it, the count of each customer's orders and the customers of each
/// count, a condition of the right operand's own under it; read by
/// batch, and a row at a time with EXECUTOR.BATCHES off.
#[test]
fn a_left_join_is_a_step_of_its_lefts_pipeline() {
    let (_db, a) = tables();
    for (c, o) in [("C", "O"), ("CW", "OW")] {
        for shape in [
            "SELECT C.K, C.N, O.K, O.N FROM {c} AS C LEFT JOIN {o} AS O ON C.K = O.CK",
            "SELECT C.K, COUNT(O.K), SUM(O.N) FROM {c} AS C LEFT JOIN {o} AS O ON C.K = O.CK GROUP BY C.K",
            "SELECT CNT, COUNT(*) FROM (SELECT C.K, COUNT(O.K) FROM {c} AS C LEFT JOIN {o} AS O
               ON C.K = O.CK AND O.S NOT LIKE '%special%requests%' GROUP BY C.K) AS X (K, CNT) GROUP BY CNT",
            "SELECT COUNT(*), COUNT(O.K), SUM(C.N) FROM {c} AS C LEFT JOIN {o} AS O ON C.K = O.CK AND C.N = O.N",
        ] {
            let sql = shape.replace("{c}", c).replace("{o}", o);
            let (alone, together, shown) = both(&a, &sql);
            let (part, join) = (shown.find("Parallel  (degree=4)"), shown.find("LeftOuterJoin"));
            assert!(part.zip(join).is_some_and(|(part, join)| part < join), "{sql}\n{shown}");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
            exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
            assert_eq!(rows(&a, &sql), alone, "by morsels, a row at a time: {sql}");
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR5b] (RULES.md D1076) a LEFT
/// outer join with a condition beyond its keys ends its pipeline, as the
/// condition decides which pairs match; its rows are those of the plan
/// run alone.
#[test]
fn a_left_join_on_more_than_its_keys_is_no_step() {
    let (_db, a) = tables();
    let sql = "SELECT COUNT(*), COUNT(O.K), SUM(O.N) FROM C LEFT JOIN O ON C.K = O.CK AND O.N > C.N";
    let (alone, together, shown) = both(&a, sql);
    let (part, join) = (shown.find("Parallel  (degree=4)"), shown.find("LeftOuterJoin"));
    assert!(join.is_some() && part.zip(join).is_none_or(|(part, join)| part > join), "{sql}\n{shown}");
    assert_eq!(alone, together, "{sql}");
}
