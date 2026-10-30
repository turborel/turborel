//! Spilling (RULES.md D38): operators that hold more rows than WORK_MEMORY
//! spill them to areas of the database's spill provider, reserved of the
//! memory budget, and give the rows, in the order, they give in memory.

use std::sync::{Arc, Mutex};

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::{Code, SqlError};
use turborel::memory::MemoryBudget;
use turborel::spill::{Area, PagedSpill, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles};
use turborel::txn::Stop;
use turborel::value::{Decimal, Value};

use crate::common::{Counted, Scratch, exec, os, rows};

/// The smallest WORK_MEMORY, 64 KiB.
pub(crate) const SMALL: usize = 64 << 10;
/// Half of it, as a grouping of DISTINCT set functions splits it.
const HALF: usize = 32 << 10;
/// Rows of each table: some thousands, many times the smallest work area.
pub(crate) const ROWS: usize = 3000;
/// Rows of `V`, half of them of one key: more than the smallest work area
/// holds.
const SKEWED: usize = 1200;

pub(crate) fn bytes(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0))
}

/// Schema `S` with tables `T` and `U`, each `ROWS` rows: `K` from 0, `G` of
/// 1000 values and `S` of 701 in `T`, of 2505 in `U`; in `U` a null `G` every 100th
/// row, its `S` from its `K`; `V` of `SKEWED` rows, `C` 0 in every other one; and `W` of four,
/// 0 to 3; `E`, empty; and `R` of eight, 0 to 7.
pub(crate) fn filled(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER, G INTEGER, S CHARACTER(60))
           CREATE TABLE U (K INTEGER, G INTEGER, S CHARACTER(60))
           CREATE TABLE V (K INTEGER, C INTEGER, S CHARACTER(60))
           CREATE TABLE W (K INTEGER)
           CREATE TABLE E (K INTEGER)
           CREATE TABLE R (K INTEGER)",
    )
    .unwrap();
    let s = db.session("S");
    let chunks: Vec<Vec<usize>> = (0..ROWS).collect::<Vec<_>>().chunks(100).map(<[usize]>::to_vec).collect();
    for chunk in chunks {
        let t: Vec<String> = chunk.iter().map(|k| format!("({k}, {}, 'S{:05}')", k % 1000, (k * 7) % 701)).collect();
        let u: Vec<String> = chunk.iter().map(|&k| u_row(k)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", t.join(", "))).unwrap();
        s.run(&format!("INSERT INTO U VALUES {}", u.join(", "))).unwrap();
    }
    let v: Vec<String> = (0..SKEWED).map(|k| format!("({k}, {}, 'V{k:05}')", if k % 2 == 0 { 0 } else { k })).collect();
    s.run(&format!("INSERT INTO V VALUES {}", v.join(", "))).unwrap();
    s.run("INSERT INTO W VALUES (0), (1), (2), (3)").unwrap();
    let r: Vec<String> = (0..8).map(|k| format!("({k})")).collect();
    s.run(&format!("INSERT INTO R VALUES {}", r.join(", "))).unwrap();
}

/// Row `k` of `U`, as a row value constructor.
fn u_row(k: usize) -> String {
    if k.is_multiple_of(100) { format!("({k}, NULL, 'S{k:05}')") } else { format!("({k}, {}, 'S{:05}')", (k * 3) % 1000, k % 2500) }
}

