//! Checkpoints that capture pages copy on write (RULES.md D25, D590): a
//! crash at each step of one taken while writers run, the walk of the
//! pages it owes among them, recovers exactly what was committed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use turborel::db::{Database, OpenOptions, Opened};

use crate::common::{Scratch, after_crash, number, small_pool, sorted};
use crate::faults::{FaultyFs, Op, Rule};

/// Rows of the table each run starts with.
const ROWS: u32 = 400;

/// A database in `dir` of `fs`, its pool small so pages are written back
/// as it runs, no checkpoint due but those asked for; table T of `ROWS`
/// rows, with an index on V.
fn opened(fs: &Arc<dyn turborel::disk::io::FileSystem>, dir: &std::path::Path, made: bool) -> Database {
    let db = Database::new();
    small_pool(&db, 24);
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    if made {
        db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(12))").unwrap();
        db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
        let s = db.session("APP");
        for k in 0..ROWS {
            s.run(&format!("INSERT INTO T VALUES ({k}, 'v{}')", k % 7)).unwrap();
        }
        s.commit().unwrap();
        db.checkpoint().unwrap();
    }
    db
}

/// Writers changing T until `stop`: updates of the rows there, inserts
/// of new ones, some rolled back; a statement failed by a fault is rolled
/// back and the writer goes on.
fn writers(db: &Database, stop: &Arc<AtomicBool>) -> Vec<thread::JoinHandle<()>> {
    (0..2_u32)
        .map(|w| {
            let (db, stop) = (db.clone(), Arc::clone(stop));
            thread::spawn(move || {
                let s = db.session("APP");
                let mut n = 0_u32;
                while !stop.load(Ordering::Acquire) {
                    let k = (n * 37 + w * 11) % ROWS;
                    let changed = s
                        .run(&format!("UPDATE T SET V = 'w{w}n{}' WHERE K = {k}", n % 100))
                        .and_then(|_| s.run(&format!("INSERT INTO T VALUES ({}, 'i{w}')", 10_000 + w * 100_000 + n)));
                    let ended = if changed.is_ok() && n % 5 != 4 { s.commit() } else { s.rollback() };
                    if ended.is_err() {
                        let _ = s.rollback();
                    }
                    n += 1;
                }
            })
        })
        .collect()
}

/// Readers of T until `stop`, by the key and by the index: their misses,
/// which a checkpoint's shut does not hold, evict pages as it notes them.
fn readers(db: &Database, stop: &Arc<AtomicBool>) -> Vec<thread::JoinHandle<()>> {
    (0..2_u32)
        .map(|r| {
            let (db, stop) = (db.clone(), Arc::clone(stop));
            thread::spawn(move || {
                let s = db.session("APP");
                while !stop.load(Ordering::Acquire) {
                    let query = if r == 0 { "SELECT COUNT(*) FROM T WHERE K >= 0" } else { "SELECT COUNT(*) FROM T WHERE V >= ''" };
                    let _ = s.run(query);
                    let _ = s.commit();
                }
            })
        })
        .collect()
}

/// The table as a query by the key and one by the index read it.
fn contents(db: &Database) -> (Vec<String>, Vec<String>) {
    let s = db.session("APP");
    (sorted(&s, "SELECT K, V FROM T"), sorted(&s, "SELECT V, COUNT(*) FROM T WHERE V >= '' GROUP BY V"))
}

/// One run: the database changed throughout, then a checkpoint taken
/// while writers run, `rule` failing one step of it or of a write back;
/// then a crash. Whether the rule fired.
fn run(scratch: &Scratch, n: usize, rule: Rule) -> bool {
    let dir = scratch.path(&format!("db{n}"));
    let (fs, faults) = FaultyFs::with_faults();
    let db = opened(&fs, &dir, true);
    db.session("APP").run("UPDATE T SET V = 'all'").unwrap();
    db.session("APP").commit().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let running = writers(&db, &stop);
    faults.inject(rule);
    let _ = db.checkpoint();
    stop.store(true, Ordering::Release);
    for w in running {
        w.join().unwrap();
    }
    let fired = faults.injected() > 0;
    faults.clear();
    let expected = contents(&db);
    drop(db);
    let db = opened(&after_crash(), &dir, false);
    assert_eq!(contents(&db), expected, "run {n}");
    fired
}

/// Verifies: [DESIGN] RULES.md D25, D590, a crash at each step of a
/// checkpoint taken while writers run: as its pages are captured, writers
/// copying pages it owes and evictions capturing them, before its batch
/// is committed; as the batch's side file is written, synced, ended by its
/// commit block and synced again; as the file is written, cut, grown and
/// synced; as the side file is removed; as the image is recorded. Each
/// step fails in turn, at its first operation and later ones, then the
/// database crashes: recovery gives exactly the rows committed, by the key
/// and by the index, the image recorded or not.
#[test]
fn a_crash_at_each_step_of_a_checkpoint_recovers() {
    let scratch = Scratch::new("checkpoint-phases");
    let steps = [(Op::Write, "shadow"), (Op::Sync, "shadow"), (Op::Write, "/data"), (Op::SetSize, "/data"), (Op::Sync, "/data")];
    let more = [(Op::Remove, "shadow"), (Op::Write, "catalog"), (Op::Sync, "catalog")];
    let mut n = 0;
    for (op, part) in steps.into_iter().chain(more) {
        for after in [0, 1, 3, 12, 48] {
            n += 1;
            if !run(&scratch, n, Rule::fail(op).on(part).after(after).times(1)) {
                break;
            }
        }
    }
    assert!(n >= 16, "steps failed: {n}");
}

/// Verifies: [DESIGN] RULES.md D590, checkpoints taken one after another
/// while writers and readers run, readers evicting pages as each
/// checkpoint notes them, then a crash: each checkpoint taken, each image
/// whole, the last recovered from with its log, the rows committed all
/// there and no other.
#[test]
fn checkpoints_while_writers_run_capture_whole_images() {
    let scratch = Scratch::new("checkpoint-images");
    let dir = scratch.path("db");
    let db = opened(&after_crash(), &dir, true);
    let stop = Arc::new(AtomicBool::new(false));
    let running: Vec<_> = writers(&db, &stop).into_iter().chain(readers(&db, &stop)).collect();
    for _ in 0..40 {
        db.checkpoint().unwrap();
    }
    stop.store(true, Ordering::Release);
    for w in running {
        w.join().unwrap();
    }
    let expected = contents(&db);
    drop(db);
    let db = Database::new();
    small_pool(&db, 24);
    let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(contents(&db), expected);
}
