//! \[DESIGN\] Queries run in parallel by morsels (RULES.md D290 to D296):
//! each part the planner marks gives the rows the plan run alone gives, in
//! its order, and raises its errors, of either engine, frozen rows changed
//! among them; the planner marks only what reads enough rows; and a
//! statement sees its transaction's own writes.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows};

/// The rows written to each of R and C: K, a key; G, one of 7 groups; S, a
/// text, null for every 11th; N, a number, null for every 13th.
const ROWS: i64 = 6000;

/// Tables R, of the row store, and C, of the column store in groups of 2048,
/// alike, written, frozen, then changed; and D, a dimension of the groups.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
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

/// The rows of `sql`, in the order given, run alone (EXECUTOR.DEGREE 1) a
/// row at a time, as a batch gives its rows in no order (RULES.md D31);
/// then in parallel; and whether its plan has a parallel part.
fn both(s: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>, bool) {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let alone = rows(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    parallel(s);
    let marked = s.physical_plan(sql, &Params::new()).unwrap().contains("Parallel  (degree=4)");
    (alone, rows(s, sql), marked)
}

/// Queries of one table `{t}`, each a part the planner marks.
const QUERIES: [&str; 16] = [
    "SELECT K, G, S, N FROM {t}",
    "SELECT K, N * 2, S FROM {t} WHERE G = 3 AND N > 10",
    "SELECT G, COUNT(*), COUNT(S), SUM(N), AVG(N), MIN(S), MAX(N) FROM {t} GROUP BY G",
    "SELECT COUNT(*), SUM(N), AVG(N), MIN(K), MAX(S) FROM {t}",
    "SELECT COUNT(*), SUM(N) FROM {t} WHERE G = 99",
    "SELECT S, COUNT(*) FROM {t} WHERE N > 500 GROUP BY S",
    "SELECT DISTINCT G, S FROM {t}",
    "SELECT K, S, N FROM {t} ORDER BY S, N DESC",
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
/// EXECUTOR.PARALLEL_MIN_ROWS, and every part of EXECUTOR.DEGREE 1.
///
/// Decision: the part is large enough and the degree above 1. Each pair
/// changes the outcome alone: rows below the least, degree 1.
#[test]
fn the_planner_marks_only_large_parts() {
    let (_db, a) = tables();
    let plan = |sql: &str| a.physical_plan(sql, &Params::new()).unwrap();
    parallel(&a);
    assert!(plan("SELECT COUNT(*) FROM R").contains("Parallel"));
    assert!(!plan("SELECT N FROM R WHERE K = 5").contains("Parallel"));
    exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 100000").unwrap();
    assert!(!plan("SELECT COUNT(*) FROM R").contains("Parallel"));
    exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 0").unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    assert!(!plan("SELECT COUNT(*) FROM R").contains("Parallel"));
}

/// Verifies: [DESIGN] RULES.md D290, D15 an error raised by a row of a
/// part run in parallel is the one the part raises alone, rows before it
/// given first; and a limit met stops the part before a row it never
/// reads raises.
#[test]
fn errors_are_those_alone_raises() {
    let (_db, a) = tables();
    for sql in ["SELECT K, 10 / (K - 4000) FROM R", "SELECT G, SUM(10 / (K - 4000)) FROM R GROUP BY G", "SELECT K FROM R ORDER BY 10 / (K - 4000)"] {
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
    for sql in ["SELECT K, COUNT(*), SUM(N) FROM R GROUP BY K", "SELECT DISTINCT K, S FROM C", "SELECT K, S FROM R ORDER BY S DESC, K"] {
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
