//! Fixtures and helpers shared by the integration tests.

mod dynamic;
mod encodings;
mod settings;

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Completion, Database, Session};
use turborel::diagnostics::{Diagnostics, Event, Level};
use turborel::error::Code;
use turborel::log::FileLogOptions;
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Concurrency, LockWait, TxCapabilities, TxId, Undo};
use turborel::value::Value;

// Written once in `support/`, for the suites every log passes too.
pub use crate::database::{LOCKING, LockingEngine, MemoryTable, after_crash, count, crc32c, number, os, rows, setup, small_pool, sorted};
pub use crate::logs::Watched;
pub use dynamic::{code_with, dynamic_host, run_with, set_text};
pub use encodings::{RunLength, Scramble};
pub use settings::{Noted, whole};

/// Schema D: tables T (btree index on N) and U, each holding 1-3; views V
/// over T and W over V, X over U, S all of T; P may read T and V.
pub fn with_views() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION D
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, N CHAR(4))
           CREATE TABLE U (K INTEGER)
           CREATE VIEW V AS SELECT K FROM T WHERE K > 1
           CREATE VIEW W AS SELECT K FROM V
           CREATE VIEW X AS SELECT K FROM U
           CREATE VIEW S AS SELECT * FROM T
           GRANT SELECT ON T TO P
           GRANT SELECT ON V TO P",
    )
    .unwrap();
    db.create_index("T_N", "D.T", &["N"], "btree").unwrap();
    let s = db.session("D");
    for k in 1..=3 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'N{k}')")).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// `setup`, its tables in `LockingEngine`: isolated by the coordinator's
/// table locks, for the tests of those locks.
pub fn locking_setup() -> Database {
    crate::database::fill(locking_database())
}

/// What `rows` returns for a query whose result is `list`.
#[expect(clippy::unnecessary_wraps, reason = "compared with the `Result` that `rows` returns")]
pub fn expected(list: &[&str]) -> Result<Vec<String>, Code> {
    Ok(list.iter().map(|s| (*s).to_owned()).collect())
}

/// The error code of `sql`, which must fail.
pub fn code(s: &Session, sql: &str) -> Code {
    rows(s, sql).expect_err(sql)
}

/// `setup` with T at 60 rows and a hash index on ID: rows 4-60 have NAME
/// `N` and the ID, null for every 10th, AMT = ID mod 7, and F 0.
pub fn big() -> Database {
    more_rows(setup())
}

/// `big`, its tables in `LockingEngine`.
pub fn locking_big() -> Database {
    more_rows(locking_setup())
}

fn more_rows(db: Database) -> Database {
    db.create_index("T_ID_HASH", "APP.T", &["ID"], "hash").unwrap();
    let s = db.session("APP");
    for i in 4..=60 {
        let name = if i % 10 == 0 { "NULL".to_string() } else { format!("'N{i}'") };
        s.run(&format!("INSERT INTO T VALUES ({i}, {name}, {}, 0)", i % 7)).unwrap();
    }
    drop(s);
    db
}

/// The bytes of `db`'s memory budget its spill areas and their files hold,
/// as INFORMATION_SCHEMA.MEMORY_RESERVATIONS shows the database's owner:
/// OWNER, made so if no one is.
pub fn spill_bytes(db: &Database) -> usize {
    // Made the first time; refused, as they are, every time after.
    let _ = db.create_user("OWNER", b"x");
    let _ = db.set_owner("OWNER");
    let sql = "SELECT COALESCE(SUM(BYTES), 0) FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS WHERE RESERVED_BY LIKE 'a spill%'";
    rows(&db.session("OWNER"), sql).unwrap().first().unwrap().parse().unwrap()
}

/// Runs `sql` with no parameters and returns its outcome.
pub fn exec(s: &Session, sql: &str) -> Result<Completion, Code> {
    s.execute(sql, &mut Params::new()).map_err(|e| e.code)
}

pub use turborel_test_support::Scratch;

/// A new database whose tables are in `LockingEngine`, by default.
pub fn locking_database() -> Database {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    db.use_engine(LOCKING).unwrap();
    db
}

/// The bytes of the files in `dir`, and below it, whose names begin with
/// `prefix`: `data` for the row store's file and its shadows, `log` for
/// the log's segments.
pub fn bytes_of(dir: &std::path::Path, prefix: &str) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let named = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix));
        if path.is_dir() {
            all.extend(bytes_of(&path, if named { "" } else { prefix }));
        } else if named {
            all.extend(std::fs::read(&path).unwrap());
        }
    }
    all
}

/// Whether `bytes` hold `text`.
pub fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes())
}

/// File log options with 4 KiB segments, so a test fills several.
pub fn small_segments() -> FileLogOptions {
    FileLogOptions { segment_size: 4096, ..FileLogOptions::default() }
}

/// The built-in provider, counting the areas made and the bytes asked.
#[derive(Clone, Default)]
pub struct Counted(pub Arc<Mutex<Vec<usize>>>);

impl turborel::spill::Spill for Counted {
    fn name(&self) -> &'static str {
        "counted"
    }
    fn area(
        &self,
        memory: &Arc<turborel::memory::MemoryBudget>,
        temp: &Arc<turborel::spill::TempFiles>,
        area: turborel::spill::Area,
    ) -> turborel::error::Result<Box<dyn turborel::spill::SpillArea>> {
        self.0.lock().unwrap().push(area.bytes);
        turborel::spill::Spill::area(&turborel::spill::PagedSpill, memory, temp, area)
    }
}

impl Counted {
    /// The areas made since the last call.
    pub fn taken(&self) -> usize {
        self.0.lock().unwrap().drain(..).count()
    }
}

