//! \[EXTENSION\] EXPLAIN ANALYZE (RULES.md D376, D378): a query run, every
//! operator of its plan showing the rows it gave beside its estimate, on
//! the row path, by batches and by morsels on several threads; the
//! statement's place among direct statements; and cardinalities a session
//! gives the planner in place of its estimates (D377). Every database
//! here reads a clock that stands still, or one that moves a microsecond
//! at each reading, so no test reads the system's (RULES.md R-TEST-5).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use turborel::clock::{Clock, FixedClock};
use turborel::db::{Database, Direct, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::physical::{Analyzed, Injected, Operator};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows, sorted};

mod changes;
mod notes;

/// A join whose few outer rows each probe T's index.
const PROBED: &str = "SELECT T.V, U.W FROM T, U WHERE T.K = U.K AND U.W = 3";

/// Rows of T: K and V each its place.
const T_ROWS: i64 = 1000;
/// Rows of U, K each tenth key of T, and of S, K each of the first keys.
const KEYS: i64 = 100;

/// A clock a microsecond later at each reading, so times are counted
/// alike at every run.
#[derive(Default)]
struct Ticking(AtomicI64);

impl Clock for Ticking {
    fn now(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// Tables T, indexed by K, U keyed by K, S, ANALYZE and EXPLAIN, of ANN,
/// their statistics gathered, read by a clock that stands still.
fn database() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_clock(Arc::new(FixedClock::default())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION ANN
           CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL PRIMARY KEY, W INTEGER)
           CREATE TABLE S (K INTEGER NOT NULL, X INTEGER)
           CREATE TABLE ANALYZE (A INTEGER)
           CREATE TABLE EXPLAIN (C INTEGER)",
    )
    .unwrap();
    let s = db.session("ANN");
    let values = |n: i64, f: &dyn Fn(i64) -> String| (0..n).map(f).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO T VALUES {}", values(T_ROWS, &|i| format!("({i}, {i})")))).unwrap();
    s.run(&format!("INSERT INTO U VALUES {}", values(KEYS, &|i| format!("({}, {})", i * 10, i % 7)))).unwrap();
    s.run(&format!("INSERT INTO S VALUES {}", values(KEYS, &|i| format!("({i}, {})", i % 3)))).unwrap();
    s.run("INSERT INTO ANALYZE VALUES (1), (2)").unwrap();
    s.run("INSERT INTO EXPLAIN VALUES (3)").unwrap();
    s.commit().unwrap();
    db.create_index("T_K", "ANN.T", &["K"], "btree").unwrap();
    // Planned once, the index's keys are asked for, and maintenance counts
    // them, so a probe is priced at the rows of one key.
    s.physical_plan(PROBED, &Params::new()).unwrap();
    db.maintain().unwrap();
    db
}

/// What EXPLAIN ANALYZE found of `sql`.
fn analyzed(s: &Session, sql: &str) -> Analyzed {
    s.analyzed(sql, &Params::new()).unwrap()
}

/// The first operator of `a` whose line begins with `word`.
fn op<'a>(a: &'a Analyzed, word: &str) -> &'a Operator {
    a.operators.iter().find(|o| o.label.starts_with(word)).unwrap_or_else(|| panic!("no {word} in\n{}", a.text))
}

/// The lines of a direct statement's rows.
fn direct_lines(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    match s.direct(sql).map_err(|e| e.code)? {
        Direct::Rows(mut cursor) => {
            let mut lines = Vec::new();
            while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
                lines.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
            }
            Ok(lines)
        }
        Direct::Done(_) => Ok(Vec::new()),
    }
}

/// Verifies: [EXTENSION] RULES.md D376: EXPLAIN ANALYZE of a table's scan
/// reads every row, its scan and projection each counted once, the
/// scan's estimate its table's rows, by batches and a row at a time; its
/// last line the query's rows; and no time while the clock stands still.
#[test]
fn a_scan_shows_its_rows_beside_its_estimate() {
    let db = database();
    let s = db.session("ANN");
    for batches in ["ON", "OFF"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let a = analyzed(&s, "SELECT K FROM T");
        let scan = op(&a, "SeqScan ANN.T");
        assert_eq!((a.rows, scan.rows, scan.loops, scan.estimate), (1000, 1000, 1, Some(1000.0_f64)), "{batches}");
        assert_eq!(op(&a, "Project").rows, 1000, "{batches}");
        assert!(a.text.starts_with("-- physical:\n") && a.text.ends_with("-- ran: 1000 rows\n"), "{}", a.text);
        assert!(a.micros.is_none() && !a.text.contains("time="), "{}", a.text);
        let line = a.text.lines().find(|l| l.contains("SeqScan ANN.T  (rows=1000, cost=")).unwrap_or_else(|| panic!("{}", a.text));
        assert!(line.contains(")  (actual rows=1000"), "{line}");
    }
}

