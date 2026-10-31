//! Fixtures over a database, through turborel's public API alone (RULES.md
//! R-TEST-10): a schema with rows, a query's rows as strings, an engine
//! whose tables keep nothing, the file systems and checksum of files made
//! by tests. The log suites use them (`logs.rs`), and so do the core's
//! integration tests, which include this file (`#[path]`) as they include
//! `types.rs`, so it is written once.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::panic_in_result_fn,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::default_numeric_fallback,
    clippy::wildcard_enum_match_arm,
    clippy::cognitive_complexity,
    clippy::shadow_unrelated,
    reason = "test code: a panic is a test failure, its fixtures literal, a database rebound once dropped (RULES.md R-TEST-4)"
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Database, Session};
use turborel::disk::io::{DiskFile, FileLock, FileSystem, OpenMode, OsFileSystem};
use turborel::error::{Code, Result, SqlError};
use turborel::hash::HashFunction;
use turborel::hash::crc::{CRC32_ISCSI, Crc};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::TxId;
use turborel::value::{Decimal, Value};

/// Schema `APP` with tables `T` (hash index on NAME, btree on AMT) and `U`;
/// `T` holds rows 1-3 and `U` two rows.
pub fn setup() -> Database {
    fill(Database::new())
}

/// `db` given schema `APP` and the rows of `setup`.
pub fn fill(db: Database) -> Database {
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
pub fn rows(s: &Session, sql: &str) -> std::result::Result<Vec<String>, Code> {
    let mut c = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch().map_err(|e| e.code)? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// `rows` of `sql`, which must succeed, sorted.
pub fn sorted(s: &Session, sql: &str) -> Vec<String> {
    let mut found = rows(s, sql).unwrap();
    found.sort();
    found
}

/// Number of rows `sql` returns, or `usize::MAX` if it fails.
pub fn count(s: &Session, sql: &str) -> usize {
    rows(s, sql).map_or(usize::MAX, |r| r.len())
}

/// The Information Schema's tables that show what a database is doing
/// now, not what it holds, so differ between two runs of it, or two
/// replicas of it.
pub const RUNNING: [&str; 3] = ["FILES", "JOBS", "MEMORY_RESERVATIONS"];

/// What `db` holds, as its users see it: each Information Schema table's
/// rows but those of `RUNNING`, each base table's rows, and each index's
/// entries, through a session of each of `readers`, sorted. Two databases
/// that hold the same rows and the same catalog give the same.
pub fn contents(db: &Database, readers: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for &user in readers {
        let s = db.session(user);
        let tables = rows(&s, "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE <> 'VIEW'").unwrap();
        for table in tables {
            let (schema, name) = table.split_once(',').unwrap();
            if schema == "INFORMATION_SCHEMA" && RUNNING.contains(&name) {
                continue;
            }
            let mut read = rows(&s, &format!("SELECT * FROM {schema}.{name}")).unwrap();
            read.sort();
            found.extend(read.into_iter().map(|row| format!("{user} {schema}.{name}: {row}")));
        }
        let indexes = rows(&s, "SELECT INDEX_NAME FROM INFORMATION_SCHEMA.INDEXES").unwrap();
        found.extend(indexes.iter().map(|ix| format!("{user} {ix}: {} entries", db.index_entries(ix.trim()).unwrap())));
        s.commit().unwrap();
    }
    found
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
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        OsFileSystem.open(path, mode)
    }
    fn remove(&self, path: &Path) -> Result<()> {
        OsFileSystem.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        OsFileSystem.exists(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        OsFileSystem.create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        OsFileSystem.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        OsFileSystem.sync_dir(path)
    }
    fn lock(&self, _path: &Path) -> Result<FileLock> {
        Ok(FileLock::new(()))
    }
}

/// `AfterCrash`, for a test that crashes a database and opens it again.
pub fn after_crash() -> Arc<dyn FileSystem> {
    Arc::new(AfterCrash)
}

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

    /// An empty table.
    pub fn new() -> MemoryTable {
        MemoryTable::default()
    }
}

fn no_row<T>(id: RowId) -> Result<T> {
    Err(SqlError::new(Code::Api, format!("no row {id}")))
}

impl TableStorage for MemoryTable {
    fn insert(&self, _tx: TxId, row: Row) -> Result<RowId> {
        let mut rows = self.rows.lock().unwrap();
        rows.push(Some(row));
        Ok(rows.len() as u64 - 1)
    }
    fn update(&self, _tx: TxId, id: RowId, row: Row) -> Result<()> {
        match self.rows.lock().unwrap().get_mut(id as usize) {
            Some(Some(live)) => {
                *live = row;
                Ok(())
            }
            _ => no_row(id),
        }
    }
    fn delete(&self, _tx: TxId, id: RowId) -> Result<()> {
        match self.rows.lock().unwrap().get_mut(id as usize) {
            Some(slot @ Some(_)) => {
                *slot = None;
                Ok(())
            }
            _ => no_row(id),
        }
    }
    fn restore(&self, _tx: TxId, id: RowId, row: Row) -> Result<()> {
        let mut rows = self.rows.lock().unwrap();
        if rows.len() <= id as usize {
            rows.resize(id as usize + 1, None);
        }
        rows[id as usize] = Some(row);
        Ok(())
    }
    fn get(&self, _tx: TxId, id: RowId) -> Result<Option<Row>> {
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
    fn create_table(&self, _desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(MemoryTable::new()))
    }
}
