//! Fixtures and helpers shared by the integration tests.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Completion, Database, Session};
use turborel::diagnostics::{Diagnostics, Event, Level};
use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::hash::HashFunction;
use turborel::hash::crc::{CRC32_ISCSI, Crc};
use turborel::log::FileLogOptions;
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Concurrency, LockWait, TxCapabilities, TxId, Undo};
use turborel::value::{Decimal, Value};

/// Schema `APP` with tables `T` (hash index on NAME, btree on AMT) and `U`;
/// `T` holds rows 1-3 and `U` two rows.
pub fn setup() -> Database {
    fill(Database::new())
}

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
    fill(locking_database())
}

fn fill(db: Database) -> Database {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE T (ID INTEGER NOT NULL UNIQUE, NAME CHAR(6), AMT DECIMAL(6,2), F REAL)
           CREATE TABLE U (ID INTEGER NOT NULL, CODE CHAR(3) NOT NULL, UNIQUE (ID, CODE))",
    )
    .unwrap();
    db.create_index("T_NAME", "APP.T", &["NAME"], "hash").unwrap();
    db.create_index("T_AMT", "APP.T", &["AMT"], "btree").unwrap();
    {
        let s = db.session("APP");
        s.run("INSERT INTO T VALUES (1, 'ANN', 10.50, 1.5E0)").unwrap();
        s.run("INSERT INTO T VALUES (2, 'BOB', -3, 2)").unwrap();
        s.run("INSERT INTO T (ID) VALUES (3)").unwrap();
        s.run("INSERT INTO U VALUES (1, 'A')").unwrap();
        s.run("INSERT INTO U VALUES (1, 'B')").unwrap();
    }
    db
}

