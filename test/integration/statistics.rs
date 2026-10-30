//! Statistics for the optimizer (RULES.md D31): the planner estimates the
//! rows a condition keeps from what a table's engine knows of its rows,
//! and from defaults where it knows nothing; the row store gathers its
//! statistics when a table opened is first asked for them, when it has
//! changed enough, and when told to; optimizer rules can read them.

use std::sync::{Arc, Mutex};

use turborel::db::{Database, Session};
use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::engine::rows::{RowTable, TableFiles, TableOptions};
use turborel::error::Code;
use turborel::log::MemoryLog;
use turborel::logical::LogicalPlan;
use turborel::optimizer::{Rule, RuleContext};
use turborel::params::Params;
use turborel::storage::TableStorage;
use turborel::threads::ThreadPolicy;
use turborel::txn::NO_TRANSACTION;
use turborel::value::{CompOp, DataType};

use crate::common::{Scratch, crc32c, locking_big, number};
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
/// done only when a test asks (THREADS.POLICY MANUAL, RULES.md D55).
fn graded(n: u32) -> Database {
    let db = Database::new();
    db.set_thread_policy(ThreadPolicy::Manual).unwrap();
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
    db.set_thread_policy(ThreadPolicy::Automatic).unwrap();
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

/// Verifies: [DESIGN] RULES.md D31, the row store: a new table has no
/// statistics until gathered; `analyze` gathers them; a table opened again
/// has them, gathered when first asked for, from its newest committed
/// rows.
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
    let stats = again.statistics().unwrap();
    assert_eq!((stats.rows(), stats.nulls(0)), (Some(30.0), Some(0.0)));
    assert!(stats.selectivity(0, CompOp::Lt, &number(15)).is_some_and(|f| (f - 0.5).abs() < 0.05));
    assert!(stats.width(&[0]).is_some_and(|w| w > 0.0));
}