/// Queries whose operators hold their rows: a sort with ties, grouping,
/// DISTINCT, a hash join, a product, outer joins with keys and without,
/// the set operations, subqueries kept and decorrelated, and DISTINCT set
/// functions.
const QUERIES: &[&str] = &[
    "SELECT G, K FROM T ORDER BY G DESC",
    "SELECT S, G, K FROM T ORDER BY S, G",
    "SELECT G, COUNT(*), SUM(K), MIN(S) FROM T GROUP BY G",
    "SELECT S, COUNT(DISTINCT G) FROM T GROUP BY S",
    "SELECT S, K, COUNT(*), MAX(G) FROM U GROUP BY S, K",
    "SELECT DISTINCT S FROM U",
    "SELECT DISTINCT G, S FROM T",
    "SELECT T.K, U.K FROM T, U WHERE T.G = U.G",
    "SELECT T.K, V.K FROM T, V WHERE T.K = V.C",
    "SELECT W.K, T.K FROM W, T",
    "SELECT T.K, U.K FROM T LEFT OUTER JOIN U ON T.G = U.G",
    "SELECT T.K, U.K FROM T RIGHT OUTER JOIN U ON T.G = U.G AND T.K < 2000",
    "SELECT T.K, U.K FROM T FULL OUTER JOIN U ON T.G = U.G AND U.K > 100",
    "SELECT T.K, V.K FROM T FULL OUTER JOIN V ON T.K = V.C",
    "SELECT U.K, T.K FROM U LEFT OUTER JOIN T ON U.G = T.G",
    "SELECT W.K, T.K FROM W LEFT OUTER JOIN T ON W.K + 2990 < T.K",
    "SELECT W.K, T.K FROM W FULL OUTER JOIN T ON W.K + 2995 < T.K",
    "SELECT E.K, T.K FROM E FULL OUTER JOIN T ON E.K < T.K",
    "SELECT T.K, V.K FROM T LEFT OUTER JOIN V ON T.K = V.C",
    "SELECT G FROM T EXCEPT SELECT G FROM U",
    "SELECT G FROM T EXCEPT ALL SELECT G FROM U",
    "SELECT G FROM T INTERSECT SELECT G FROM U",
    "SELECT G FROM T INTERSECT ALL SELECT G FROM U",
    "SELECT G FROM T UNION SELECT G FROM U",
    "SELECT S FROM U EXCEPT SELECT S FROM T",
    "SELECT S, G FROM U EXCEPT ALL SELECT S, G FROM T",
    "SELECT S FROM T INTERSECT SELECT S FROM U",
    "SELECT S FROM U INTERSECT ALL SELECT S FROM U WHERE K > 500",
    "SELECT K FROM W WHERE K + 3 IN (SELECT G FROM U)",
    "SELECT K FROM T WHERE S IN (SELECT S FROM U)",
    "SELECT K FROM T WHERE G >= ALL (SELECT G FROM U WHERE U.K < 2900)",
    "SELECT K FROM T WHERE EXISTS (SELECT * FROM U WHERE U.G = T.G AND U.K > 10)",
    "SELECT K FROM W WHERE K < ALL (SELECT G FROM U WHERE U.K > 100)",
    "SELECT K FROM W WHERE UNIQUE (SELECT S FROM U)",
    "SELECT K FROM W WHERE K MATCH UNIQUE (SELECT G FROM T)",
    "SELECT COUNT(DISTINCT S), COUNT(DISTINCT G), COUNT(*) FROM T",
];

/// Verifies: [DESIGN] RULES.md D38: with WORK_MEMORY at its least, a sort,
/// grouping, DISTINCT, a hash join, a product, LEFT, RIGHT and FULL outer
/// joins on keys and on no keys [ISO/IEC 9075:1992 §7.5], EXCEPT,
/// INTERSECT and UNION, the rows of subqueries, kept and decorrelated, for
/// IN, EXISTS, ALL, UNIQUE and MATCH, and the values of DISTINCT set
/// functions spill to
/// areas of WORK_MEMORY's bytes, of the provider given (`set_spill`), and
/// give the rows, in the order, they give in memory, whatever SPILL_BATCH,
/// SPILL.PARTITIONS and SPILL.PARTITION_DEPTH;
/// the areas' memory goes back to the budget once the query is read. A
/// hash join's and an outer join's partitions are partitioned again while
/// they outgrow the work area, as SPILL.PARTITION_DEPTH allows, and one
/// that does not shrink, as of a key most rows have, is joined so too.
#[test]
fn operators_spill_and_give_the_same_rows() {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let s = db.session("S");
    let in_memory: Vec<Vec<String>> = QUERIES.iter().map(|q| rows(&s, q).unwrap()).collect();
    assert_eq!(counted.taken(), 0, "the default WORK_MEMORY holds them all");
    let reserved = db.memory().unwrap().1;

    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    // The default batch, then the least, with which a join partitions most;
    // then two partitions, partitioned again while they outgrow the work
    // area, and never, built in tables or joined in parts.
    for (batch, partitions, depth) in [(16 << 10, 64, 8), (4 << 10, 64, 8), (16 << 10, 2, 8), (16 << 10, 2, 0)] {
        db.set_setting("SPILL_BATCH", &bytes(batch)).unwrap();
        db.set_setting("SPILL.PARTITIONS", &bytes(partitions)).unwrap();
        db.set_setting("SPILL.PARTITION_DEPTH", &bytes(depth)).unwrap();
        for (q, expected) in QUERIES.iter().zip(&in_memory) {
            assert_eq!(&rows(&s, q).unwrap(), expected, "{q}, batch {batch}");
            assert!(counted.taken() > 0, "{q} spilled");
            assert_eq!(db.memory().unwrap().1, reserved, "{q}: its areas given back");
        }
    }
    // A grouping of DISTINCT set functions splits its work area in two.
    assert!(counted.0.lock().unwrap().iter().all(|&b| b == SMALL || b == HALF));
    assert_eq!(db.set_setting("WORK_MEMORY", &bytes(SMALL - 1)).err().map(|e| e.code), Some(Code::Api), "below its least");
}

