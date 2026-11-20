//! DISTINCT set functions computed by two groupings (RULES.md D340-D342,
//! `optimizer/distinct_aggregates.rs`): each query's rows, in their
//! order, its warning and its error, alike with the rule on and off, of
//! rows read by batches and row by row.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::optimizer::DISTINCT_AGGREGATES;
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// The rows of T: K, G, X, Y, D, S. Duplicates, nulls, character strings
/// alike but for case, approximate numbers whose sum depends on their
/// order, and a 0 of X.
const T_ROWS: &str = "(1, 1, 1, 0.1, 1.10, 'a'), (2, 1, 1, 0.2, 1.10, 'A'), (3, 1, 2, 0.3, 2.00, 'b'),
    (4, 1, NULL, 0.4, NULL, NULL), (5, 2, 3, 0.5, 3.00, 'B'), (6, 2, 3, 0.25, 3.00, 'b'),
    (7, 2, 0, NULL, 0.00, 'c'), (8, 3, NULL, NULL, NULL, NULL), (9, NULL, 5, 0.125, 5.50, 'a'),
    (10, NULL, 5, 0.1, 5.5, 'd'), (11, 2, -3, 0.3, -3.00, 'C'), (12, 1, 2, 0.7, 2.0, 'a')";

/// The rows of the column store's table C.
const C_ROWS: i64 = 5000;

