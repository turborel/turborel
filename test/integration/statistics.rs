//! Statistics for the optimizer (RULES.md D31): the planner estimates the
//! rows a condition keeps from what a table's engine knows of its rows,
//! and from defaults where it knows nothing; the row store gathers its
//! statistics once a table opened is first asked for them, when it has
//! changed enough, and when told to, and an index counts its keys once
//! first asked for, both in maintenance; optimizer rules can read them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::engine::rows::{RowTable, TableFiles, TableOptions};
use turborel::error::{Code, Result, SqlError};
use turborel::index::{BTreeMethod, Entries, Index, IndexMethod, Key, KeyOrder, Probe};
use turborel::log::MemoryLog;
use turborel::logical::LogicalPlan;
use turborel::optimizer::{Rule, RuleContext};
use turborel::params::Params;
use turborel::storage::{RowId, TableStorage};
use turborel::threads::JobPolicy;
use turborel::txn::NO_TRANSACTION;
use turborel::value::{CompOp, DataType};

use crate::common::{Scratch, crc32c, locking_big, number, os, rows};
use turborel::value::Spelled;

/// The rows the Filter of `sql`'s physical plan is estimated to keep, if
/// it shows an estimate.
fn kept(s: &Session, sql: &str) -> Option<f64> {
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    let line = plan.lines().find(|l| l.trim_start().starts_with("Filter"))?;
    let est = line.split("est. rows=").nth(1)?;
    Some(est.trim_end_matches(')').parse().unwrap())
}

/// Whether `got` is within `by` of `want`.
fn near(got: Option<f64>, want: f64, by: f64) -> bool {
    got.is_some_and(|g| (g - want).abs() <= by)
}

/// A database whose table APP.T holds `n` rows, committed: ID 1 to `n`,
/// GRADE = ID mod 10, NOTE NULL for every fourth; its background work
/// done only when a test asks (JOBS.POLICY MANUAL, RULES.md D55).
fn graded(n: u32) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, GRADE INTEGER, NOTE CHAR(5))").unwrap();
    add(&db, 1..=n, |i| i % 10);
    db
}

/// Inserts rows of IDs `ids`, of GRADE `grade(ID)`, and commits; then
/// has the maintenance due done, statistics gathered among it.
fn add(db: &Database, ids: impl Iterator<Item = u32>, grade: impl Fn(u32) -> u32) {
    added(db, ids, grade);
    db.maintain().unwrap();
}

