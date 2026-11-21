//! A correlated subquery's plan kept across its runs (RULES.md D675): the
//! rows it gives are those it gives planned at each run with its outer
//! values as constants (EXECUTOR.KEEP_SUBQUERY_PLANS OFF), whatever its
//! values, nulls among them, its runs spilling or cancelled.

use std::sync::{Arc, Mutex};

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::memory::MemoryBudget;
use turborel::optimizer::UNNEST_SUBQUERIES;
use turborel::spill::{Area, PagedSpill, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles};
use turborel::txn::Stop;

use crate::common::{exec, rows, set_rule, sorted};
use crate::spilling::{SMALL, bytes, filled};

/// Outer rows: more than the runs a subquery learns its plan over, and
/// than those after which it is decorrelated.
const OUTER: usize = 300;

/// Schema `P`: `T` of `OUTER` rows, its K of 97 values, a null every 13th
/// and in the first row, so a subquery's first run has a null; D of
/// decimals; C of two or three letters and digits, of either case; F
/// approximate; TS a timestamp of no fraction. `U` of twice as many, its K
/// of 120 values, a null every 17th; D of another scale than T's; C, CI
/// case-insensitive, of other lengths; TS of a microsecond's precision;
/// DT a date; each indexed.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION P
           CREATE TABLE T (K INTEGER, D DECIMAL(5,1), C CHARACTER(3), F REAL, TS TIMESTAMP(0))
           CREATE TABLE U (K INTEGER, D DECIMAL(5,2), C VARCHAR(3), W INTEGER, CI VARCHAR(3) COLLATE CASE_INSENSITIVE,
             TS TIMESTAMP(6), DT DATE)",
    )
    .unwrap();
    let s = db.session("P");
    let null_or = |n: usize, every: usize, v: String| if n.is_multiple_of(every) { "NULL".to_owned() } else { v };
    let letter = |n: usize, every: usize| if n.is_multiple_of(every) { "a" } else { "A" };
    let t: Vec<String> = (0..OUTER)
        .map(|n| {
            let (k, c) = (null_or(n, 13, (n % 97).to_string()), format!("{}{}", letter(n, 2), n % 11));
            format!("({k}, {}.{}, '{c}', {}E0, TIMESTAMP '2024-01-{:02} {:02}:00:00')", n % 41, n % 10, n % 89, 1 + n % 28, n % 3)
        })
        .collect();
    let u: Vec<String> = (0..2 * OUTER)
        .map(|n| {
            let (k, c) = (null_or(n, 17, (n % 120).to_string()), format!("{}{}", letter(n, 3), n % 13));
            let at = format!("2024-01-{:02} {:02}:00:00{}", 1 + n % 28, n % 3, if n % 5 == 0 { ".000001" } else { ".000000" });
            let date = format!("2024-01-{:02}", 1 + n % 30);
            format!("({k}, {}.{:02}, '{c}', {}, '{c}', TIMESTAMP '{at}', DATE '{date}')", n % 41, (n * 7) % 100, n % 50)
        })
        .collect();
    s.run(&format!("INSERT INTO T VALUES {}", t.join(", "))).unwrap();
    s.run(&format!("INSERT INTO U VALUES {}", u.join(", "))).unwrap();
    s.commit().unwrap();
    for column in ["K", "C", "D", "CI", "TS", "DT"] {
        s.run(&format!("CREATE INDEX U{column} ON U ({column})")).unwrap();
        s.commit().unwrap();
    }
    set_rule(&db, UNNEST_SUBQUERIES, false);
    db
}

