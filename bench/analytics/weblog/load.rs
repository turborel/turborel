//! The web log's table made and filled (`weblog.rs`). The database is
//! kept in files. HITS is defined, and its rows are made a group at a
//! time and loaded by several sessions at once, each through a prepared
//! INSERT given a group's rows by column, as a bulk copy gives them, so
//! each goes straight to a frozen group. Then the maintenance due is
//! done, so the statistics are gathered. The memory the database may use
//! is sized for the rows as the reference runs' is for theirs. A
//! directory named to keep the table holds a
//! note of the rows it was filled with, and a later run that asks for the
//! same rows opens it and loads nothing.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use turborel::db::Database;
use turborel::params::Load;
use turborel::storage::lanes::Lanes;
use turborel::value::{Decimal, Value};

use super::super::harness::on_threads;
use super::rows::Maker;
use super::schema;

/// The buffer pool's bytes, enough that the table's pages fit and a hot
/// query reads none from its file.
const POOL_BYTES: i128 = 6 << 30;
/// The memory budget's bytes, for the pool and for work areas large
/// enough that no query of the rows made by default spills.
const BUDGET_BYTES: i128 = 20 << 30;
/// The most a grouping or sort holds before it spills.
const WORK_BYTES: i128 = 8 << 30;
/// The bytes of the headers of frozen groups the reference runs hold in
/// memory at most, a 64th of their memory budget of 112 GiB, and the rows
/// of the reference log, whose headers outgrow it. HITS is given the same
/// bytes a row, so a scan meets the headers as it does there.
const REFERENCE_HEADERS: (u128, u128) = (1_879_048_192, 99_997_497);
/// The rows of an execution of the load, a group's by default
/// (COLUMNSTORE.GROUP_ROWS), so each execution writes one whole group.
const GROUP: u64 = 65_536;
/// The file of a kept directory that notes the rows it holds.
const NOTE: &str = "weblog.rows";

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A database with the bench's settings, for `rows` rows, its files in
/// `dir`. Nothing is synced, as a table that can be made again needs no
/// durability.
fn opened(dir: &Path, rows: u64) -> Database {
    let db = Database::new();
    let (held, of) = REFERENCE_HEADERS;
    db.set_setting("COLUMNSTORE.HEADER_CACHE_BYTES", &number((held * u128::from(rows)).div_euclid(of).cast_signed())).unwrap();
    db.set_setting("MEMORY.BUDGET_BYTES", &number(BUDGET_BYTES)).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &number(POOL_BYTES)).unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &number(WORK_BYTES)).unwrap();
    db.set_setting("LOG.DURABILITY", &Value::Char("NONE".into())).unwrap();
    let fs = db.file_system().unwrap();
    db.open_files(&fs, dir, &turborel::db::OpenOptions::default()).unwrap();
    db.set_setting("LOG.COMMIT_SYNC", &Value::Char("OFF".into())).unwrap();
    db
}

/// The database of HITS filled by `maker`, in `dir`. It is opened as it
/// is if `kept` and the directory notes the same rows, and made anew
/// otherwise.
pub fn database(dir: &Path, kept: bool, maker: &Maker, threads: usize) -> Database {
    let note = dir.join(NOTE);
    if kept && std::fs::read_to_string(&note).is_ok_and(|held| held == maker.label()) {
        return opened(dir, maker.rows());
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let db = opened(dir, maker.rows());
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let s = db.session("B");
    for definition in schema::definitions() {
        s.run(&definition).unwrap();
    }
    s.commit().unwrap();
    drop(s);
    load(&db, maker, threads);
    db.maintain().unwrap();
    if kept {
        std::fs::write(&note, maker.label()).unwrap();
    }
    db
}

/// Loads every row of `maker` into HITS on `threads` threads, each with
/// a session of its own that makes the next group's rows not yet taken
/// and inserts them as one execution of the load, so each is one frozen
/// group and the groups are written at once, as a copy of several streams
/// writes them. The groups' order in the table is the order they were
/// finished in.
fn load(db: &Database, maker: &Maker, threads: usize) {
    let starts: Vec<u64> = (0..maker.rows()).step_by(GROUP as usize).collect();
    let next = AtomicUsize::new(0);
    on_threads(threads, |_| {
        let s = db.session("B");
        let insert = s.prepare(&schema::insert()).unwrap();
        while let Some(&start) = starts.get(next.fetch_add(1, Ordering::Relaxed)) {
            let lanes = maker.lanes(start, GROUP.min(maker.rows() - start));
            s.execute_prepared_lanes(&insert, Arc::new(lanes), Load::Going).unwrap();
        }
        s.execute_prepared_lanes(&insert, Arc::new(Lanes::default()), Load::Ending).unwrap();
        s.commit().unwrap();
    });
}