/// Tables T and C of the row store and the column store, E empty, alike
/// in their columns, of schema A.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, G INTEGER, X INTEGER, Y DOUBLE PRECISION, D DECIMAL(9, 2), S VARCHAR(10))
           CREATE TABLE E (K INTEGER, G INTEGER, X INTEGER, Y DOUBLE PRECISION, D DECIMAL(9, 2), S VARCHAR(10))
           CREATE TABLE C (K INTEGER, G INTEGER, X INTEGER, Y DOUBLE PRECISION, D DECIMAL(9, 2), S VARCHAR(10))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES {T_ROWS}")).unwrap();
    for first in (0..C_ROWS).step_by(500) {
        let values: Vec<String> = (first..first + 500)
            .map(|k| {
                let x = if k % 17 == 0 { "NULL".to_owned() } else { (k * 7 % 1300).to_string() };
                format!("({k}, {}, {x}, {}.5, {}.25, 's{}')", k % 9, k % 40, k % 300, k % 61)
            })
            .collect();
        a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of `sql` in their order and its warning, or its error.
fn outcome(s: &Session, sql: &str) -> String {
    match rows(s, sql) {
        Ok(found) => format!("{found:?} {:?}", s.warning()),
        Err(code) => format!("{code:?}"),
    }
}

/// Turns the rule on or off for `s`.
fn rule(s: &Session, on: bool) {
    exec(s, &format!("SET OPTIMIZER.{} = '{}'", DISTINCT_AGGREGATES.to_uppercase(), if on { "ON" } else { "OFF" })).unwrap();
}

/// Whether the rule rewrote `sql`'s plan.
fn rewritten(s: &Session, sql: &str) -> bool {
    let plan = s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    plan.lines().any(|l| l.starts_with("-- rules:") && l.contains(DISTINCT_AGGREGATES))
}

/// The outcome of `sql`, which must be alike with the rule on and off,
/// by batches and row by row, and which the rule rewrites.
fn alike(s: &Session, sql: &str) -> String {
    assert!(rewritten(s, sql), "not rewritten: {sql}");
    compared(s, sql)
}

/// The outcome of `sql`, which must be alike with the rule on and off,
/// by batches and row by row.
fn compared(s: &Session, sql: &str) -> String {
    let on = outcome(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows_on = outcome(s, sql);
    rule(s, false);
    let rows_off = outcome(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let off = outcome(s, sql);
    rule(s, true);
    assert_eq!(on, off, "{sql}");
    assert_eq!(rows_on, rows_off, "{sql}, row by row");
    assert_eq!(on, rows_on, "{sql}, by batches and not");
    on
}

/// The queries compared, of `table`.
fn queries(table: &str) -> Vec<String> {
    [
        "SELECT COUNT(DISTINCT X) FROM {T}",
        "SELECT G, COUNT(DISTINCT X) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X), SUM(DISTINCT X), AVG(DISTINCT X), MIN(DISTINCT X), MAX(DISTINCT S) FROM {T} GROUP BY G",
        "SELECT G, SUM(DISTINCT Y), AVG(DISTINCT Y), SUM(DISTINCT D), AVG(DISTINCT D) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X), COUNT(*), COUNT(Y), SUM(D), MIN(S) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X), SUM(Y), AVG(Y), COUNT(*) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X), COUNT(DISTINCT S), SUM(DISTINCT D) FROM {T} GROUP BY G",
        "SELECT COUNT(DISTINCT X), COUNT(DISTINCT S), COUNT(*), SUM(Y), SUM(D) FROM {T}",
        "SELECT G, COUNT(DISTINCT X) FILTER (WHERE K > 3), COUNT(DISTINCT X), COUNT(*) FILTER (WHERE Y > 0.2) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X) FILTER (WHERE K > 3), SUM(Y) FILTER (WHERE K < 9) FROM {T} GROUP BY G",
        "SELECT COUNT(DISTINCT X) FILTER (WHERE S IS NOT NULL), SUM(DISTINCT D) FILTER (WHERE S IS NOT NULL) FROM {T}",
        "SELECT G, COUNT(DISTINCT X + G), SUM(DISTINCT X * 2) FROM {T} GROUP BY G",
        "SELECT G, COUNT(DISTINCT X) FROM {T} GROUP BY G HAVING COUNT(DISTINCT X) > 1 AND G > 0",
        "SELECT G, COUNT(DISTINCT X), SUM(D), GROUPING(G) FROM {T} GROUP BY ROLLUP (G)",
        "SELECT G, S, COUNT(DISTINCT X), COUNT(DISTINCT D), SUM(Y) FROM {T} GROUP BY CUBE (G, S)",
        "SELECT G, COUNT(DISTINCT S), COUNT(*) FROM {T} GROUP BY GROUPING SETS ((G), ())",
        "SELECT COUNT(DISTINCT S COLLATE CASE_INSENSITIVE), COUNT(DISTINCT S) FROM {T}",
        "SELECT S COLLATE CASE_INSENSITIVE, COUNT(DISTINCT X), COUNT(*) FROM {T} GROUP BY S COLLATE CASE_INSENSITIVE",
        "SELECT COUNT(DISTINCT X), SUM(DISTINCT X) FROM {T} WHERE K < 0",
        "SELECT G, COUNT(DISTINCT X) FROM {T} WHERE K < 0 GROUP BY G",
    ]
    .iter()
    .map(|q| q.replace("{T}", table))
    .collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i, GR7d] [DESIGN] RULES.md
/// D340-D342: COUNT, SUM, AVG, MAX and MIN with DISTINCT, alone, several
/// of one argument, of several arguments, mixed with functions without
/// DISTINCT of states and of none, filtered, of expressions, under
/// HAVING, ROLLUP, CUBE, GROUPING SETS and a collation, and of no rows:
/// alike with the rule and without it, of the row store and the column
/// store. A correlated subquery's, whose plan the rule is not given,
/// alike too.
#[test]
fn distinct_functions_alike_with_and_without_the_rewrite_10_9() {
    let (_db, a) = tables();
    for table in ["T", "C", "E"] {
        for sql in queries(table) {
            alike(&a, &sql);
        }
    }
    compared(&a, "SELECT K, (SELECT COUNT(DISTINCT X) FROM T U WHERE U.G = T.G) FROM T WHERE K < 20");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d i, ii] of no rows COUNT
/// with DISTINCT is 0 and every other result null, of a grouping without
/// GROUP BY; with GROUP BY there is no group.
#[test]
fn of_no_rows_count_is_0_and_others_null_10_9() {
    let (_db, a) = tables();
    let sql = "SELECT COUNT(DISTINCT X), SUM(DISTINCT X), AVG(DISTINCT Y), MAX(DISTINCT S), COUNT(*), SUM(Y) FROM E";
    assert_eq!(alike(&a, sql), "[\"0,NULL,NULL,NULL,0,NULL\"] None");
    assert_eq!(alike(&a, "SELECT G, COUNT(DISTINCT X) FROM E GROUP BY G"), "[] None");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7a, GR4a] a null of a DISTINCT
/// function's argument is taken out and warns, 01003; of rows the filter
/// does not admit it does not; the values counted are those of other
/// rows, each once.
#[test]
fn nulls_are_taken_out_and_warn_10_9() {
    let (_db, a) = tables();
    assert_eq!(alike(&a, "SELECT COUNT(DISTINCT X) FROM T"), format!("[\"6\"] {:?}", Some(SqlState::NULL_VALUE_ELIMINATED)));
    assert_eq!(alike(&a, "SELECT COUNT(DISTINCT X) FILTER (WHERE K <> 4 AND K <> 8), COUNT(*) FROM T"), "[\"6,12\"] None");
    assert_eq!(alike(&a, "SELECT G, COUNT(DISTINCT D) FROM T WHERE G = 2 GROUP BY G"), "[\"2,3\"] None");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR4a] [DESIGN] RULES.md D15:
/// an argument is computed only of rows its filter admits, so a division
/// it guards raises nothing; unguarded, it raises 22012 with the rule
/// and without it.
#[test]
fn arguments_are_computed_only_of_rows_admitted_10_9() {
    let (_db, a) = tables();
    let guarded = "SELECT G, COUNT(DISTINCT 6 / X) FILTER (WHERE X <> 0), COUNT(DISTINCT S) FROM T GROUP BY G";
    assert!(!alike(&a, guarded).contains("Data"));
    let unguarded = "SELECT COUNT(DISTINCT 6 / X), COUNT(DISTINCT S) FROM T";
    assert_eq!(alike(&a, unguarded), format!("{:?}", Code::Data));
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7b i, §8.2] [EXTENSION]
/// RULES.md D60: DISTINCT tells values apart by their collation, and a
/// grouping by a collation keeps its groups, each showing its first
/// row's value.
#[test]
fn collations_decide_what_is_distinct_10_9() {
    let (_db, a) = tables();
    assert_eq!(alike(&a, "SELECT COUNT(DISTINCT S COLLATE CASE_INSENSITIVE), COUNT(DISTINCT S) FROM T"), format!("[\"4,7\"] {:?}", Some(SqlState::NULL_VALUE_ELIMINATED)));
    let grouped = alike(&a, "SELECT S COLLATE CASE_INSENSITIVE, COUNT(DISTINCT X) FROM T WHERE S IS NOT NULL GROUP BY S COLLATE CASE_INSENSITIVE");
    assert_eq!(grouped, "[\"a,3\", \"b,2\", \"c,2\", \"d,1\"] None");
}

/// Verifies: [DESIGN] RULES.md D340-D342: EXPLAIN shows the rule fired,
/// the two groupings, the parts of the functions without DISTINCT and
/// their merge, and of more than one slot the copies of each row.
#[test]
fn explain_shows_the_two_groupings() {
    let (_db, a) = tables();
    let plan = a.explain("SELECT G, COUNT(DISTINCT X), COUNT(DISTINCT S), COUNT(*) FROM T GROUP BY G", &Params::new()).unwrap();
    let physical = plan.split("-- physical:").nth(1).unwrap();
    assert_eq!(physical.matches("Aggregate").count(), 2, "{plan}");
    for shown in ["PARTIAL COUNT(*)", "MERGED COUNT(*)", "COPY", "COUNT(DISTINCT T.X)"] {
        assert!(physical.contains(shown), "{shown} in {plan}");
    }
    let single = a.explain("SELECT COUNT(DISTINCT X) FROM C", &Params::new()).unwrap();
    assert!(!single.contains("COPY"), "{single}");
}

/// Verifies: [DESIGN] RULES.md D340: an aggregation of an inverse
/// distribution function, or of an argument invoking a routine, is left to
/// a DISTINCT function's own way, and its results are those it gave.
#[test]
fn some_aggregations_are_left_alone() {
    let (_db, a) = tables();
    let sorted = "SELECT COUNT(DISTINCT X), PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY X) FROM T";
    assert!(!rewritten(&a, sorted));
    assert_eq!(rows(&a, sorted).unwrap(), ["6,2E0"]);
    a.run("CREATE FUNCTION TWICE (N INTEGER) RETURNS INTEGER CONTAINS SQL RETURN N * 2").unwrap();
    let invoked = "SELECT COUNT(DISTINCT TWICE(X)) FROM T";
    assert!(!rewritten(&a, invoked));
    assert_eq!(rows(&a, invoked).unwrap(), ["6"]);
    rule(&a, false);
    assert!(!rewritten(&a, "SELECT COUNT(DISTINCT X) FROM T"));
}

/// Verifies: [DESIGN] RULES.md D38, D340: the first grouping spills past
/// a small work area and the results are alike.
#[test]
fn the_first_grouping_spills_alike() {
    let (_db, a) = tables();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for sql in [
        "SELECT COUNT(DISTINCT X), COUNT(DISTINCT K), SUM(DISTINCT D), COUNT(*) FROM C",
        "SELECT G, COUNT(DISTINCT K), SUM(Y), SUM(D) FROM C GROUP BY G",
        "SELECT G, COUNT(DISTINCT K), COUNT(DISTINCT X) FILTER (WHERE K > 100), MAX(S) FROM C GROUP BY G",
    ] {
        alike(&a, sql);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.23 SR8d ii, Feature T627] DISTINCT
/// set functions of windows, which the rule does not rewrite, keep their
/// results, their values kept in the bag of values taken.
#[test]
fn window_distinct_functions_keep_their_results() {
    let (_db, a) = tables();
    let sql = "SELECT K, COUNT(DISTINCT X) OVER (PARTITION BY G), COUNT(DISTINCT X) OVER (ORDER BY K ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) FROM T WHERE K < 8 ORDER BY K";
    assert_eq!(rows(&a, sql).unwrap(), ["1,2,1", "2,2,1", "3,2,2", "4,2,2", "5,2,2", "6,2,1", "7,2,2"]);
}