/// Correlated subqueries of each kind of outer value: scalar set functions
/// by an index's equality and range, EXISTS by an inequality, IN, ALL, two
/// outer references, a nested subquery of the outer one's; an integer
/// column compared with decimals and a decimal one with integers, an
/// approximate value with an integer column; strings of varying lengths,
/// and compared case-insensitively; timestamps of another precision than
/// the column's, a date of a timestamp, and nulls in the first runs then
/// values.
const QUERIES: &[&str] = &[
    "SELECT K, (SELECT COUNT(*) FROM U WHERE U.K = T.K) FROM T",
    "SELECT C, (SELECT MIN(W) FROM U WHERE U.C = T.C) FROM T",
    "SELECT D, (SELECT AVG(W) FROM U WHERE U.D = T.D) FROM T",
    "SELECT D, (SELECT COUNT(*) FROM U WHERE U.D < T.D AND U.K > 100) FROM T",
    "SELECT K FROM T WHERE EXISTS (SELECT * FROM U WHERE U.K > T.K AND U.W < T.K)",
    "SELECT K FROM T WHERE K IN (SELECT W FROM U WHERE U.K = T.K + 1)",
    "SELECT K FROM T WHERE K >= ALL (SELECT W FROM U WHERE U.K = T.K)",
    "SELECT K, C, (SELECT SUM(W) FROM U WHERE U.K = T.K AND U.C = T.C) FROM T",
    "SELECT K, (SELECT COUNT(*) FROM U WHERE U.K = T.K AND EXISTS (SELECT * FROM U U2 WHERE U2.W = U.W AND U2.K <> T.K)) FROM T",
    "SELECT F, (SELECT COUNT(*) FROM U WHERE U.K = T.F) FROM T",
    "SELECT D, (SELECT COUNT(*) FROM U WHERE U.K = T.D) FROM T",
    "SELECT K, (SELECT COUNT(*) FROM U WHERE U.D = T.K) FROM T",
    "SELECT K, D, (SELECT COUNT(*) FROM U WHERE U.D = CASE WHEN T.K > 40 THEN T.D ELSE T.K END) FROM T",
    "SELECT C, (SELECT COUNT(*) FROM U WHERE U.C = TRIM(T.C)) FROM T",
    "SELECT C, (SELECT COUNT(*) FROM U WHERE U.CI = TRIM(T.C)) FROM T",
    "SELECT C, (SELECT COUNT(*) FROM U WHERE U.C = TRIM(T.C) COLLATE CASE_INSENSITIVE) FROM T",
    "SELECT TS, (SELECT COUNT(*) FROM U WHERE U.TS = T.TS) FROM T",
    "SELECT TS, (SELECT COUNT(*) FROM U WHERE U.DT = CAST(T.TS AS DATE)) FROM T",
    "SELECT K, (SELECT COUNT(*) FROM U WHERE U.TS > CASE WHEN T.K > 40 THEN T.TS END) FROM T",
];

/// The rows of `sql`, sorted, with EXECUTOR.KEEP_SUBQUERY_PLANS `keep`.
fn kept_or_not(s: &Session, sql: &str, keep: &str) -> Vec<String> {
    exec(s, &format!("SET EXECUTOR.KEEP_SUBQUERY_PLANS = '{keep}'")).unwrap();
    let found = sorted(s, sql);
    exec(s, "SET EXECUTOR.KEEP_SUBQUERY_PLANS = 'ON'").unwrap();
    found
}