/// As `add`, the maintenance left to the database's thread.
fn added(db: &Database, ids: impl Iterator<Item = u32>, grade: impl Fn(u32) -> u32) {
    let s = db.session("APP");
    for i in ids {
        let note = if i % 4 == 0 { "NULL".to_string() } else { format!("'N{}'", i % 3) };
        s.run(&format!("INSERT INTO T VALUES ({i}, {}, {note})", grade(i))).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D31, estimates: with the row store's
/// statistics, EXPLAIN shows the rows a filter keeps: equality shares the
/// rows among the distinct values; an order follows the histogram; IS NULL
/// and IS NOT NULL the fraction NULL; OR, AND and NOT combine their parts
/// as independent; a comparison with the column on the right is turned
/// around; one the statistics cannot price takes a default.
#[test]
fn filters_are_estimated_from_statistics() {
    let db = graded(1000);
    let s = db.session("APP");
    let est = |cond: &str| kept(&s, &format!("SELECT ID FROM T WHERE {cond}"));
    assert!(near(est("GRADE = 3"), 100.0, 5.0), "{:?}", est("GRADE = 3"));
    assert!(near(est("GRADE <> 3"), 900.0, 5.0));
    assert!(near(est("ID < 250"), 250.0, 20.0), "{:?}", est("ID < 250"));
    assert!(near(est("250 > ID"), 250.0, 20.0), "turned around");
    assert!(near(est("ID >= 900"), 100.0, 20.0));
    assert!(near(est("NOTE IS NULL"), 250.0, 1.0));
    assert!(near(est("NOTE IS NOT NULL"), 750.0, 1.0));
    assert!(near(est("GRADE = 3 OR GRADE = 4"), 190.0, 10.0));
    assert!(near(est("GRADE = 3 AND ID < 500"), 50.0, 5.0));
    assert!(near(est("NOT GRADE = 3"), 900.0, 5.0));
    assert!(near(est("GRADE = 42"), 0.0, 0.0), "outside the values");
    assert!(near(est("GRADE = ID"), 10.0, 0.0), "a comparison of columns: one row in a hundred");
    assert!(near(est("GRADE < ID"), 1000.0 / 3.0, 1.0), "a third");
    assert!(near(est("GRADE <> ID"), 990.0, 0.0));
}

/// The rows the Filter of "SELECT ID FROM T WHERE ID > `id`" keeps.
fn above(db: &Database, id: u32) -> Option<f64> {
    kept(&db.session("APP"), &format!("SELECT ID FROM T WHERE ID > {id}"))
}

/// Verifies: [DESIGN] RULES.md D31, the row store's statistics are
/// gathered again, as the database's maintenance, once a transaction that
/// changed the table ends and it has changed by more than a tenth of its
/// rows plus 50, not before;
/// `Database::analyze` gathers them at once, and refuses a table that is
/// not "AUTH.TABLE" or does not exist.
#[test]
fn statistics_follow_the_table() {
    let db = graded(1000);
    assert!(near(above(&db, 1000), 0.0, 0.0), "no row above the greatest");
    add(&db, 1001..=1100, |_| 1);
    assert!(near(above(&db, 1000), 0.0, 0.0), "100 changes of 1,000 rows: not yet due");
    db.analyze("APP.T").unwrap();
    assert!(above(&db, 1000).is_some_and(|e| e > 50.0), "{:?}", above(&db, 1000));
    assert!(near(above(&db, 1100), 0.0, 0.0));
    add(&db, 1101..=1300, |_| 1);
    assert!(above(&db, 1100).is_some_and(|e| e > 100.0), "200 changes of 1,100 rows: gathered again");
    let s = db.session("APP");
    s.execute("DELETE FROM T WHERE ID > 300", &mut Params::new()).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    assert!(near(above(&db, 150), 150.0, 20.0), "1,000 rows deleted: gathered again");
    assert_eq!(db.analyze("T").map_err(|e| e.code), Err(Code::Api));
    assert_eq!(db.analyze("APP.NONE").map_err(|e| e.code), Err(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D31, statistics in the background: the
/// database's maintenance thread gathers a table's statistics once it has
/// changed enough, no call asking it to.
#[test]
fn statistics_are_gathered_in_the_background() {
    let db = graded(1000);
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    added(&db, 1001..=1300, |_| 1);
    let until = std::time::Instant::now().checked_add(std::time::Duration::from_secs(30)).unwrap();
    while above(&db, 1000).is_none_or(|e| e < 100.0) {
        assert!(std::time::Instant::now() < until, "gathered within 30 seconds");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Verifies: [DESIGN] RULES.md D31, estimates without statistics: for an
/// engine that gives none, a filter keeps a default fraction (one in a
/// hundred for equality and IS NULL, a third for an order); a filter over
/// an index's rows is estimated from the index's; a filter over a join
/// shows no estimate.
#[test]
fn filters_are_estimated_without_statistics() {
    let db = locking_big();
    let s = db.session("APP");
    let est = |cond: &str| kept(&s, &format!("SELECT ID FROM T WHERE {cond}"));
    assert!(near(est("F = 0"), 0.6, 0.5));
    assert!(near(est("F > 0"), 20.0, 0.5));
    assert!(near(est("NAME IS NULL"), 0.6, 0.5));
    assert!(near(est("ID = 7 AND F > 0"), 0.3, 0.5), "a third of the index's one row");
    assert_eq!(kept(&s, "SELECT T.ID FROM T, U WHERE T.ID = U.ID"), None);
}

/// The rows of a table, and the distinct values of its second column, as
/// a plug-in rule read them.
type Noted = (Option<f64>, Option<f64>);

/// A plug-in rule noting the statistics of each table it sees.
struct Seen(Arc<Mutex<Vec<Noted>>>);

impl Rule for Seen {
    fn name(&self) -> &'static str {
        "note_statistics"
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Scan { table, .. } = plan else { return None };
        let stats = ctx.db.statistics(table);
        self.0.lock().unwrap().push((stats.as_ref().and_then(|s| s.rows()), stats.and_then(|s| s.distinct(1))));
        None
    }
}

/// Verifies: [DESIGN] RULES.md D31, extensibility: an optimizer rule reads
/// a table's statistics through `RuleContext`, as the planner does; a table
/// without any has none.
#[test]
fn rules_read_statistics() {
    let db = graded(200);
    let seen = Arc::new(Mutex::new(Vec::new()));
    db.with_optimizer(|o| o.add(Box::new(Seen(Arc::clone(&seen))))).unwrap();
    db.session("APP").physical_plan("SELECT ID FROM T", &Params::new()).unwrap();
    let got = seen.lock().unwrap().first().copied();
    assert!(got.is_some_and(|(rows, distinct)| rows == Some(200.0) && distinct.is_some_and(|d| (d - 10.0).abs() < 0.5)), "{got:?}");
    let db = locking_big();
    let seen = Arc::new(Mutex::new(Vec::new()));
    db.with_optimizer(|o| o.add(Box::new(Seen(Arc::clone(&seen))))).unwrap();
    db.session("APP").physical_plan("SELECT ID FROM T", &Params::new()).unwrap();
    assert_eq!(seen.lock().unwrap().first().copied(), Some((None, None)));
}

/// Verifies: [DESIGN] RULES.md D31, D35, the row store: a new table has no
/// statistics until gathered; `analyze` gathers them; a table opened again
/// has none until first asked for, which makes them due, and maintenance
/// then gathers them from its newest committed rows. Regression test
/// (R-TEST-7): the first ask made the pass itself, in whatever plan asked.
#[test]
fn tables_opened_have_statistics() {
    let scratch = Scratch::new("statistics-open");
    let hash = crc32c();
    let pager = Arc::new(Pager::create(&OsFileSystem, &scratch.path("rows"), hash, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(pager);
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let types = [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }];
    let t = RowTable::create(&files, &types, TableOptions::default()).unwrap();
    assert!(t.statistics().is_none());
    for n in 0..30 {
        t.insert(NO_TRANSACTION, vec![number(n)]).unwrap();
    }
    assert!(t.statistics().is_none(), "30 changes: not due, and nothing ended");
    t.analyze().unwrap();
    assert_eq!(t.statistics().and_then(|s| s.rows()), Some(30.0));
    let meta = t.meta();
    drop(t);
    let again = RowTable::open(&files, meta, &types, TableOptions::default()).unwrap();
    while again.maintain(u64::MAX).unwrap() {}
    assert!(again.statistics().is_none(), "not gathered until asked for, and asking makes no pass");
    assert!(again.maintenance_due(), "but makes them due");
    while again.maintain(u64::MAX).unwrap() {}
    assert!(!again.maintenance_due());
    let stats = again.statistics().unwrap();
    assert_eq!((stats.rows(), stats.nulls(0)), (Some(30.0), Some(0.0)));
    assert!(stats.selectivity(0, CompOp::Lt, &number(15)).is_some_and(|f| (f - 0.5).abs() < 0.05));
    assert!(stats.width(&[0]).is_some_and(|w| w > 0.0));
}

/// Verifies: [DESIGN] RULES.md D35, D55: a table of a database opened again
/// has no statistics until first asked for, and planning then makes no pass
/// over it: queries planned at once on two threads plan without them, and
/// the table is noted for maintenance, asked for as their transactions
/// end; the maintenance job gathers them, once, and plans after have them.
/// Regression test (R-TEST-7): the first plan of each planner made a pass
/// over the whole table itself, which no cancel could stop.
#[test]
fn a_table_opened_has_its_statistics_gathered_by_maintenance() {
    let scratch = Scratch::new("statistics-reopened");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, GRADE INTEGER, NOTE CHAR(5))").unwrap();
    add(&db, 1..=200, |i| i % 10);
    db.close().unwrap();
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    db.with_optimizer(|o| o.add(Box::new(Seen(Arc::clone(&seen))))).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let s = db.session("APP");
                assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["200".into()]));
                s.commit().unwrap();
            });
        }
    });
    let planned = seen.lock().unwrap().clone();
    assert!(!planned.is_empty() && planned.iter().all(|noted| *noted == (None, None)), "{planned:?}");
    db.run_due().unwrap();
    seen.lock().unwrap().clear();
    db.session("APP").physical_plan("SELECT ID FROM T", &Params::new()).unwrap();
    assert_eq!(seen.lock().unwrap().first().map(|noted| noted.0), Some(Some(200.0)));
}

