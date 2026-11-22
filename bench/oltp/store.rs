//! The database a workload runs on: kept in files in the harness's
//! directory, durable as a server keeps it by default, each commit waiting
//! for its log record to be synced (LOG.COMMIT_SYNC ON, LOG.DURABILITY
//! DATA), unless the run turns the commit's sync off to tell the engine's
//! work from the waits for the disk. A row's second writer waits for the
//! first, as in a database a server serves (LOCKS.WAIT ON).

use std::path::Path;

use turborel::db::Database;
use turborel::value::{Decimal, Value};

/// The settings each database is opened with.
#[derive(Clone, Copy)]
pub struct Store {
    /// Whether each commit waits for its log record to be synced.
    pub sync: bool,
    /// The bytes of the buffer pool, enough to hold every table.
    pub pool_bytes: i128,
}

/// The memory budget beyond the pool, for sorts, groupings and caches.
const WORK_BYTES: i128 = 1 << 30;

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn word(w: &str) -> Value {
    Value::Char(w.to_owned())
}

impl Store {
    /// A new database in `dir`.
    pub fn open(&self, dir: &Path) -> Database {
        let _ = std::fs::remove_dir_all(dir);
        let db = Database::new();
        db.set_setting("MEMORY.BUDGET_BYTES", &number(self.pool_bytes + WORK_BYTES)).unwrap();
        db.set_setting("STORAGE.POOL_BYTES", &number(self.pool_bytes)).unwrap();
        db.set_setting("LOG.DURABILITY", &word("DATA")).unwrap();
        let fs = db.file_system().unwrap();
        db.open_files(&fs, dir, &turborel::db::OpenOptions::default()).unwrap();
        db.set_setting("LOG.COMMIT_SYNC", &word(if self.sync { "ON" } else { "OFF" })).unwrap();
        db.set_setting("LOCKS.WAIT", &word("ON")).unwrap();
        db
    }
}