/// Verifies: [DESIGN] RULES.md D675: a correlated subquery whose plan is
/// kept across its runs gives, for each of many outer values, nulls
/// among them and first, of integers, decimals of a scale not their
/// column's, strings of varying lengths, compared case-insensitively too,
/// approximate numbers, and timestamps and dates of another precision or
/// kind than their column's, the rows it gives planned at each run with
/// its values as constants: decorrelated after its default runs, and
/// never.
#[test]
fn kept_subquery_plans_give_what_each_run_planned_gives() {
    let db = database();
    let s = db.session("P");
    for after in [8, 1_000_000] {
        db.set_setting("EXECUTOR.DECORRELATE_AFTER_RUNS", &crate::common::number(after)).unwrap();
        for sql in QUERIES {
            let planned = kept_or_not(&s, sql, "OFF");
            assert!(!planned.is_empty(), "{sql}");
            assert_eq!(kept_or_not(&s, sql, "ON"), planned, "{sql}, after {after}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38, D675: with MEMORY.WORK_BYTES at its
/// least, a correlated subquery whose plan is kept, its rows for each
/// outer value spilling, gives the rows it gives in memory and planned at
/// each run.
#[test]
fn kept_subquery_plans_spill_as_planned_ones() {
    const SPILLED: &str = "SELECT K FROM T WHERE K < 40 AND K * 7 IN (SELECT G FROM U WHERE U.K >= T.K)";
    let db = Database::new();
    filled(&db);
    set_rule(&db, UNNEST_SUBQUERIES, false);
    let s = db.session("S");
    let in_memory = kept_or_not(&s, SPILLED, "OFF");
    assert!(!in_memory.is_empty());
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    assert_eq!(kept_or_not(&s, SPILLED, "ON"), in_memory);
    assert_eq!(kept_or_not(&s, SPILLED, "OFF"), in_memory);
}

/// Verifies: [DESIGN] RULES.md D44, D675: a statement cancelled as a run of
/// its correlated subquery, by the plan kept, reads its spilled rows back
/// fails (SQLCODE -920), and the session's next statement gives the rows
/// it gives uncancelled.
#[test]
fn a_cancel_stops_a_run_of_a_kept_subquery_plan() {
    const SPILLED: &str = "SELECT K FROM T WHERE K < 40 AND K * 7 IN (SELECT G FROM U WHERE U.K >= T.K)";
    let db = Database::new();
    filled(&db);
    set_rule(&db, UNNEST_SUBQUERIES, false);
    let s = db.session("S");
    let expected = sorted(&s, SPILLED);
    let cancelling = CancelAt::default();
    db.set_spill(Box::new(cancelling.clone())).unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    cancelling.arm(&s, READ_BACKS);
    assert_eq!(rows(&s, SPILLED).err(), Some(Code::Cancelled));
    assert!(cancelling.fired());
    assert_eq!(sorted(&s, SPILLED), expected, "the next statement runs");
}

/// The spilled runs read back before the cancel: past the runs a
/// subquery learns its plan over, so the run cancelled is by the plan kept.
const READ_BACKS: usize = 20;

/// The session's canceller, and the read-backs left before it cancels.
type Armed = Arc<Mutex<(usize, Option<Arc<Stop>>)>>;

/// The built-in provider, whose runs, once armed (`CancelAt::arm`), cancel
/// what the session runs when as many runs as it was armed with are read
/// back.
#[derive(Clone, Default)]
struct CancelAt(Armed);

impl CancelAt {
    fn arm(&self, s: &Session, after: usize) {
        *self.0.lock().unwrap() = (after, Some(s.canceller()));
    }

    fn fired(&self) -> bool {
        self.0.lock().unwrap().1.is_none()
    }
}

impl Spill for CancelAt {
    fn name(&self) -> &'static str {
        "cancel-at"
    }
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        Ok(Box::new(CancelAtArea(Spill::area(&PagedSpill, memory, temp, area)?, Arc::clone(&self.0))))
    }
}

struct CancelAtArea(Box<dyn SpillArea>, Armed);

impl SpillArea for CancelAtArea {
    fn table(&self) -> turborel::error::Result<Box<dyn SpillTable>> {
        self.0.table()
    }
    fn run(&self) -> turborel::error::Result<Box<dyn SpillRun>> {
        Ok(Box::new(CancelAtRun(self.0.run()?, Arc::clone(&self.1))))
    }
}

struct CancelAtRun(Box<dyn SpillRun>, Armed);

/// Counts a read-back of `armed`, cancelling at the last.
fn read_back(armed: &Armed) {
    let mut armed = armed.lock().unwrap();
    if armed.1.is_none() {
        return;
    }
    armed.0 = armed.0.saturating_sub(1);
    if armed.0 == 0 {
        armed.1.take().unwrap().cancel();
    }
}

impl SpillRun for CancelAtRun {
    fn push(&mut self, record: &[u8]) -> turborel::error::Result<()> {
        self.0.push(record)
    }
    fn records(&mut self) -> turborel::error::Result<Records> {
        read_back(&self.1);
        self.0.records()
    }
    fn into_records(self: Box<Self>) -> turborel::error::Result<Records> {
        read_back(&self.1);
        self.0.into_records()
    }
}
