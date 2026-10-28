//! The database shared between threads (RULES.md D27): handles that cross
//! threads, sessions running at once, and statements that keep the
//! definitions they started with while other threads change them.

use std::sync::{Arc, Barrier};
use std::thread;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::txn::LockWait;

use crate::common::{count, rows};

/// Fails to compile unless `T` may be shared between threads.
fn is_send_and_sync<T: Send + Sync>() {}
/// Fails to compile unless `T` may be moved between threads.
fn is_send<T: Send>() {}

/// Threads, each with its own session and table.
const THREADS: usize = 4;
/// Rows each thread inserts.
const ROWS_PER_THREAD: usize = 50;

/// A database with one table per thread, `W.T0`, `W.T1`, ...
fn tables_per_thread() -> Database {
    let db = Database::new();
    let tables: Vec<String> = (0..THREADS).map(|i| format!("CREATE TABLE T{i} (K INTEGER NOT NULL UNIQUE)")).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION W {}", tables.join(" "))).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D27: a `Database` may be shared between
/// threads, and a `Session` moved to another thread.
#[test]
fn handles_cross_threads() {
    is_send_and_sync::<Database>();
    is_send::<Session>();
}

/// Verifies: [X3.135-1986 §4.16] transactions on different threads run at
/// once and each commits all its rows; [DESIGN] RULES.md D27.
#[test]
fn sessions_write_at_once_on_separate_threads() {
    let db = tables_per_thread();
    let writers: Vec<_> = (0..THREADS)
        .map(|i| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("W");
                for k in 0..ROWS_PER_THREAD {
                    s.run(&format!("INSERT INTO T{i} VALUES ({k})")).unwrap();
                }
                s.commit().unwrap();
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    assert_eq!(db.active_transactions().unwrap(), 0, "every writer committed");
    let s = db.session("W");
    for i in 0..THREADS {
        assert_eq!(count(&s, &format!("SELECT K FROM T{i}")), ROWS_PER_THREAD, "T{i}");
    }
}

/// Verifies: [X3.135-1986 §4.16] readers on different threads share a table
/// at once; [DESIGN] RULES.md D27.
#[test]
fn sessions_read_at_once_on_separate_threads() {
    let db = crate::common::setup();
    let readers: Vec<_> = (0..THREADS)
        .map(|_| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                let ids = count(&s, "SELECT ID FROM T");
                s.commit().unwrap();
                ids
            })
        })
        .collect();
    for r in readers {
        assert_eq!(r.join().unwrap(), 3);
    }
}

/// Verifies: [DESIGN] RULES.md D27: a statement or open cursor keeps the
/// definitions it started with; a change made meanwhile on another thread
/// is seen by the statements that start after it.
#[test]
fn an_open_cursor_keeps_its_definitions() {
    let db = crate::common::setup();
    let s = db.session("APP");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T").unwrap();
    s.execute("OPEN C", &mut turborel::params::Params::new()).unwrap();
    let other = db.clone();
    thread::spawn(move || other.schema("CREATE SCHEMA AUTHORIZATION LATER CREATE TABLE X (A INTEGER)").unwrap()).join().unwrap();
    let mut out = turborel::params::Params::new().declare(
        "OUT",
        turborel::value::DataType::Exact { precision: 5, scale: 0 },
        turborel::value::Value::Null,
    );
    assert!(s.execute("FETCH C INTO OUT", &mut out).is_ok(), "the open cursor reads through its own snapshot");
    assert_eq!(rows(&db.session("LATER"), "SELECT A FROM X"), Ok(vec![]), "a new statement sees the new table");
}

/// A scalar function with only a name, registered by `changes_take_turns`.
struct Named(String);
impl turborel::functions::ScalarFunction for Named {
    fn name(&self) -> &str {
        &self.0
    }
    fn return_type(&self, _: &[turborel::value::DataType]) -> turborel::error::Result<turborel::value::DataType> {
        Ok(turborel::value::DataType::Exact { precision: 5, scale: 0 })
    }
    fn call(&self, _: &[turborel::value::Value]) -> turborel::error::Result<turborel::value::Value> {
        Ok(turborel::value::Value::Null)
    }
}

/// Verifies: [DESIGN] RULES.md D27: changes of definitions made at once on
/// several threads take turns, so none is lost.
#[test]
fn changes_take_turns() {
    let db = Database::new();
    let registrars: Vec<_> = (0..THREADS)
        .map(|i| {
            let db = db.clone();
            thread::spawn(move || {
                for k in 0..ROWS_PER_THREAD {
                    db.register_scalar_function(Box::new(Named(format!("F{i}X{k}")))).unwrap();
                }
            })
        })
        .collect();
    for r in registrars {
        r.join().unwrap();
    }
    let mut s = db.session("A");
    s.enable_extensions(true);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    s.run("INSERT INTO T VALUES (1)").unwrap();
    for i in 0..THREADS {
        for k in 0..ROWS_PER_THREAD {
            assert_eq!(rows(&s, &format!("SELECT F{i}X{k}(K) FROM T")), Ok(vec!["NULL".into()]), "F{i}X{k}");
        }
    }
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] `LockWait::Wait`: a reader waits
/// for a writer's lock until the writer commits, then sees its row.
#[test]
fn a_reader_waits_for_a_writer_to_commit() {
    let db = crate::common::locking_setup();
    db.set_lock_wait(LockWait::Wait).unwrap();
    let writer = db.session("APP");
    writer.run("INSERT INTO T (ID) VALUES (9)").unwrap();
    let other = db.clone();
    let reader = thread::spawn(move || {
        let s = other.session("APP");
        let seen = count(&s, "SELECT ID FROM T");
        s.commit().unwrap();
        seen
    });
    writer.commit().unwrap();
    assert_eq!(reader.join().unwrap(), 4, "the reader saw the committed row");
}