/// An index method, "uncounted", whose indexes are the B-link tree's but
/// estimate no probe, and know their distinct keys only once maintenance
/// has counted them, as a built-in index opened without its counts does;
/// the passes it made counted.
struct Uncounted(BTreeMethod, Arc<AtomicUsize>);

impl IndexMethod for Uncounted {
    fn name(&self) -> &'static str {
        "uncounted"
    }
    fn supports(&self, op: CompOp) -> bool {
        self.0.supports(op)
    }
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        self.0.supports_prefix(columns, equal, then)
    }
    fn ordered(&self) -> bool {
        self.0.ordered()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let entries = Arc::from(self.0.create(columns)?);
        let (asked, failed) = (AtomicBool::new(false), AtomicBool::new(false));
        Ok(Box::new(UncountedIndex { entries, asked, failed, passes: Arc::clone(&self.1) }))
    }
}

/// An index of `Uncounted`: whether its keys were asked for, and whether
/// its first pass failed, as it does.
struct UncountedIndex {
    entries: Arc<dyn Index>,
    asked: AtomicBool,
    failed: AtomicBool,
    passes: Arc<AtomicUsize>,
}

impl Index for UncountedIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.entries.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.entries.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.entries.rows(key)
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn distinct_keys(&self) -> Option<f64> {
        if self.passes.load(Ordering::SeqCst) == 0 {
            self.asked.store(true, Ordering::SeqCst);
            return None;
        }
        self.entries.distinct_keys()
    }
    fn maintenance_due(&self) -> bool {
        self.asked.load(Ordering::SeqCst) && self.passes.load(Ordering::SeqCst) == 0
    }
    fn maintain(&self) -> Result<bool> {
        if !self.maintenance_due() {
            return Ok(false);
        }
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "[DESIGN] the first pass fails"));
        }
        self.passes.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        Arc::clone(&self.entries).search(probe, order)
    }
}

