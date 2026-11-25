//! \[DESIGN\] Semi joins and antijoins read by batch and by morsels
//! (RULES.md D1070, D1071, D1075): one that builds its left operand
//! (`exec/batch/joined/semi/marked.rs`) and one that builds its right as
//! a step of its left's pipeline (`exec/batch/joined/semi/step.rs`) each
//! give the rows they give row by row on one thread, and the rows their
//! subquery gives unnested another way, of either engine, with and
//! without runtime filters, and past the work area.

use turborel::db::{Database, Session};
use turborel::optimizer::SEMI_JOINS;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, set_rule};

/// Tables O, orders, and L, their lines, of the column store in groups of
/// 2048, and OW and LW, of the row store, alike. O has K, a key; G, one of
/// 9, null for every 17th; D, a number below 1000. L has K, an order's
/// key or one no order has, null for every 19th; S, one of 5, null for
/// every 23rd; A and B, numbers compared.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE O (K INTEGER PRIMARY KEY, G INTEGER, D INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE L (K INTEGER, S INTEGER, A INTEGER, B INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE OW (K INTEGER PRIMARY KEY, G INTEGER, D INTEGER)
           CREATE TABLE LW (K INTEGER, S INTEGER, A INTEGER, B INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let null_or = |null: bool, v: i64| if null { "NULL".to_owned() } else { v.to_string() };
    for table in ["O", "OW"] {
        let values: Vec<String> = (0_i64..5000).map(|k| format!("({k}, {}, {})", null_or(k % 17 == 0, k % 9), k * 7919 % 1000)).collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    for table in ["L", "LW"] {
        let values: Vec<String> = (0_i64..9000)
            .map(|n| {
                let (k, s) = (null_or(n % 19 == 0, n * 31 % 6000), null_or(n % 23 == 0, n % 5));
                format!("({k}, {s}, {}, {})", n * 13 % 50, n * 29 % 50)
            })
            .collect();
        a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The shapes joined, `{o}` and `{l}` the tables: EXISTS of an order's
/// lines under a condition of their own; EXISTS and NOT EXISTS of another
/// line of a line's order, of another S, the second under a condition of
/// its own; NOT EXISTS on a key of nulls on both sides; EXISTS on two
/// keys; and EXISTS of another line of another S alone, unknown of a
/// null S.
const SHAPES: [&str; 5] = [
    "SELECT G, COUNT(*), SUM(D) FROM {o} WHERE D < 400 AND EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.A < {l}.B) GROUP BY G",
    "SELECT COUNT(*), SUM(L1.S), MIN(L1.K), MAX(L1.A) FROM {l} AS L1 WHERE L1.A > 40
       AND EXISTS (SELECT * FROM {l} AS L2 WHERE L2.K = L1.K AND L2.S <> L1.S)
       AND NOT EXISTS (SELECT * FROM {l} AS L3 WHERE L3.K = L1.K AND L3.S <> L1.S AND L3.A > L3.B)",
    "SELECT COUNT(*), SUM(K), COUNT(G) FROM {o} WHERE D < 500 AND NOT EXISTS (SELECT * FROM {l} WHERE {l}.S = {o}.G AND {l}.A < 45)",
    "SELECT COUNT(*), SUM(D) FROM {o} WHERE D < 300 AND EXISTS (SELECT * FROM {l} WHERE {l}.K = {o}.K AND {l}.S = {o}.G)",
    "SELECT COUNT(*), SUM(L1.A), COUNT(L1.S) FROM {l} AS L1 WHERE L1.A > 40
       AND EXISTS (SELECT * FROM {l} AS L2 WHERE L2.K = L1.K AND L2.S <> L1.S)",
];

/// The physical plan of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    s.physical_plan(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// Applies each of `settings` to `s`.
fn set(s: &Session, settings: &[&str]) {
    for setting in settings {
        exec(s, setting).unwrap();
    }
}

/// Verifies: [X3.135-1986 §5.17 GR2, GR3] (RULES.md D1070) a semi join or
/// an antijoin planned to build its left, read by batch, gives the rows
/// it gives row by row on one thread and the rows of its subquery
/// unnested with no semi join, of both engines: with a condition beyond
/// its keys, with null keys on either side, which match nothing, and on
/// two keys; its right read by morsels on 4 threads, as EXPLAIN shows, or by one; with
/// runtime filters and without.
#[test]
fn a_built_left_gives_the_rows_of_its_subquery() {
    let (db, a) = tables();
    for (o, l) in [("O", "L"), ("OW", "LW")] {
        for shape in SHAPES {
            let sql = shape.replace("{o}", o).replace("{l}", l);
            set(&a, &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]);
            let alone = rows(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            set_rule(&db, SEMI_JOINS, false);
            assert_eq!(rows(&a, &sql), alone, "with no semi join: {sql}");
            set_rule(&db, SEMI_JOINS, true);
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            assert_eq!(rows(&a, &sql), alone, "by batch on one thread: {sql}");
            set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
            let shown = physical(&a, &sql);
            let built_left = shown.contains("HashRightSemiJoin") || shown.contains("HashRightAntiJoin");
            assert!(built_left && shown.contains("Parallel  (degree=4)"), "{sql}\n{shown}");
            assert_eq!(rows(&a, &sql), alone, "by morsels: {sql}");
            exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
            assert_eq!(rows(&a, &sql), alone, "with no runtime filter: {sql}");
            exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1070, D38) a semi join that builds its
/// left gives way, its left past the work area, to the join that builds
/// its right, the batches it read of its left given first: its rows are
/// those of the join within a work area that holds it.
#[test]
fn a_left_past_the_work_area_is_joined_by_its_right() {
    let (db, a) = tables();
    set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
    let queries: Vec<String> = SHAPES.iter().map(|shape| shape.replace("{o}", "O").replace("{l}", "L")).collect();
    let held: Vec<_> = queries.iter().map(|sql| rows(&a, sql)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(65_536, 0))).unwrap();
    for (sql, held) in queries.iter().zip(held) {
        assert!(held.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {held:?}");
        assert_eq!(rows(&a, sql), held, "{sql}");
    }
}

/// Verifies: [X3.135-1986 §5.17 GR2, GR3] (RULES.md D1070) of a left
/// operand of no row a semi join and an antijoin that build it give no
/// row; of a right operand of no row, the semi join gives none and the
/// antijoin every left row.
#[test]
fn an_empty_operand_of_a_built_left() {
    let (_db, a) = tables();
    set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
    let count = |sql: &str| rows(&a, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")).join(";");
    assert_eq!(count("SELECT COUNT(*) FROM O WHERE D < 0 AND EXISTS (SELECT * FROM L WHERE L.K = O.K)"), "0");
    assert_eq!(count("SELECT COUNT(*) FROM O WHERE D < 0 AND NOT EXISTS (SELECT * FROM L WHERE L.K = O.K)"), "0");
    assert_eq!(count("SELECT COUNT(*) FROM O WHERE D < 400 AND EXISTS (SELECT * FROM L WHERE L.K = O.K AND L.A > 99)"), "0");
    let all = count("SELECT COUNT(*) FROM O WHERE D < 400");
    assert_eq!(count("SELECT COUNT(*) FROM O WHERE D < 400 AND NOT EXISTS (SELECT * FROM L WHERE L.K = O.K AND L.A > 99)"), all);
}

/// Verifies: [X3.135-1986 §5.17 GR2; ISO/IEC 9075:1992 §13.1 GR3] (RULES.md
/// D1071) a semi join and an antijoin under a sort, which reads them row
/// by row, are made of their batches, their rows those read with no
/// batch, in the order asked: of a join that builds its left and of one
/// that builds its right.
#[test]
fn a_semi_join_under_a_sort_is_made_of_its_batches() {
    let (_db, a) = tables();
    for sql in [
        "SELECT K, D FROM O WHERE D < 400 AND EXISTS (SELECT * FROM L WHERE L.K = O.K AND L.A < L.B) ORDER BY D, K",
        "SELECT K, D FROM O WHERE D < 400 AND NOT EXISTS (SELECT * FROM L WHERE L.K = O.K AND L.A < L.B) ORDER BY D DESC, K",
        "SELECT K, D FROM O WHERE EXISTS (SELECT * FROM L WHERE L.K = O.K AND L.A = 7) ORDER BY D, K",
    ] {
        set(&a, &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]);
        let alone = rows(&a, sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
        assert_eq!(rows(&a, sql), alone, "{sql}");
    }
}

/// The shapes whose right operand is the smaller, so it is built and the
/// left's pipeline probes it, `{o}` and `{l}` the tables: EXISTS and NOT
/// EXISTS of a line's order, few orders kept; EXISTS with a condition
/// beyond the key, unknown of a null S; and NOT EXISTS on a second key.
const STEPS: [&str; 4] = [
    "SELECT S, COUNT(*), SUM(A) FROM {l} WHERE EXISTS (SELECT * FROM {o} WHERE {o}.K = {l}.K AND {o}.D < 100) GROUP BY S",
    "SELECT COUNT(*), SUM(A), COUNT(K) FROM {l} WHERE NOT EXISTS (SELECT * FROM {o} WHERE {o}.K = {l}.K AND {o}.D < 100)",
    "SELECT COUNT(*), SUM(B) FROM {l} WHERE EXISTS (SELECT * FROM {o} WHERE {o}.K = {l}.K AND {o}.G <> {l}.S AND {o}.D < 200)",
    "SELECT COUNT(*), SUM(B) FROM {l} WHERE NOT EXISTS (SELECT * FROM {o} WHERE {o}.K = {l}.K AND {o}.G = {l}.S AND {o}.D < 200)",
];

/// Verifies: [X3.135-1986 §5.17 GR2, GR3] (RULES.md D1075) a hashed semi
/// join or antijoin is a step of its left operand's pipeline, inside the
/// part run in parallel as EXPLAIN shows, and its morsels keep the rows
/// the join keeps row by row on one thread and its subquery keeps
/// unnested with no semi join: a left row of a null key matches nothing,
/// and a condition beyond the keys unknown of a pair does not match it;
/// read by batch, and a row at a time with EXECUTOR.BATCHES off.
#[test]
fn a_hashed_semi_join_is_a_step_of_its_lefts_pipeline() {
    let (db, a) = tables();
    for (o, l) in [("O", "L"), ("OW", "LW")] {
        for shape in STEPS {
            let sql = shape.replace("{o}", o).replace("{l}", l);
            set(&a, &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]);
            let alone = rows(&a, &sql);
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            set_rule(&db, SEMI_JOINS, false);
            assert_eq!(rows(&a, &sql), alone, "with no semi join: {sql}");
            set_rule(&db, SEMI_JOINS, true);
            set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
            assert_eq!(rows(&a, &sql), alone, "by morsels, a row at a time: {sql}");
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            let shown = physical(&a, &sql);
            let part = shown.find("Parallel  (degree=4)");
            let join = shown.find("HashSemiJoin").or_else(|| shown.find("HashAntiJoin"));
            assert!(part.zip(join).is_some_and(|(part, join)| part < join), "{sql}\n{shown}");
            assert_eq!(rows(&a, &sql), alone, "by morsels: {sql}");
            exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'OFF'").unwrap();
            assert_eq!(rows(&a, &sql), alone, "with no runtime filter: {sql}");
            exec(&a, "SET EXECUTOR.RUNTIME_FILTERS = 'ON'").unwrap();
        }
    }
}

/// Verifies: [ISO/IEC 9075:1992 §8.7 GR2; X3.135-1986 §5.16 GR2] (RULES.md
/// D1077) NOT IN of a subquery, both columns declared to hold no null, is
/// an antijoin hashed on their equality, as EXPLAIN shows, a step of its
/// left's pipeline; of a column that may hold a null on either side it
/// keeps its condition, a null among the values or the row's own null
/// keeping no row. Each has the rows of the subquery run with no semi
/// join.
#[test]
fn not_in_of_columns_that_hold_no_null_is_hashed() {
    let (db, a) = tables();
    for (o, l) in [("O", "L"), ("OW", "LW")] {
        for (shape, hashed) in [
            ("SELECT COUNT(*), SUM(D) FROM {o} WHERE K NOT IN (SELECT K FROM {o} WHERE D < 500)", true),
            ("SELECT COUNT(*), SUM(D) FROM {o} WHERE D > 900 AND G NOT IN (SELECT K FROM {o} WHERE D < 5)", false),
            ("SELECT COUNT(*), SUM(D) FROM {o} WHERE D > 900 AND K NOT IN (SELECT S FROM {l} WHERE A < 3)", false),
            ("SELECT COUNT(*), SUM(D) FROM {o} WHERE D > 900 AND K NOT IN (SELECT S FROM {l} WHERE A < 3 AND S IS NOT NULL)", false),
        ] {
            let sql = shape.replace("{o}", o).replace("{l}", l);
            set(&a, &["SET EXECUTOR.DEGREE = 1", "SET EXECUTOR.BATCHES = 'OFF'"]);
            set_rule(&db, SEMI_JOINS, false);
            let nested = rows(&a, &sql);
            set_rule(&db, SEMI_JOINS, true);
            assert!(nested.is_ok(), "{sql}: {nested:?}");
            assert_eq!(rows(&a, &sql), nested, "alone: {sql}");
            exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
            set(&a, &["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]);
            let shown = physical(&a, &sql);
            assert_eq!(shown.contains("HashAntiJoin") || shown.contains("HashRightAntiJoin"), hashed, "{sql}\n{shown}");
            assert!(shown.contains("AntiJoin"), "{sql}\n{shown}");
            assert_eq!(rows(&a, &sql), nested, "by morsels: {sql}");
        }
    }
}