/// All result rows as strings, or the error code.
pub fn rows(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut c = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch().map_err(|e| e.code)? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// What `rows` returns for a query whose result is `list`.
#[expect(clippy::unnecessary_wraps, reason = "compared with the `Result` that `rows` returns")]
pub fn expected(list: &[&str]) -> Result<Vec<String>, Code> {
    Ok(list.iter().map(|s| (*s).to_owned()).collect())
}

/// All result rows of `sql` in `s`, sorted, for results in no set order.
pub fn sorted(s: &Session, sql: &str) -> Vec<String> {
    let mut found = rows(s, sql).unwrap();
    found.sort();
    found
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

/// Number of rows `sql` returns, or `usize::MAX` if it fails.
pub fn count(s: &Session, sql: &str) -> usize {
    rows(s, sql).map_or(usize::MAX, |r| r.len())
}

/// Runs `sql` with no parameters and returns its outcome.
pub fn exec(s: &Session, sql: &str) -> Result<Completion, Code> {
    s.execute(sql, &mut Params::new()).map_err(|e| e.code)
}

pub use turborel_test_support::Scratch;

/// A table in memory that leaves undo and isolation to the coordinator
/// (`TxCapabilities::default()`): the base of the test engines that check
/// what the coordinator does for such an engine, and of `LockingEngine`.
/// Rows are kept by id; a deleted row leaves a hole, so ids are never given
/// again.
#[derive(Default)]
pub struct MemoryTable {
    rows: Mutex<Vec<Option<Row>>>,
}

impl MemoryTable {
    /// Every live row, with its id, copied as of now: what a scan of it
    /// gives, and of the test engines that wrap it.
    pub fn live_rows(&self) -> Rows {
        let rows: Vec<_> = self.rows.lock().unwrap().iter().enumerate().filter_map(|(i, r)| Some((i as u64, r.clone()?))).collect();
        Box::new(rows.into_iter().map(Ok))
    }

    pub fn new() -> MemoryTable {
        MemoryTable::default()
    }
}

fn no_row<T>(id: RowId) -> turborel::error::Result<T> {
    Err(turborel::error::SqlError::new(Code::Api, format!("no row {id}")))
}

impl TableStorage for MemoryTable {
    fn insert(&self, _tx: TxId, row: Row) -> turborel::error::Result<RowId> {
        let mut rows = self.rows.lock().unwrap();
        rows.push(Some(row));
        Ok(rows.len() as u64 - 1)
    }
    fn update(&self, _tx: TxId, id: RowId, row: Row) -> turborel::error::Result<()> {
        match self.rows.lock().unwrap().get_mut(id as usize) {
            Some(Some(live)) => {
                *live = row;
                Ok(())
            }
            _ => no_row(id),
        }
    }
    fn delete(&self, _tx: TxId, id: RowId) -> turborel::error::Result<()> {
        match self.rows.lock().unwrap().get_mut(id as usize) {
            Some(slot @ Some(_)) => {
                *slot = None;
                Ok(())
            }
            _ => no_row(id),
        }
    }
    fn restore(&self, _tx: TxId, id: RowId, row: Row) -> turborel::error::Result<()> {
        let mut rows = self.rows.lock().unwrap();
        if rows.len() <= id as usize {
            rows.resize(id as usize + 1, None);
        }
        rows[id as usize] = Some(row);
        Ok(())
    }
    fn get(&self, _tx: TxId, id: RowId) -> turborel::error::Result<Option<Row>> {
        Ok(self.rows.lock().unwrap().get(id as usize).cloned().flatten())
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.live_rows()
    }
    fn row_count(&self) -> usize {
        self.rows.lock().unwrap().iter().flatten().count()
    }
}

/// An engine of `MemoryTable`s, named "locking": its tables are isolated by
/// the coordinator's table locks and undone by its undo log.
pub struct LockingEngine;

/// The name of `LockingEngine`.
pub const LOCKING: &str = "locking";

impl StorageEngine for LockingEngine {
    fn name(&self) -> &'static str {
        LOCKING
    }
    fn create_table(&self, _desc: &TableDesc) -> turborel::error::Result<Box<dyn TableStorage>> {
        Ok(Box::new(MemoryTable::new()))
    }
}

/// A new database whose tables are in `LockingEngine`, by default.
pub fn locking_database() -> Database {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    db.use_engine(LOCKING).unwrap();
    db
}

/// CRC-32C, the checksum of the files the tests make.
pub fn crc32c() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// The operating system's file system.
pub fn os() -> Arc<dyn FileSystem> {
    Arc::new(OsFileSystem)
}

/// The operating system's file system as a process sees it whose earlier
/// openers of a database have all crashed: tests crash a database by
/// leaving it, then open it again, and the system gives a crashed process's
/// locks up (`FileSystem::lock`), so here no lock stands against another.
pub struct AfterCrash;

impl FileSystem for AfterCrash {
    fn open(
        &self,
        path: &std::path::Path,
        mode: turborel::disk::io::OpenMode,
    ) -> turborel::error::Result<Box<dyn turborel::disk::io::DiskFile>> {
        OsFileSystem.open(path, mode)
    }
    fn remove(&self, path: &std::path::Path) -> turborel::error::Result<()> {
        OsFileSystem.remove(path)
    }
    fn exists(&self, path: &std::path::Path) -> turborel::error::Result<bool> {
        OsFileSystem.exists(path)
    }
    fn create_dir_all(&self, path: &std::path::Path) -> turborel::error::Result<()> {
        OsFileSystem.create_dir_all(path)
    }
    fn read_dir(&self, path: &std::path::Path) -> turborel::error::Result<Vec<PathBuf>> {
        OsFileSystem.read_dir(path)
    }
    fn sync_dir(&self, path: &std::path::Path) -> turborel::error::Result<()> {
        OsFileSystem.sync_dir(path)
    }
    fn lock(&self, _path: &std::path::Path) -> turborel::error::Result<turborel::disk::io::FileLock> {
        Ok(turborel::disk::io::FileLock::new(()))
    }
}

/// `AfterCrash`, for a test that crashes a database and opens it again.
pub fn after_crash() -> Arc<dyn FileSystem> {
    Arc::new(AfterCrash)
}

/// An exact number of scale 0.
pub fn number(n: impl Into<i128>) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// `db` given a buffer pool of `pages` pages of 8 KiB, before it is
/// opened from its files, so pages are written back as it runs.
pub fn small_pool(db: &Database, pages: i128) {
    db.set_setting("STORAGE.POOL_BYTES", &number(pages * 8192)).unwrap();
}

/// File log options with 4 KiB segments, so a test fills several.
pub fn small_segments() -> FileLogOptions {
    FileLogOptions { segment_size: 4096, ..FileLogOptions::default() }
}

/// The built-in provider, counting the areas made and the bytes asked.
#[derive(Clone, Default)]
pub struct Counted(pub Arc<Mutex<Vec<usize>>>);

impl turborel::spill::Spill for Counted {
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

/// Run-length compression: a flag byte, then either the input as it is (0),
/// or (count, byte) pairs (1), whichever is shorter.
pub struct RunLength;

const RAW: u8 = 0;
const RUNS: u8 = 1;

impl Encoding for RunLength {
    fn id(&self) -> EncodingId {
        EncodingId(1001)
    }
    fn name(&self) -> &'static str {
        "test run-length"
    }
    fn overhead(&self, _: &str) -> usize {
        1
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        let mut runs = Vec::new();
        for chunk in input.chunk_by(|a, b| a == b) {
            for part in chunk.chunks(255) {
                runs.push(u8::try_from(part.len()).unwrap());
                runs.push(part[0]);
            }
        }
        if runs.len() < input.len() {
            out.push(RUNS);
            out.extend_from_slice(&runs);
        } else {
            out.push(RAW);
            out.extend_from_slice(input);
        }
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        match input.split_first() {
            Some((&RAW, data)) => out.extend_from_slice(data),
            Some((&RUNS, runs)) => {
                for pair in runs.chunks(2) {
                    out.extend(std::iter::repeat_n(pair[1], usize::from(pair[0])));
                }
            }
            _ => return Err(turborel::error::SqlError::new(Code::Corrupt, "not run-length encoded")),
        }
        Ok(())
    }
}

/// XORs each byte with a key stream derived from the context, and appends
/// the context, refusing to decode under another: what an authenticated
/// encryption would do, without the cryptography.
pub struct Scramble;

impl Encoding for Scramble {
    fn id(&self) -> EncodingId {
        EncodingId(1002)
    }
    fn name(&self) -> &'static str {
        "test scramble"
    }
    fn overhead(&self, _: &str) -> usize {
        8
    }
    fn confidential(&self) -> bool {
        true
    }
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        out.extend(input.iter().enumerate().map(|(i, &b)| b ^ key(context, i)));
        out.extend_from_slice(&context.to_le_bytes());
        Ok(())
    }
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        let (data, bound) = input.split_at(input.len().saturating_sub(8));
        if bound != context.to_le_bytes() {
            return Err(turborel::error::SqlError::new(Code::Corrupt, "scrambled for another place"));
        }
        out.extend(data.iter().enumerate().map(|(i, &b)| b ^ key(context, i)));
        Ok(())
    }
}

fn key(context: u64, i: usize) -> u8 {
    u8::try_from((context.wrapping_mul(31).wrapping_add(i as u64 * 7) + 0x5A) % 256).unwrap()
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

/// The rows of `sql`, sorted, read by batches where it can be, and read
/// row by row throughout (EXECUTOR.BATCHES OFF).
pub fn both_ways(s: &Session, sql: &str) -> (Vec<String>, Vec<String>) {
    let batched = sorted(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let rows = sorted(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    (batched, rows)
}
