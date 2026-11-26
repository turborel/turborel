//! \[DESIGN\] A grouping run in parallel whose partitions outgrow its
//! grant (RULES.md D1226): its partitions are written to the spill by the
//! threads that merged them and merged back at the end, split again where
//! one alone outgrows the grant, and it gives the rows it gives in memory,
//! in their order, rather than giving way to a grouping a row at a time.

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use turborel::db::{Database, Session};
use turborel::memory::MemoryBudget;
use turborel::spill::{Area, PagedSpill, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The smallest MEMORY.WORK_BYTES, 64 KiB.
const SMALL: i128 = 64 << 10;

/// What the runs of a provider's areas showed: the thread each run was
/// made on, the bytes written to them, and how many areas were made.
#[derive(Default)]
struct Seen {
    made_on: Vec<ThreadId>,
    written: usize,
    areas: usize,
}

/// The built-in provider, keeping what `Seen` keeps.
#[derive(Clone, Default)]
struct Threads(Arc<Mutex<Seen>>);

impl Spill for Threads {
    fn name(&self) -> &'static str {
        "threads"
    }
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        self.0.lock().unwrap().areas += 1;
        Ok(Box::new(Kept(Spill::area(&PagedSpill, memory, temp, area)?, Arc::clone(&self.0))))
    }
}

impl Threads {
    /// What the runs showed since the last call.
    fn taken(&self) -> Seen {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// An area of the built-in provider, keeping what `Threads` keeps.
struct Kept(Box<dyn SpillArea>, Arc<Mutex<Seen>>);

impl SpillArea for Kept {
    fn table(&self) -> turborel::error::Result<Box<dyn SpillTable>> {
        self.0.table()
    }
    fn run(&self) -> turborel::error::Result<Box<dyn SpillRun>> {
        self.1.lock().unwrap().made_on.push(std::thread::current().id());
        Ok(Box::new(Written(self.0.run()?, Arc::clone(&self.1))))
    }
}

/// A run of the built-in provider, counting the bytes written to it.
struct Written(Box<dyn SpillRun>, Arc<Mutex<Seen>>);

impl SpillRun for Written {
    fn push(&mut self, record: &[u8]) -> turborel::error::Result<()> {
        self.1.lock().unwrap().written += record.len();
        self.0.push(record)
    }
    fn records(&mut self) -> turborel::error::Result<Records> {
        self.0.records()
    }
}

/// Table C of the column store in groups of 2048: K, a key; G, one of
/// 24000, each of four rows, null for every 37th; T, a text of G with trailing spaces; N, a
/// number.
fn table() -> (Database, Session, Threads) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let threads = Threads::default();
    db.set_spill(Box::new(threads.clone())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G BIGINT, T VARCHAR(20), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0_i64..96_000)
        .map(|k| {
            let g = k * 7919 % 24_000;
            let shown = if k % 37 == 0 { "NULL".to_owned() } else { g.to_string() };
            format!("({k}, {shown}, 'g{g}{}', {})", " ".repeat(usize::try_from(k % 3).unwrap()), k % 101)
        })
        .collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    threads.taken();
    (db, a, threads)
}

/// Runs `sql` alone, a row at a time.
fn alone(s: &Session, sql: &str) -> Vec<String> {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Runs `sql` in parallel on 4 threads in morsels of 256 rows.
fn parallel(s: &Session, sql: &str) -> Vec<String> {
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Groupings of many groups run in parallel: of a count, an exact sum and
/// mean, a text's greatest, written by its accumulator; of a text key; under
/// HAVING.
const QUERIES: [&str; 3] = [
    "SELECT G, COUNT(*), SUM(N), AVG(N), MAX(T) FROM C GROUP BY G",
    "SELECT T, COUNT(*), MIN(N) FROM C GROUP BY T",
    "SELECT G, SUM(N) FROM C GROUP BY G HAVING COUNT(*) > 2",
];

/// Verifies: [DESIGN] (RULES.md D1226) groupings run in parallel at the
/// smallest MEMORY.WORK_BYTES, whose partitions outgrow it many times,
/// write their partitions to the spill on the threads that merged them,
/// not on the statement's thread alone as a grouping given way to does,
/// in no more areas than the grouping run alone at that grant makes, which
/// spills in one of its own beside those that hold its rows, as a grouping
/// given way to would make one more, and give the rows they give run alone a row at a time, in their order, with their
/// spilled partitions split again where one outgrows the grant.
#[test]
fn partitions_past_the_grant_are_spilled_by_their_threads() {
    let (db, a, threads) = table();
    let in_memory: Vec<Vec<String>> = QUERIES.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    let statement = std::thread::current().id();
    for (q, expected) in QUERIES.iter().zip(&in_memory) {
        alone(&a, q);
        let serial = threads.taken().areas;
        assert_eq!(&parallel(&a, q), expected, "{q}");
        let Seen { made_on, written, areas } = threads.taken();
        let shown = a.physical_plan(q, &turborel::params::Params::new()).unwrap();
        assert!(
            made_on.iter().any(|&t| t != statement),
            "{q}: runs made by the partitions' threads, {} runs, {written} bytes\n{shown}",
            made_on.len()
        );
        assert!(written > usize::try_from(SMALL).unwrap() * 4, "{q}: its partitions many times the grant, spilled");
        assert!(areas <= serial, "{q}: {areas} areas, the grouping run alone {serial}: none of a grouping given way to");
    }
}

/// Verifies: [DESIGN] (RULES.md D1226, D1113) the first grouping of a
/// pair, as a DISTINCT set function is run, and SELECT DISTINCT, at the
/// smallest MEMORY.WORK_BYTES, give the rows they give run alone, in
/// their order.
#[test]
fn a_pair_and_select_distinct_past_the_grant_give_their_rows() {
    let (db, a, _threads) = table();
    let queries = ["SELECT T, COUNT(DISTINCT N) FROM C GROUP BY T", "SELECT DISTINCT G, N FROM C"];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(&parallel(&a, q), expected, "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1226) a grouping whose spilled
/// partitions outgrow the grant past SPILL.PARTITION_DEPTH, here 0, gives
/// way, the grouping given way to spilling in an area of its own beside
/// the one the partitions were spilled in, and then gives the rows it
/// gives run alone, in their order.
#[test]
fn partitions_past_the_depth_give_way() {
    let (db, a, threads) = table();
    let q = QUERIES.first().unwrap();
    let in_memory = alone(&a, q);
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    db.set_setting("SPILL.PARTITION_DEPTH", &Value::Exact(Decimal::new(0, 0))).unwrap();
    alone(&a, q);
    let serial = threads.taken().areas;
    assert_eq!(parallel(&a, q), in_memory);
    assert!(threads.taken().areas > serial, "the grouping's area and one of the grouping given way to");
}