/// A sink that keeps every event, and its level.
#[derive(Clone, Default)]
pub struct Kept(Arc<Mutex<Vec<(Level, Event)>>>);

impl Diagnostics for Kept {
    fn event(&self, event: &Event) {
        self.0.lock().unwrap().push((event.level(), event.clone()));
    }
}

impl Kept {
    /// The events kept since the last call, as `Debug` shows them, with
    /// their levels.
    pub fn taken(&self) -> Vec<(Level, String)> {
        self.0.lock().unwrap().drain(..).map(|(l, e)| (l, format!("{e:?}"))).collect()
    }
}

/// A table's gate: once armed, the first scan to reach its second row says
/// so, and waits to be told to go on; scans after pass until it is armed
/// again.
#[derive(Clone)]
pub struct Gate {
    armed: Arc<std::sync::atomic::AtomicBool>,
    reached: Arc<Mutex<Sender<()>>>,
    go: Arc<Mutex<Receiver<()>>>,
}

/// An engine, `gated`, whose tables' scans stop at the second row until
/// their gate opens: so a test cancels a statement while it reads, at a
/// point it knows, or holds it reading. A test-local plug-in (RULES.md D6).
struct Gated(Gate);

struct GatedTable {
    rows: Arc<MemoryTable>,
    gate: Gate,
}

impl StorageEngine for Gated {
    fn name(&self) -> &'static str {
        "gated"
    }
    fn create_table(&self, _: &TableDesc) -> turborel::error::Result<Box<dyn TableStorage>> {
        Ok(Box::new(GatedTable { rows: Arc::new(MemoryTable::new()), gate: self.0.clone() }))
    }
}

impl TableStorage for GatedTable {
    fn insert(&self, tx: TxId, row: Row) -> turborel::error::Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> turborel::error::Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> turborel::error::Result<()> {
        self.rows.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> turborel::error::Result<()> {
        self.rows.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> turborel::error::Result<Option<Row>> {
        self.rows.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        let gate = self.gate.clone();
        let mut read = 0_usize;
        Box::new(self.rows.live_rows().inspect(move |_| {
            read += 1;
            if read == 2 && gate.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                gate.reached.lock().unwrap().send(()).unwrap();
                gate.go.lock().unwrap().recv().unwrap();
            }
        }))
    }
    fn row_count(&self) -> usize {
        self.rows.row_count()
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { undo: Undo::Log, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
}

/// A database whose table X.G, of the gated engine, holds 1, 2 and 3; its
/// gate, not armed; and the gate's ends: to hear a scan reach its second
/// row, and to let it go on.
pub fn gated() -> (Database, Gate, Receiver<()>, Sender<()>) {
    let (reached, heard) = channel();
    let (go, wait) = channel();
    let db = Database::new();
    let gate = Gate { armed: Arc::default(), reached: Arc::new(Mutex::new(reached)), go: Arc::new(Mutex::new(wait)) };
    db.register_engine(Box::new(Gated(gate.clone()))).unwrap();
    db.place_table("X.G", "gated").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE G (K INTEGER)").unwrap();
    let s = db.session("X");
    for k in 1..=3 {
        s.run(&format!("INSERT INTO G VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    (db, gate, heard, go)
}

/// Arms `db`'s gate: the next scan to reach a second row waits at it.
pub fn arm(gate: &Gate) {
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// A database whose table X.L, of the locking engine, its session `holder`
/// holds locked, by a row it inserted and has not committed; statements
/// wait for locks.
pub fn locked() -> (Database, Session) {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    db.place_table("X.L", LOCKING).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE L (K INTEGER)").unwrap();
    let holder = db.session("X");
    exec(&holder, "INSERT INTO L VALUES (1)").unwrap();
    (db, holder)
}

/// Turns every optimizer rule of `db` on or off, as `enabled` says.
pub fn set_all_rules(db: &Database, enabled: bool) {
    db.with_optimizer(|o| o.rules()).unwrap().into_iter().for_each(|name| set_rule(db, name, enabled));
}

/// Turns the function call syntax on for `s` (SQL.FUNCTION_CALLS).
pub fn function_calls(s: &Session) {
    s.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".to_owned())).unwrap();
}

/// Turns the optimizer rule `rule` of `db` on or off, by its setting (RULES.md D36).
pub fn set_rule(db: &Database, rule: &str, enabled: bool) {
    db.set_setting(&turborel::optimizer::switch_name(rule), &Value::Char(if enabled { "ON" } else { "OFF" }.to_owned())).unwrap();
}

/// The rows of `sql`, sorted, read by batches where it can be, and read
/// row by row throughout (EXECUTOR.BATCHES OFF).
pub fn both_ways(s: &Session, sql: &str) -> (Vec<String>, Vec<String>) {
    let batched = sorted(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows = sorted(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (batched, rows)
}

/// xorshift64*: a small generator, enough to pick among forms and values:
/// the random tests' (RULES.md R-TEST-5), from a fixed seed.
pub struct Rng(u64);

impl Rng {
    /// A generator from `seed`; xorshift needs a state not zero.
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1_u64))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12_u32;
        self.0 ^= self.0 << 25_u32;
        self.0 ^= self.0 >> 27_u32;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D_u64)
    }

    /// A number in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    /// True `percent` times in a hundred.
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100_usize) < percent
    }

    pub fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    /// A small integer, as SQL text, negative ones among them.
    pub fn number(&mut self) -> String {
        (i64::try_from(self.below(21_usize)).unwrap() - 6_i64).to_string()
    }
}