/// Verifies: [DESIGN] RULES.md D38: a spill area is reserved of the memory
/// budget: one the budget can grant is made, one it cannot is refused
/// (SQLCODE -918), and the query with it; under the default WORK_MEMORY,
/// which holds its rows, the same query needs no area.
#[test]
fn a_spill_area_past_the_budget_is_refused() {
    const SORT: &str = "SELECT S, K FROM T ORDER BY S";
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory = rows(&s, SORT).unwrap();
    let reserved = db.memory().unwrap().1;
    assert!(reserved >= 1 << 20, "the engine's pool");
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    db.set_setting("MEMORY_BUDGET", &bytes(reserved + (1 << 20))).unwrap();
    assert_eq!(rows(&s, SORT).unwrap(), in_memory, "an area the budget grants");
    db.set_setting("MEMORY_BUDGET", &bytes(1 << 20)).unwrap();
    assert_eq!(rows(&s, SORT).err(), Some(Code::Resources), "one it cannot");
    db.reset_setting("WORK_MEMORY").unwrap();
    assert_eq!(rows(&s, SORT).unwrap(), in_memory, "no area needed");
}

/// Verifies: [DESIGN] RULES.md D38: a database kept in a directory keeps
/// its spill areas' files in `tmp/` there, each removed once its operator
/// is done, and removes any left there when it opens.
#[test]
fn a_kept_database_spills_to_its_directory() {
    let scratch = Scratch::new("spilling");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    filled(&db);
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    let s = db.session("S");
    let mut c = s.open_cursor("SELECT S, K FROM T ORDER BY S").unwrap();
    assert!(c.fetch().unwrap().is_some());
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 1, "a sort's area in tmp/ while it is read");
    drop(c);
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0, "and removed after");
    drop(s);
    db.close().unwrap();
    drop(db);
    std::fs::write(dir.join("tmp").join("left.spill"), b"stale").unwrap();
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0, "a file left is removed");
}

/// Verifies: [DESIGN] RULES.md D36, D38: WORK_MEMORY is a session's to set:
/// its statements spill in work areas of its size, another session's in
/// the database's, which hold them; SET WORK_MEMORY TO DEFAULT takes the
/// database's again. The rows are the same either way.
#[test]
fn a_session_sets_its_own_work_memory() {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let (small, other) = (db.session("S"), db.session("S"));
    let sort = "SELECT S, K FROM T ORDER BY S";
    exec(&small, &format!("SET WORK_MEMORY = {SMALL}")).unwrap();
    let spilled = rows(&small, sort).unwrap();
    let sizes: Vec<usize> = counted.0.lock().unwrap().drain(..).collect();
    assert!(!sizes.is_empty() && sizes.iter().all(|&b| b == SMALL), "{sizes:?}");
    assert_eq!(rows(&other, sort).unwrap(), spilled, "the same rows");
    assert_eq!(counted.taken(), 0, "another session's in the database's work area");
    exec(&small, "SET WORK_MEMORY TO DEFAULT").unwrap();
    assert_eq!(rows(&small, sort).unwrap(), spilled);
    assert_eq!(counted.taken(), 0, "the database's again");
}

