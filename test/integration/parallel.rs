//! \[DESIGN\] Queries run in parallel by morsels (RULES.md D290 to D296):
//! each part the planner marks gives the rows the plan run alone gives, in
//! its order, and raises its errors, of either engine, frozen rows changed
//! among them; the planner marks only what reads enough rows; and a
//! statement sees its transaction's own writes.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The rows written to each of R and C: K, a key; G, one of 7 groups; S, a
/// text, null for every 11th; N, a number, null for every 13th.
const ROWS: i64 = 6000;

/// Tables R, of the row store, and C, of the column store in groups of 2048,
/// alike, written, frozen, then changed; and D, a dimension of the groups.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    // Four workers whatever the machine's cores, so a degree of 4 is not
    // cut to fewer.
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
           CREATE TABLE C (K INTEGER PRIMARY KEY, G INTEGER, S VARCHAR(20), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE D (G INTEGER, NAME VARCHAR(10))",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500)
                .map(|k| {
                    let s = if k % 11 == 0 { "NULL".to_owned() } else { format!("'s{}'", k % 97) };
                    let n = if k % 13 == 0 { "NULL".to_owned() } else { (k * 7919 % 1000).to_string() };
                    format!("({k}, {}, {s}, {n})", k % 7)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
        db.maintain().unwrap();
        a.run(&format!("UPDATE {table} SET N = -N, S = 'changed' WHERE K BETWEEN 100 AND 150")).unwrap();
        a.run(&format!("DELETE FROM {table} WHERE K BETWEEN 3000 AND 3100")).unwrap();
        a.commit().unwrap();
    }
    a.run("INSERT INTO D VALUES (0, 'zero'), (1, 'one'), (2, 'two'), (2, 'deux'), (3, 'three'), (5, 'five')").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Asks for parallel parts of any size, on 4 threads, in morsels of 256
/// rows, so a table of `ROWS` rows is read in many.
fn parallel(s: &Session) {
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(s, set).unwrap();
    }
}

/// Rows alone, rows in parallel, and whether the plan has a parallel part.
type Both = (Result<Vec<String>, Code>, Result<Vec<String>, Code>, bool);

/// The rows of `sql`, in the order given, run alone (EXECUTOR.DEGREE 1) a
/// row at a time, as a batch gives its rows in no order (RULES.md D31);
/// then in parallel, of the column store too, which a scan by batch keeps
/// serial; and whether its plan has a parallel part.
fn both(s: &Session, sql: &str) -> Both {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    parallel(s);
    let marked = s.physical_plan(sql, &Params::new()).unwrap().contains("Parallel  (degree=4)");
    (alone, rows(s, sql), marked)
}

/// Queries of one table `{t}`, each a part the planner marks.
const QUERIES: [&str; 17] = [
    "SELECT K, G, S, N FROM {t}",
    "SELECT K, N * 2, S FROM {t} WHERE G = 3 AND N > 10",
    "SELECT G, COUNT(*), COUNT(S), SUM(N), AVG(N), MIN(S), MAX(N) FROM {t} GROUP BY G",
    "SELECT COUNT(*), SUM(N), AVG(N), MIN(K), MAX(S) FROM {t}",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE G = 99",
    "SELECT S, COUNT(*) FROM {t} WHERE N > 500 GROUP BY S",
    // Of S grouped by an expression, no estimate of its groups keeps it
    // serial (RULES.md D355).
    "SELECT DISTINCT G, S || '' FROM {t}",
    "SELECT K, S, N FROM {t} ORDER BY S, N DESC",
    "SELECT K, G, S FROM {t} ORDER BY G",
    "SELECT K, N FROM {t} ORDER BY N, K FETCH FIRST 7 ROWS ONLY",
    "SELECT {t}.K, D.NAME FROM {t} JOIN D ON {t}.G = D.G WHERE {t}.N < 300",
    "SELECT K, G, ROW_NUMBER() OVER (PARTITION BY G ORDER BY K), SUM(N) OVER (PARTITION BY G ORDER BY K) FROM {t}",
    "SELECT K, G FROM {t} WHERE G = 1 UNION ALL SELECT K, G FROM {t} WHERE G = 2",
    "SELECT G, S FROM {t} UNION SELECT G, S FROM {t} WHERE N > 100",
    "SELECT G, S FROM {t} EXCEPT SELECT G, S FROM {t} WHERE N > 100",
    "SELECT G FROM {t} INTERSECT ALL SELECT G FROM {t} WHERE K < 3000",
    "SELECT G, N FROM {t} EXCEPT ALL SELECT G, N FROM {t} WHERE K < 3000",
];

/// Verifies: [DESIGN] RULES.md D290, D292 to D296 scans, filters,
/// projections, groupings, DISTINCT, sorts, a hash join's probe, window
/// functions of a partitioning, UNION [ALL], EXCEPT [ALL] and INTERSECT
/// [ALL], each run in parallel, give the rows they give run alone, in the
/// same order, of the row store and of the column store's groups.
#[test]
fn parallel_parts_give_the_rows_alone_gives() {
    let (_db, a) = tables();
    for table in ["R", "C"] {
        for query in QUERIES {
            let sql = query.replace("{t}", table);
            let (alone, together, marked) = both(&a, &sql);
            assert!(marked, "{sql} is not run in parallel");
            assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
            assert_eq!(alone, together, "{sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D292 a hash join whose built operand is a
/// table big enough is built by morsels, and probed by morsels, its rows
/// those of the join run alone, in order; and a grouping over the join.
#[test]
fn joins_build_and_probe_by_morsels() {
    let (_db, a) = tables();
    for sql in [
        "SELECT R.K, C.N, C.S FROM R JOIN C ON R.K = C.K WHERE R.G = 2",
        "SELECT R.G, COUNT(*), SUM(C.N) FROM R JOIN C ON R.K = C.K GROUP BY R.G",
        "SELECT R.K, D.NAME FROM R JOIN D ON R.G = D.G ORDER BY D.NAME, R.K DESC",
    ] {
        let (alone, together, marked) = both(&a, sql);
        assert!(marked, "{sql}");
        assert_eq!(alone, together, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D291 the planner leaves on the statement's
/// thread a point query, a part reading fewer rows than
/// EXECUTOR.PARALLEL_MIN_ROWS, a correlated subquery, its scan below it
/// run in parallel, a column store's part a scan by batch serves while
/// EXECUTOR.BATCHES is on, not its join nor its grouping (D358), a bare
/// COUNT(*) the row store counts (D351), and every part of
/// EXECUTOR.DEGREE 1.
///
/// Decision: the part is large enough, its expressions a worker's alone,
/// and the degree above 1. Each pair changes the outcome alone: a
/// subquery, rows below the least, degree 1.
#[test]
fn the_planner_marks_only_large_parts() {
    let (_db, a) = tables();
    let plan = |sql: &str| a.physical_plan(sql, &Params::new()).unwrap();
    parallel(&a);
    assert!(plan("SELECT SUM(N) FROM R").contains("Parallel"));
    assert!(!plan("SELECT COUNT(*) FROM R").contains("Parallel"), "a count the row store makes (D351)");
    assert!(!plan("SELECT N FROM R WHERE K = 5").contains("Parallel"));
    let correlated = plan("SELECT K, (SELECT COUNT(*) FROM D WHERE D.G = R.G) FROM R");
    assert!(correlated.starts_with("Project") && correlated.contains("  Parallel"), "a subquery's on the statement's thread: {correlated}");
    assert!(!plan("SELECT K, N FROM C WHERE G = 3").contains("Parallel"), "the column store's scan by batch");
    assert!(plan("SELECT COUNT(*) FROM C JOIN D ON C.G = D.G").contains("Parallel"), "a probe is not by batch");
    assert!(plan("SELECT G, COUNT(*) FROM C GROUP BY G").contains("Parallel"), "a grouping by morsels of frozen groups (D358)");
    assert!(plan("SELECT DISTINCT S FROM C").contains("Parallel"), "DISTINCT by morsels of frozen groups (D358)");
    exec(&a, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert!(plan("SELECT K, N FROM C WHERE G = 3").contains("Parallel"));
    exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 100000").unwrap();
    assert!(!plan("SELECT SUM(N) FROM R").contains("Parallel"));
    exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 0").unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    assert!(!plan("SELECT SUM(N) FROM R").contains("Parallel"));
}

/// Verifies: [DESIGN] RULES.md D290, D15 an error raised by a row of a
/// part run in parallel is the one the part raises alone, rows before it
/// given first; and a limit met stops the part before a row it never
/// reads raises.
#[test]
fn errors_are_those_alone_raises() {
    let (_db, a) = tables();
    for sql in
        ["SELECT K, 10 / (K - 4000) FROM R", "SELECT G, SUM(10 / (K - 4000)) FROM R GROUP BY G", "SELECT K FROM R ORDER BY 10 / (K - 4000)"]
    {
        let (alone, together, _) = both(&a, sql);
        assert!(alone.is_err(), "{sql}");
        assert_eq!(alone, together, "{sql}");
    }
    let (alone, together, _) = both(&a, "SELECT K, 10 / (K - 4000) FROM R WHERE G = 1 FETCH FIRST 3 ROWS ONLY");
    assert_eq!(alone.as_ref().map(Vec::len), Ok(3));
    assert_eq!(alone, together);
}

/// Verifies: [DESIGN] RULES.md D290, D29 a part run in parallel reads as
/// its statement's transaction does: its own writes not yet committed.
#[test]
fn parallel_parts_see_their_transactions_writes() {
    let (_db, a) = tables();
    a.run("INSERT INTO R VALUES (-1, 1, 'mine', 1), (-2, 2, 'mine', 2)").unwrap();
    a.run("DELETE FROM R WHERE K BETWEEN 10 AND 19").unwrap();
    for sql in ["SELECT COUNT(*), SUM(N) FROM R", "SELECT K FROM R WHERE S = 'mine'", "SELECT G, COUNT(*) FROM R GROUP BY G"] {
        let (alone, together, marked) = both(&a, sql);
        assert!(marked, "{sql}");
        assert_eq!(alone, together, "{sql}");
    }
    let (_, together, _) = both(&a, "SELECT COUNT(*) FROM R");
    assert_eq!(together, Ok(vec![(ROWS - 101 + 2 - 10).to_string()]));
    a.rollback().unwrap();
}

/// Verifies: [DESIGN] RULES.md D293, D38 a grouping whose groups outgrow
/// its work area gives way to the serial grouping, which spills, its rows
/// those it gives alone.
#[test]
fn a_part_past_its_work_area_gives_way() {
    let (_db, a) = tables();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    // Grouped by expressions' values, of groups not estimated, so not kept serial
    // by their exchange (RULES.md D355).
    for sql in [
        "SELECT J, COUNT(*), SUM(N) FROM (SELECT K + 0 AS J, N FROM R) AS T GROUP BY J",
        "SELECT DISTINCT K + 0, S FROM C",
        "SELECT K, S FROM R ORDER BY S DESC, K",
    ] {
        let (alone, together, marked) = both(&a, sql);
        assert!(marked, "{sql}");
        // Given way, it runs by batch, its rows in no order.
        let sorted = |r: Result<Vec<String>, Code>| {
            r.map(|mut r| {
                r.sort();
                r
            })
        };
        assert_eq!(sorted(alone), sorted(together), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D297 an index range scan is cut at its
/// B-link tree's inner keys into morsels of 16 rows, its rows those the
/// scan gives alone, in key order, of a range and of a range filtered
/// further.
#[test]
fn index_ranges_split_into_morsels() {
    let (_db, a) = tables();
    for sql in ["SELECT K, N FROM R WHERE K BETWEEN 1000 AND 1150", "SELECT K, S FROM R WHERE K BETWEEN 2000 AND 2150 AND N > 100"] {
        exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
        let alone = rows(&a, sql);
        parallel(&a);
        exec(&a, "SET EXECUTOR.MORSEL_ROWS = 16").unwrap();
        let plan = a.physical_plan(sql, &Params::new()).unwrap();
        assert!(plan.contains("Parallel  (degree=4)") && plan.contains("IndexScan"), "{plan}");
        assert!(alone.as_ref().is_ok_and(|r| r.len() > 50), "{sql}");
        assert_eq!(alone, rows(&a, sql), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D296, D44 a statement running past its
/// STATEMENT.TIMEOUT_MS while its part runs in parallel is cancelled
/// (SQLCODE -920) as a morsel is taken up; the session's next statement
/// runs.
#[test]
fn a_cancelled_parallel_part_fails() {
    let (_db, a) = tables();
    parallel(&a);
    exec(&a, "SET STATEMENT.TIMEOUT_MS = 1").unwrap();
    let long = "SELECT X.G, COUNT(*) FROM R AS X JOIN R AS Y ON X.G = Y.G GROUP BY X.G";
    assert!(a.physical_plan(long, &Params::new()).unwrap().contains("Parallel"));
    assert_eq!(rows(&a, long).err(), Some(Code::Cancelled));
    exec(&a, "SET STATEMENT.TIMEOUT_MS = 0").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM R").map(|r| r.len()), Ok(1));
}

/// Verifies: [DESIGN] RULES.md D356, D357: groupings and DISTINCT of
/// dates, timestamps with time zones of one instant in two zones, truth
/// values, fixed-length texts with and without trailing spaces, decimals of
/// one value at two scales and doubles of -0 and 0, with nulls among them,
/// their groups spread over every morsel, give by the executor's table,
/// serial by batch and in parallel, its partitions merged at the end or in
/// waves, the rows and order a row at a time gives, each group's values
/// its first row's.
#[test]
fn groups_of_every_type_come_as_a_row_at_a_time_gives() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION T
           CREATE TABLE V (K INTEGER, D DATE, Z TIMESTAMP WITH TIME ZONE, B BOOLEAN, C CHARACTER(6),
             E DECIMAL(9, 2), X DOUBLE PRECISION)",
    )
    .unwrap();
    let t = db.session("T");
    for first in (0..4000_i64).step_by(500) {
        let values: Vec<String> = (first..first + 500)
            .map(|k| {
                let null = |v: String| if k % 17 == 0 { "NULL".to_owned() } else { v };
                let zone = if k % 2 == 0 { "TIMESTAMP '2024-01-01 10:00:00+00:00'" } else { "TIMESTAMP '2024-01-01 12:00:00+02:00'" };
                let text = if k % 3 == 0 { format!("'c{}  '", k % 40) } else { format!("'c{}'", k % 40) };
                let decimal = if k % 2 == 0 { format!("{}.5", k % 300) } else { format!("{}.50", k % 300) };
                let double = if k % 2 == 0 { "0.0E0" } else { "-0.0E0" };
                format!(
                    "({k}, {}, {}, {}, {}, {}, {})",
                    null(format!("DATE '2024-01-01' + INTERVAL '{}' DAY(3)", k % 365)),
                    null(zone.to_owned()),
                    null(if k % 5 == 0 { "TRUE".to_owned() } else { "FALSE".to_owned() }),
                    null(text),
                    null(decimal),
                    null(double.to_owned()),
                )
            })
            .collect();
        t.run(&format!("INSERT INTO V VALUES {}", values.join(", "))).unwrap();
    }
    t.commit().unwrap();
    for sql in [
        "SELECT D, COUNT(*), SUM(E), MIN(K) FROM V GROUP BY D",
        "SELECT Z, B, COUNT(*), MAX(E), AVG(X) FROM V GROUP BY Z, B",
        "SELECT C, E, COUNT(K), SUM(X), MIN(C) FROM V GROUP BY C, E",
        "SELECT X, COUNT(*) FROM V GROUP BY X",
        "SELECT DISTINCT C, B FROM V",
        "SELECT DISTINCT E, D FROM V",
        "SELECT COUNT(DISTINCT C), COUNT(DISTINCT E), SUM(DISTINCT E) FROM V",
    ] {
        let (alone, together, _) = both(&t, sql);
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        assert_eq!(alone, together, "{sql}, in parallel");
        exec(&t, "SET EXECUTOR.DEGREE = 1").unwrap();
        exec(&t, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        assert_eq!(alone, rows(&t, sql), "{sql}, serial by batch");
        parallel(&t);
        assert_eq!(alone, rows(&t, sql), "{sql}, in parallel by batch");
        // Merged in waves, a few morsels' groups at a time, or given way.
        exec(&t, "SET MEMORY.WORK_BYTES = 262144").unwrap();
        assert_eq!(alone, rows(&t, sql), "{sql}, in parallel by batch, in waves");
        exec(&t, "SET MEMORY.WORK_BYTES TO DEFAULT").unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D356, D358: a grouping in parallel whose
/// first morsels hold few groups, merged on the statement's thread, and
/// whose later ones hold many, handed to the partitions, or the other way
/// about, gives the groups, their set functions and their order a row at a
/// time gives; and of the column store, grouped by batch in parallel, each
/// morsel of its frozen groups, by a column kept as runs too, the same
/// groups, in the order its scan by batch gives its rows.
#[test]
fn groupings_of_few_then_many_groups_come_as_alone() {
    let (db, a) = tables();
    a.run("CREATE TABLE F (K INTEGER, G INTEGER, H INTEGER, X DOUBLE PRECISION)").unwrap();
    a.run("CREATE TABLE O (K INTEGER, G INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'G')").unwrap();
    for first in (0..8000_i64).step_by(500) {
        let few = |k: i64, before: bool| if (k < 4000) == before { k % 3 } else { k };
        let values: Vec<String> =
            (first..first + 500).map(|k| format!("({k}, {}, {}, {}.5E0)", few(k, true), few(k, false), k % 7)).collect();
        a.run(&format!("INSERT INTO F VALUES {}", values.join(", "))).unwrap();
        let values: Vec<String> = (first..first + 500).map(|k| format!("({k}, {})", k % 300)).collect();
        a.run(&format!("INSERT INTO O VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    for sql in [
        "SELECT G, COUNT(*), MIN(K), MAX(X) FROM F GROUP BY G",
        "SELECT H, COUNT(*), MIN(K), MAX(X) FROM F GROUP BY H",
        "SELECT G, COUNT(*), MAX(K) FROM F WHERE K >= 2000 OR MOD(K, 2) = 0 GROUP BY G",
        "SELECT DISTINCT H FROM F WHERE K < 1000 OR K >= 7000",
        "SELECT G, COUNT(*), SUM(K) FROM O GROUP BY G",
        "SELECT DISTINCT G FROM O WHERE K > 100",
        "SELECT G, COUNT(*), SUM(K), AVG(N) FROM C GROUP BY G",
        "SELECT COUNT(DISTINCT S), SUM(N) FROM C",
    ] {
        let (alone, _, _) = both(&a, sql);
        exec(&a, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
        let marked = a.physical_plan(sql, &Params::new()).unwrap().contains("Parallel  (degree=4)");
        assert!(marked, "{sql} is not run in parallel");
        assert!(alone.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {alone:?}");
        // The column store's scan by batch reads its frozen groups, O's
        // sorted by G, then its rows changed; a row at a time reads them in
        // the order of their ids.
        let ordered = |r: Result<Vec<String>, Code>| {
            r.map(|mut r| {
                if !sql.contains("FROM F ") {
                    r.sort();
                }
                r
            })
        };
        assert_eq!(ordered(alone), ordered(rows(&a, sql)), "{sql}, in parallel by batch");
    }
}