/// Verifies: [DESIGN] `LockWait::Wait`: two transactions that each wait for a
/// lock the other holds are a deadlock; exactly one of them is refused with
/// SQLCODE -911 at once, and once it rolls back the other completes.
#[test]
fn a_deadlock_is_refused() {
    let db = crate::common::locking_setup();
    db.set_lock_wait(LockWait::Wait).unwrap();
    let both_read = Arc::new(Barrier::new(2));
    let sides = [("T", "INSERT INTO U VALUES (2, 'C')"), ("U", "INSERT INTO T (ID) VALUES (9)")];
    let threads: Vec<_> = sides
        .into_iter()
        .map(|(read, write)| {
            let (db, both_read) = (db.clone(), Arc::clone(&both_read));
            thread::spawn(move || {
                let s = db.session("APP");
                count(&s, &format!("SELECT ID FROM {read}"));
                both_read.wait();
                let outcome = s.run(write).map(|_| ()).map_err(|e| e.code);
                if outcome.is_ok() {
                    s.commit().unwrap();
                } else {
                    s.rollback().unwrap();
                }
                outcome
            })
        })
        .collect();
    let mut outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    outcomes.sort_by_key(Result::is_ok);
    assert_eq!(outcomes, vec![Err(Code::Conflict), Ok(())]);
}

/// Verifies: [EXTENSION] `create_index` under `LockWait::Wait` waits for an
/// open writer of a table whose engine leaves concurrency to the
/// coordinator, then builds the index under its exclusive lock. (On a
/// table that keeps versions the build does not wait: a writer it finds
/// open cannot commit, `db/indexes.rs`.)
#[test]
fn create_index_waits_for_an_open_writer() {
    let db = crate::common::locking_setup();
    db.set_lock_wait(LockWait::Wait).unwrap();
    let writer = db.session("APP");
    writer.run("INSERT INTO T (ID) VALUES (9)").unwrap();
    let other = db.clone();
    let indexer = thread::spawn(move || other.create_index("T_ID2", "APP.T", &["ID"], "btree").map_err(|e| e.code));
    writer.commit().unwrap();
    assert_eq!(indexer.join().unwrap(), Ok(()));
    assert_eq!(rows(&db.session("APP"), "SELECT ID FROM T WHERE ID = 9"), Ok(vec!["9".into()]));
}

/// Verifies: [DESIGN] `LockMode::Update`: an updatable cursor holds its
/// table's update lock, which readers share and another read that intends to
/// write does not; writing through the cursor waits for the readers to end.
#[test]
fn reads_that_intend_to_write_take_turns() {
    let db = crate::common::locking_setup();
    let (owner, reader, other) = (db.session("APP"), db.session("APP"), db.session("APP"));
    let out = turborel::value::DataType::Exact { precision: 9, scale: 0 };
    let mut p = turborel::params::Params::new().declare("OUT", out, turborel::value::Value::Null);
    owner.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T WHERE ID = 1").unwrap();
    owner.execute("OPEN C", &mut p).unwrap();
    owner.execute("FETCH C INTO OUT", &mut p).unwrap();
    assert_eq!(count(&reader, "SELECT ID FROM T"), 3, "a reader shares the update lock");
    other.declare_cursor("DECLARE D CURSOR FOR SELECT ID FROM T").unwrap();
    assert_eq!(other.execute("OPEN D", &mut p).map_err(|e| e.code), Err(Code::Conflict), "another updatable cursor");
    assert_eq!(other.run("DELETE FROM T WHERE ID = 2").map_err(|e| e.code), Err(Code::Conflict), "a searched DELETE");
    let update = "UPDATE T SET NAME = 'NEW' WHERE CURRENT OF C";
    assert_eq!(owner.run(update).map_err(|e| e.code), Err(Code::Conflict), "the reader still reads T");
    reader.commit().unwrap();
    owner.run(update).unwrap();
    owner.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT NAME FROM T WHERE ID = 1"), Ok(vec!["NEW   ".into()]));
}

/// Verifies: [DESIGN] `LockMode::Update` under `LockWait::Wait`: searched
/// UPDATEs of the same rows, each reading the table before writing it, on
/// threads at once, take turns rather than deadlock: every one completes.
#[test]
fn concurrent_updates_do_not_deadlock() {
    let db = crate::common::locking_setup();
    db.set_lock_wait(LockWait::Wait).unwrap();
    let start = Arc::new(Barrier::new(THREADS));
    let updaters: Vec<_> = (0..THREADS)
        .map(|_| {
            let (db, start) = (db.clone(), Arc::clone(&start));
            thread::spawn(move || {
                let s = db.session("APP");
                start.wait();
                for _ in 0..ROWS_PER_THREAD {
                    s.run("UPDATE T SET AMT = AMT + 1 WHERE ID = 1").unwrap();
                    s.commit().unwrap();
                }
            })
        })
        .collect();
    for u in updaters {
        u.join().unwrap();
    }
    assert_eq!(rows(&db.session("APP"), "SELECT AMT FROM T WHERE ID = 1"), Ok(vec!["210.50".into()]), "10.50 + 4 * 50");
}