/// Verifies: [EXTENSION] RULES.md D376: a join's operators each give the
/// rows the query gives at their place, and an index nested loop shows
/// its probes, one for each row of its outer input.
#[test]
fn a_join_counts_each_operator_and_an_index_join_its_probes() {
    let db = database();
    let s = db.session("ANN");
    let a = analyzed(&s, PROBED);
    assert_eq!(a.rows, 14);
    let join = op(&a, "IndexNestedLoop ANN.T");
    assert_eq!((join.rows, join.tables, join.join), (14, 2, true), "{}", a.text);
    assert!(a.text.contains("(actual rows=14, probes=14, entries=14"), "{}", a.text);
    let hashed = analyzed(&s, "SELECT T.V, S.X FROM T, S WHERE T.K = S.K AND S.X = 1");
    let join = hashed.operators.iter().find(|o| o.join).unwrap();
    assert_eq!((hashed.rows, join.rows), (33, 33), "{}", hashed.text);
}

/// Verifies: [EXTENSION] RULES.md D376: a FETCH FIRST that stops early
/// shows a scan that gave fewer rows than its estimate.
#[test]
fn a_limit_that_stops_early_reads_less_than_estimated() {
    let db = database();
    let s = db.session("ANN");
    let a = analyzed(&s, "SELECT V FROM T FETCH FIRST 3 ROWS ONLY");
    let scan = op(&a, "SeqScan ANN.T");
    assert_eq!(a.rows, 3);
    assert!(scan.rows < 1000 && scan.estimate == Some(1000.0_f64), "{}", a.text);
}

/// Verifies: [EXTENSION] RULES.md D376, D15: a query that raises makes
/// EXPLAIN ANALYZE raise its exception, by the library and as a direct
/// statement.
#[test]
fn a_query_that_raises_raises_the_same_exception() {
    let db = database();
    let s = db.session("ANN");
    let sql = "SELECT V / (K - K) FROM T";
    let run = rows(&s, sql).unwrap_err();
    assert_eq!(s.analyzed(sql, &Params::new()).map_err(|e| e.code).unwrap_err(), run);
    s.rollback().unwrap();
    assert_eq!(direct_lines(&s, &format!("EXPLAIN ANALYZE {sql}")).unwrap_err(), run);
}

/// Verifies: [EXTENSION] RULES.md D376, D622: ANALYZE is no key word. A
/// table named ANALYZE is read, explained and analyzed; EXPLAIN ANALYZE
/// followed by a parenthesis that begins no query is a table EXPLAIN
/// whose correlation name is ANALYZE; and EXPLAIN of a data change
/// statement shows it, changing nothing (D980).
#[test]
fn analyze_is_read_as_the_option_only_before_a_query() {
    let db = database();
    let s = db.session("ANN");
    assert_eq!(sorted(&s, "SELECT A FROM ANALYZE"), ["1", "2"]);
    assert!(direct_lines(&s, "EXPLAIN SELECT A FROM ANALYZE").unwrap().iter().any(|l| l.contains("SeqScan ANN.ANALYZE")));
    let lines = direct_lines(&s, "EXPLAIN ANALYZE SELECT A FROM ANALYZE").unwrap();
    assert_eq!(lines.last().map(String::as_str), Some("-- ran: 2 rows"));
    let joined = "EXPLAIN ANALYZE (D) CROSS JOIN ANALYZE";
    assert_eq!(direct_lines(&s, joined), rows(&s, &format!("SELECT * FROM {joined}")));
    assert_eq!(direct_lines(&s, "EXPLAIN INSERT INTO T VALUES (1, 1)").unwrap().get(1).map(String::as_str), Some("Insert into ANN.T"));
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["1000"]);
}