/// Verifies: [DESIGN] RULES.md D38, D44: with WORK_MEMORY at its least, a
/// statement cancelled as a spilled operator begins to read back what it
/// wrote, after every row of its tables is read, fails (SQLCODE -920)
/// there: grouping, of set functions and of DISTINCT ones, EXCEPT and
/// INTERSECT, a hash join and an outer join on keys reading their
/// partitions, and a product and an outer join on no keys their right
/// operand's run, by batches and row by row. The session's next statement
/// runs, giving the rows it gives unspilled. Regression test (R-TEST-7):
/// those passes looked for no cancel, so a statement ran on until it read
/// a table again, or to its end.
#[test]
fn a_cancel_stops_a_spilled_operator_as_it_reads_back() {
    const CANCELLED: &[&str] = &[
        "SELECT G, COUNT(*), MIN(S) FROM T GROUP BY G",
        "SELECT S, COUNT(DISTINCT G) FROM T GROUP BY S",
        "SELECT S, G FROM U EXCEPT ALL SELECT S, G FROM T",
        "SELECT S FROM T INTERSECT SELECT S FROM U",
        "SELECT T.K, U.K FROM T, U WHERE T.G = U.G",
        "SELECT T.K, V.K FROM T FULL OUTER JOIN V ON T.K = V.C",
        "SELECT W.K, T.K FROM W, T",
        "SELECT W.K, T.K FROM W FULL OUTER JOIN T ON W.K + 2995 < T.K",
    ];
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory: Vec<Vec<String>> = CANCELLED.iter().map(|q| rows(&s, q).unwrap()).collect();
    let cancelling = Cancelling::default();
    db.set_spill(Box::new(cancelling.clone())).unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    for batches in ["ON", "OFF"] {
        exec(&s, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        for (q, expected) in CANCELLED.iter().zip(&in_memory) {
            cancelling.arm(&s);
            assert_eq!(rows(&s, q).err(), Some(Code::Cancelled), "{q}, batches {batches}");
            assert!(cancelling.fired(), "{q}");
            assert_eq!(&rows(&s, q).unwrap(), expected, "{q}: the next statement runs");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D38: the rows a correlated subquery gave for
/// each set of outer values are kept only while they fit the work area,
/// counted with their keys and entries, and rows that spilled as their
/// area: a subquery run for many sets of outer values, each of its results
/// spilling, holds a few areas of the budget at once, not one for each
/// result; the rows are those it gives unspilled. Regression test
/// (R-TEST-7): a result that spilled counted as no bytes, so each was kept,
/// with its area, until the statement ended.
#[test]
fn kept_subquery_rows_stay_within_the_work_area() {
    const CORRELATED: &str = "SELECT K FROM R WHERE K * 7 IN (SELECT G FROM U WHERE U.K >= R.K)";
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory = rows(&s, CORRELATED).unwrap();
    assert_ne!(in_memory, Vec::<String>::new());
    let peak = Peak::default();
    db.set_spill(Box::new(peak.clone())).unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    let reserved = db.memory().unwrap().1;
    assert_eq!(rows(&s, CORRELATED).unwrap(), in_memory);
    let most = peak.taken();
    assert!(most > reserved, "its results spilled");
    // Eight results, each kept with its area, held eight areas and more.
    assert!(most - reserved <= 2 * SMALL, "{} bytes of areas at once", most - reserved);
}

/// Verifies: [DESIGN] RULES.md D38: a grouping of DISTINCT set functions
/// splits its work area in two, half for its groups and half for the bag
/// of the values they took, each spilling to an area of its half, so the
/// two hold no more than WORK_MEMORY; the rows are those it gives
/// unspilled. The rows of the query, read as its cursor opens as its set
/// functions may raise an error, are held apart, in an area of their own.
/// Regression test (R-TEST-7): the bag took a work area of its own, so
/// the grouping held up to twice WORK_MEMORY.
#[test]
fn a_grouping_shares_its_work_area_with_its_distinct_values() {
    const GROUPED: &str = "SELECT S, COUNT(DISTINCT G), COUNT(DISTINCT K) FROM T GROUP BY S";
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory = rows(&s, GROUPED).unwrap();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    assert_eq!(rows(&s, GROUPED).unwrap(), in_memory);
    let sizes: Vec<usize> = counted.0.lock().unwrap().drain(..).collect();
    let halves = sizes.iter().filter(|&&b| b == HALF).count();
    let whole = sizes.iter().filter(|&&b| b == SMALL).count();
    assert!(halves >= 2 && whole <= 1 && halves + whole == sizes.len(), "{sizes:?}");
}

/// Verifies: [DESIGN] RULES.md D38: a key's rows built in a table of the
/// spill area are read from it as they are joined (`SpillTable::values`),
/// never gathered (`SpillTable::get`, refused here): of a hash join's
/// partition of one key, too big for the work area, probed by each left
/// row of that key; of a LEFT join read by batches; and of a decorrelated
/// subquery's rows kept by key. The rows are those it gives unspilled.
/// Regression test (R-TEST-7): each probe gathered every row of its key in
/// memory, however many.
#[test]
fn a_probe_reads_its_keys_rows_as_it_joins() {
    const PROBED: &[&str] = &[
        "SELECT T.K, V.K FROM T, V WHERE T.K = V.C",
        "SELECT T.K, V.K FROM T LEFT OUTER JOIN V ON T.K = V.C",
        "SELECT K FROM T WHERE EXISTS (SELECT * FROM V WHERE V.C = T.K AND V.K > 10)",
    ];
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    let in_memory: Vec<Vec<String>> = PROBED.iter().map(|q| rows(&s, q).unwrap()).collect();
    db.set_spill(Box::new(Streamed)).unwrap();
    db.set_setting("WORK_MEMORY", &bytes(SMALL)).unwrap();
    for (q, expected) in PROBED.iter().zip(&in_memory) {
        assert_eq!(&rows(&s, q).unwrap(), expected, "{q}");
    }
}

/// The built-in provider, whose tables refuse to give a key's values at
/// once (`SpillTable::get`).
struct Streamed;

impl Spill for Streamed {
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        Ok(Box::new(StreamedArea(PagedSpill.area(memory, temp, area)?)))
    }
}

struct StreamedArea(Box<dyn SpillArea>);

impl SpillArea for StreamedArea {
    fn table(&self) -> turborel::error::Result<Box<dyn SpillTable>> {
        Ok(Box::new(StreamedTable(self.0.table()?)))
    }
    fn run(&self) -> turborel::error::Result<Box<dyn SpillRun>> {
        self.0.run()
    }
}

struct StreamedTable(Box<dyn SpillTable>);

impl SpillTable for StreamedTable {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> turborel::error::Result<()> {
        self.0.insert(key, value)
    }
    fn get(&self, _: &[u8]) -> turborel::error::Result<Vec<Vec<u8>>> {
        Err(SqlError::new(Code::Api, "[DESIGN] a key's values gathered"))
    }
    fn values(&self, key: &[u8]) -> turborel::error::Result<Records> {
        self.0.values(key)
    }
    fn remove(&mut self, key: &[u8], value: &[u8]) -> turborel::error::Result<bool> {
        self.0.remove(key, value)
    }
}

/// The built-in provider, keeping the most bytes the memory budget had
/// reserved as an area was asked for: what the areas made before held,
/// and every other reservation.
#[derive(Clone, Default)]
pub struct Peak(pub Arc<Mutex<usize>>);

impl Spill for Peak {
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        let mut most = self.0.lock().unwrap();
        *most = (*most).max(memory.reserved());
        Spill::area(&PagedSpill, memory, temp, area)
    }
}

impl Peak {
    /// The most reserved since the last call.
    pub fn taken(&self) -> usize {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// What `Cancelling` cancels: a session's statement, once.
type Armed = Arc<Mutex<Option<Arc<Stop>>>>;

/// The built-in provider, whose runs, once armed with a session's
/// canceller (`Cancelling::arm`), cancel what the session runs the first
/// time one is read back: as a spilled operator's passes over what it
/// wrote begin, after it read its operands' rows.
#[derive(Clone, Default)]
pub struct Cancelling(Armed);

impl Cancelling {
    /// Cancels what `s` runs as a run is next read back.
    pub fn arm(&self, s: &Session) {
        *self.0.lock().unwrap() = Some(s.canceller());
    }

    /// Whether it cancelled since it was armed.
    pub fn fired(&self) -> bool {
        self.0.lock().unwrap().is_none()
    }
}

impl Spill for Cancelling {
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        let made = Spill::area(&PagedSpill, memory, temp, area)?;
        Ok(Box::new(CancellingArea(made, Arc::clone(&self.0))))
    }
}

struct CancellingArea(Box<dyn SpillArea>, Armed);

impl SpillArea for CancellingArea {
    fn table(&self) -> turborel::error::Result<Box<dyn SpillTable>> {
        self.0.table()
    }
    fn run(&self) -> turborel::error::Result<Box<dyn SpillRun>> {
        Ok(Box::new(CancellingRun(self.0.run()?, Arc::clone(&self.1))))
    }
}

struct CancellingRun(Box<dyn SpillRun>, Armed);

impl CancellingRun {
    fn fire(armed: &Armed) {
        if let Some(stop) = armed.lock().unwrap().take() {
            stop.cancel();
        }
    }
}

impl SpillRun for CancellingRun {
    fn push(&mut self, record: &[u8]) -> turborel::error::Result<()> {
        self.0.push(record)
    }
    fn records(&mut self) -> turborel::error::Result<Records> {
        CancellingRun::fire(&self.1);
        self.0.records()
    }
    fn into_records(self: Box<Self>) -> turborel::error::Result<Records> {
        CancellingRun::fire(&self.1);
        self.0.into_records()
    }
}