/// The estimated rows of the scan of `U_AB` that `sql` plans, in `s`.
fn ab_estimate(s: &Session, sql: &str) -> f64 {
    let plan = s.explain(sql, &Params::new()).unwrap();
    let est = plan.lines().find(|l| l.contains("using U_AB")).unwrap_or_else(|| panic!("{plan}"));
    est.split("est. rows=").nth(1).and_then(|r| r.split(',').next()).and_then(|n| n.parse::<f64>().ok()).unwrap_or_else(|| panic!("{est}"))
}

/// Verifies: [DESIGN] RULES.md D35, D55, `Index::maintenance_due`,
/// `Index::maintain`: an index that must count its distinct keys first
/// gives none to planning, and makes no pass there: queries planned at
/// once on two threads plan without them, and the table is noted for
/// maintenance, asked for as their transactions end; the maintenance job
/// has the index count them, once, a pass that failed made again when
/// maintenance is next done, and plans after estimate an equality on all its columns as
/// the table's rows over them. Regression test
/// (R-TEST-7): an index opened without its counts counted its keys in a
/// pass over every entry inside the first plan to ask, which no cancel
/// could stop.
#[test]
fn an_index_has_its_keys_counted_by_maintenance() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let passes = Arc::new(AtomicUsize::new(0));
    db.register_index_method(Box::new(Uncounted(BTreeMethod::in_memory(), Arc::clone(&passes)))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE U (K INTEGER NOT NULL, A INTEGER NOT NULL, B INTEGER NOT NULL)").unwrap();
    db.create_index("U_AB", "APP.U", &["A", "B"], "uncounted").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, {}, {})", k % 10, k % 10)).collect();
    s.run(&format!("INSERT INTO U VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    // Asked in the index's order, which the index gives as it probes.
    let sql = "SELECT K, A, B FROM U WHERE A = 3 AND B = 3 ORDER BY A, B";
    let ests = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let s = db.session("APP");
                ests.lock().unwrap().push(ab_estimate(&s, sql));
                assert_eq!(rows(&s, "SELECT COUNT(*) FROM U WHERE A = 3 AND B = 3"), Ok(vec!["200".into()]));
                s.commit().unwrap();
            });
        }
    });
    assert_eq!(passes.load(Ordering::SeqCst), 0, "no pass in planning");
    let without = ests.into_inner().unwrap();
    assert!(without.iter().all(|est| (est - 200.0_f64).abs() > 50.0_f64), "as if its columns were independent: {without:?}");
    assert_eq!(db.run_due().err().map(|e| e.code), Some(Code::Io), "the first pass fails");
    assert_eq!(passes.load(Ordering::SeqCst), 0);
    db.maintain().unwrap();
    assert_eq!(passes.load(Ordering::SeqCst), 1, "counted by maintenance, once");
    let est = ab_estimate(&db.session("APP"), sql);
    assert!((est - 200.0_f64).abs() < 10.0_f64, "the rows over its ten keys: {est}");
    db.run_due().unwrap();
    assert_eq!(passes.load(Ordering::SeqCst), 1, "not due again");
}

/// Verifies: [DESIGN] RULES.md D31, D35, a filter over a scan that skips
/// zones (`TableStorage::scan_share`): the rows it keeps are estimated as
/// its share of the table's, no more than the scan reads; a LIKE of a
/// prefix of a column in the rows' order, which the estimate guesses as a
/// range, a third of the table, keeps no more than the rows of the zones
/// it may match, a zone of the twenty; and the rows found are those the
/// condition matches.
#[test]
fn a_filters_estimate_is_no_more_than_its_scan_reads() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE L (K INTEGER NOT NULL, C CHARACTER(8))").unwrap();
    let s = db.session("S");
    for from in (0..20_000_u32).step_by(500) {
        let values: Vec<String> = (from..from + 500).map(|k| format!("({k}, '{k:08}')")).collect();
        s.run(&format!("INSERT INTO L VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    let sql = "SELECT K FROM L WHERE C LIKE '0001234%'";
    let est = kept(&s, sql).unwrap();
    assert!(est <= 2048.0_f64, "at most the rows of the zones read: {est}");
    let found = rows(&s, sql).unwrap();
    assert_eq!(found.len(), 10, "{found:?}");
}