/// Verifies: [EXTENSION] RULES.md D376, D378, D290: of a part run in
/// parallel on four threads, its operators' rows are summed over the
/// threads, by batches and a row at a time, and are those of the plan run
/// alone; the part's top operator is counted, the part's mark not.
#[test]
fn a_part_run_in_parallel_sums_its_threads_counts() {
    let db = database();
    // Four workers whatever the machine's cores, so a degree of 4 is not cut.
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let s = db.session("ANN");
    for set in ["SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 64", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"] {
        exec(&s, set).unwrap();
    }
    let sql = "SELECT COUNT(*), SUM(V) FROM T WHERE V >= 10";
    for batches in ["ON", "OFF"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let a = analyzed(&s, sql);
        assert!(a.text.contains("Parallel  (degree=4)\n"), "{}", a.text);
        assert_eq!((a.rows, op(&a, "Aggregate").rows), (1, 1), "{batches}: {}", a.text);
        let filtered = a.operators.iter().filter(|o| o.label.starts_with("Filter") || o.label.starts_with("SeqScan"));
        assert!(filtered.clone().any(|o| o.rows == 990), "{batches}: {}", a.text);
        assert!(filtered.into_iter().all(|o| o.rows >= 990 && o.micros.is_none()), "{batches}: {}", a.text);
    }
}

/// Verifies: [EXTENSION] RULES.md D378, D351: an operator never opened is
/// shown so: a scan whose table's storage counted its rows.
#[test]
fn an_operator_never_run_is_shown_so() {
    let db = database();
    let s = db.session("ANN");
    let a = analyzed(&s, "SELECT COUNT(*) FROM T");
    assert!(a.text.contains("(never run)") && a.text.contains("-- counted by storage: ANN.T"), "{}", a.text);
    assert_eq!(op(&a, "SeqScan").loops, 0);
}

/// Verifies: [EXTENSION] RULES.md D378, D54: times are those of the
/// database's clock, a statement's at least its top operator's, an
/// operator's at least each of its inputs', shown on each line once the
/// clock moves.
#[test]
fn times_are_the_clocks() {
    let db = database();
    db.set_clock(Arc::new(Ticking::default())).unwrap();
    let s = db.session("ANN");
    let a = analyzed(&s, "SELECT T.V FROM T, S WHERE T.K = S.K AND S.X = 2");
    let micros: Vec<u64> = a.operators.iter().map(|o| o.micros.unwrap_or_else(|| panic!("{}", a.text))).collect();
    let top = micros.first().copied().unwrap_or_default();
    assert!(a.micros.unwrap() >= top && micros.iter().all(|&m| m <= top), "{}", a.text);
    assert!(a.text.contains(" ms)") && a.text.lines().last().unwrap().contains(" ms"), "{}", a.text);
}

/// Verifies: [EXTENSION] RULES.md D377, D302: rows a session gives in place
/// of the estimates of a set of relations change the plan chosen for a
/// join of three tables, a join's alone among them, and the estimate
/// shown, but not its rows; a set naming no relation of the query changes
/// nothing; none given, the estimates come back.
#[test]
fn injected_cardinalities_change_the_plan_not_the_rows() {
    let db = database();
    let s = db.session("ANN");
    let sql = "SELECT T.V, U.W, S.X FROM T, U, S WHERE T.K = U.K AND U.K = S.K AND S.X = 1";
    let plan = |s: &Session| s.physical_plan(sql, &Params::new()).unwrap();
    let (before, rows_before) = (plan(&s), sorted(&s, sql));
    let shown = |s: &Session| analyzed(s, sql).operators.iter().map(|o| o.estimate).collect::<Vec<_>>();
    let unchanged = shown(&s);
    s.with_cardinalities(Injected::new().with(&["NOWHERE"], 5.0_f64));
    assert_eq!(plan(&s), before);
    // A join's rows alone, which the join enumeration takes.
    s.with_cardinalities(Injected::new().with(&["S", "U"], 1.0e7_f64));
    assert_ne!(plan(&s), before);
    s.with_cardinalities(Injected::new().with(&["T"], 1.0e7_f64).with(&["S"], 1.0e7_f64).with(&["S", "U"], 1.0e7_f64));
    let injected = plan(&s);
    assert_ne!(injected, before);
    assert_eq!(sorted(&s, sql), rows_before);
    // The estimates shown come of the given rows, joined by the keys of
    // the relations (RULES.md D384), not of the statistics'.
    assert_ne!(shown(&s), unchanged, "{}", analyzed(&s, sql).text);
    s.with_cardinalities(Injected::new());
    assert_eq!(plan(&s), before);
}
