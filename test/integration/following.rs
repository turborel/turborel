//! A replica's database kept in files, following its replica's log
//! (RULES.md D57): opened to follow (`Database::open_following`), it
//! recovers from its last checkpoint alone, never from the records of its
//! log after it, which a view change may yet replace; its checkpoints,
//! taken following, log nothing; and deposed, it is opened again in place
//! (`Standby::reopen`), the same handle, sessions begun before refused.

use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, LogStorage, OpenOptions, OpenedLog, Standby, StorePlace};
use turborel::disk::io::FileSystem;
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::{Code, Result};
use turborel::log::{Log, Lsn, MemoryLog};
use turborel::value::Value;

use crate::common::{exec, rows};

/// The directory of the following database.
const DIR: &str = "/db";

/// A log storage giving one log, as a replica's driver gives its own: the
/// database's directory holds no log of its own.
struct Given(Arc<MemoryLog>);

impl LogStorage for Given {
    fn name(&self) -> &'static str {
        "given"
    }
    fn open(&self, _place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        Ok(Some(OpenedLog { log: Arc::clone(&self.0) as Arc<dyn Log>, cut: 0_u64 }))
    }
    fn exists(&self, _place: &StorePlace<'_>) -> Result<bool> {
        Ok(false)
    }
    fn create(&self, _place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        Ok(Arc::clone(&self.0) as Arc<dyn Log>)
    }
}

/// A primary logging to `log`: schema APP, T with a UNIQUE key and an index
/// on V, rows 1 and 2 committed.
fn primary(log: &Arc<MemoryLog>) -> Database {
    let db = Database::new();
    db.set_log(Arc::clone(log) as Arc<dyn Log>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(10))").unwrap();
    db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    committed(&db, &["INSERT INTO T VALUES (1, 'one')", "INSERT INTO T VALUES (2, 'two')"]);
    db
}

/// Runs each of `statements` in a session of APP, committing after each.
fn committed(db: &Database, statements: &[&str]) {
    let s = db.session("APP");
    for sql in statements {
        exec(&s, sql).unwrap();
        s.commit().unwrap();
    }
}

/// A database opened from `fs` to follow `log`: it, and through where it
/// holds the log.
fn following(fs: &Arc<dyn FileSystem>, log: &Arc<MemoryLog>) -> (Database, Lsn) {
    let db = Database::new();
    db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into())).unwrap();
    db.set_log_storage(Box::new(Given(Arc::clone(log)))).unwrap();
    let holds = db.open_following(fs, Path::new(DIR), &OpenOptions::default()).unwrap();
    (db, holds)
}

/// The records of `log` after `holds`, through `through`, given to `db`.
fn give(db: &Database, log: &MemoryLog, holds: Lsn, through: Lsn) {
    let records: Vec<_> = log.read_from(Lsn(holds.0 + 1_u64)).take_while(|r| r.as_ref().is_ok_and(|(at, _)| *at <= through)).collect();
    Standby::replay(db, &mut records.into_iter()).unwrap();
}

/// T's rows, as `db` shows them.
fn shown(db: &Database) -> Vec<String> {
    rows(&db.session("APP"), "SELECT K, V FROM T ORDER BY K").unwrap()
}

/// The log's last position.
fn last(log: &MemoryLog) -> Lsn {
    Lsn(log.end().0 - 1_u64)
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §5.1], D25: a database opened to
/// follow, new, holds nothing and takes no change (SQLCODE -917); given
/// the log, it takes a checkpoint, logging nothing to it, a transaction of
/// its primary then open held; opened again from its files, it recovers
/// from that checkpoint alone, none of the records after it applied
/// though its log holds them, and follows from the first record of the
/// transaction it held: given the records again, it holds what its
/// primary holds, that transaction too once committed.
#[test]
fn a_following_database_recovers_from_its_checkpoint_alone() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let (db, holds) = following(&fs, &log);
    assert_eq!(holds, Lsn(0_u64));
    assert_eq!(db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER)").err().map(|e| e.code), Some(Code::TransactionState));
    let open = primary.session("APP");
    exec(&open, "INSERT INTO T VALUES (5, 'five')").unwrap();
    // Written, as a checkpoint writes an open transaction's records (RULES.md D470).
    primary.write_held().unwrap();
    let first_open = last(&log);
    committed(&primary, &["INSERT INTO T VALUES (3, 'three')"]);
    give(&db, &log, holds, last(&log));
    let before = log.end();
    db.checkpoint().unwrap();
    assert_eq!(log.end(), before, "nothing logged");

    committed(&primary, &["INSERT INTO T VALUES (4, 'four')", "UPDATE T SET V = 'uno' WHERE K = 1"]);
    give(&db, &log, Lsn(before.0 - 1_u64), last(&log));
    drop(db);
    let (again, holds) = following(&fs, &log);
    assert_eq!(holds, Lsn(first_open.0 - 1_u64), "it follows from the transaction it held");
    assert_eq!(shown(&again), ["1,one       ", "2,two       ", "3,three     "], "the checkpoint's alone");
    give(&again, &log, holds, last(&log));
    assert_eq!(shown(&again), shown(&primary), "but the transaction still open");
    open.commit().unwrap();
    give(&again, &log, Lsn(last(&log).0 - 1_u64), last(&log));
    assert_eq!(shown(&again), shown(&primary));
    // What a batch left collected (RULES.md D920): one entry a row.
    primary.maintain().unwrap();
    assert_eq!(again.index_entries("T_V").unwrap(), primary.index_entries("T_V").unwrap());
}

/// Verifies: [DESIGN] RULES.md D57: a following database's checkpoint is
/// refused (SQLCODE -910) until it has replayed again what the checkpoint
/// it was opened from holds.
#[test]
fn a_following_checkpoint_waits_for_its_image() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let open = primary.session("APP");
    exec(&open, "INSERT INTO T VALUES (5, 'five')").unwrap();
    primary.write_held().unwrap();
    committed(&primary, &["INSERT INTO T VALUES (3, 'three')"]);
    let (db, holds) = following(&fs, &log);
    give(&db, &log, holds, last(&log));
    db.checkpoint().unwrap();
    drop(db);
    let (again, holds) = following(&fs, &log);
    assert_eq!(again.checkpoint().err().map(|e| e.code), Some(Code::Api));
    give(&again, &log, holds, last(&log));
    again.checkpoint().unwrap();
}

/// Verifies: [DESIGN] RULES.md D57: deposed, a following database is
/// opened again in place (`Standby::reopen`): the same handle recovered
/// from its last checkpoint, its settings and jobs kept, following again,
/// through where it then holds the log given; a session begun before is
/// refused (SQLCODE -910), one begun after reads. One serving, or not
/// opened from its files, is refused (-910).
#[test]
fn a_deposed_database_is_opened_again_in_place() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let log = Arc::new(MemoryLog::new());
    let primary = primary(&log);
    let (db, holds) = following(&fs, &log);
    give(&db, &log, holds, last(&log));
    db.checkpoint().unwrap();
    let checkpointed = last(&log);
    committed(&primary, &["INSERT INTO T VALUES (3, 'three')"]);
    give(&db, &log, checkpointed, last(&log));
    let earlier = db.session("APP");
    assert_eq!(shown(&db).len(), 3_usize);
    let jobs = db.jobs().len();

    db.depose().unwrap();
    let holds = db.reopen().unwrap();
    assert_eq!(holds, checkpointed, "the checkpoint's position");
    assert_eq!(earlier.run("SELECT K FROM T").err().map(|e| e.code), Some(Code::Api));
    assert_eq!(shown(&db).len(), 2_usize, "recovered from the checkpoint");
    assert_eq!((db.setting("JOBS.POLICY").unwrap().to_string(), db.jobs().len()), ("MANUAL".to_owned(), jobs));
    give(&db, &log, holds, last(&log));
    assert_eq!(shown(&db), shown(&primary));

    let own = Arc::new(MemoryLog::new());
    db.promote(Arc::clone(&own) as Arc<dyn Log>).unwrap();
    assert_eq!(db.reopen().err().map(|e| e.code), Some(Code::Api), "serving");
    let memory = Database::new();
    memory.depose().unwrap();
    assert_eq!(memory.reopen().err().map(|e| e.code), Some(Code::Api), "not from files");
}

/// Verifies: [DESIGN] RULES.md D57, D470, a promoted database's change
/// spanning its checkpoint: serving, a transaction updates committed rows,
/// its records written open, a checkpoint is taken before its commit
/// record, and the rows are changed again after; deposed and opened again
/// in place from that checkpoint, given the log again from the
/// transaction's first record, it holds what it held serving, for each
/// seed of how its records were written and where a second update falls.
#[test]
fn a_change_spanning_a_promoted_checkpoint_is_followed_again() {
    for seed in 0_u64..4 {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let log = Arc::new(MemoryLog::new());
        let primary = primary(&log);
        let (db, holds) = following(&fs, &log);
        give(&db, &log, holds, last(&log));
        drop(primary);
        db.promote(Arc::clone(&log) as Arc<dyn Log>).unwrap();
        let spans = db.session("APP");
        exec(&spans, "UPDATE T SET V = 'spans' WHERE K = 1").unwrap();
        if seed % 2 == 0 {
            db.write_held().unwrap();
        }
        db.checkpoint().unwrap();
        if seed >= 2 {
            exec(&spans, "UPDATE T SET V = 'twice' WHERE K = 2").unwrap();
        }
        spans.commit().unwrap();
        committed(&db, &["UPDATE T SET V = 'after' WHERE K = 1", "DELETE FROM T WHERE K = 2"]);
        let served = shown(&db);
        db.depose().unwrap();
        let holds = db.reopen().unwrap();
        give(&db, &log, holds, last(&log));
        assert_eq!(shown(&db), served, "seed {seed}");
    }
}

/// Verifies: [DESIGN] RULES.md D57, D470, a change spanning a following
/// checkpoint: the primary's transaction updates and deletes committed
/// rows, its records written open, and the following database is given
/// them and checkpoints before their commit record; the rows are changed
/// again after. Opened again from that checkpoint and given the log from
/// the transaction's first record, or promoted after, it holds what the
/// primary holds, for each seed of where the later change falls.
#[test]
fn a_change_spanning_a_following_checkpoint_is_followed_again() {
    for seed in 0_u64..4 {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let log = Arc::new(MemoryLog::new());
        let primary = primary(&log);
        let (db, holds) = following(&fs, &log);
        give(&db, &log, holds, last(&log));
        let given = last(&log);
        let spans = primary.session("APP");
        exec(&spans, "UPDATE T SET V = 'spans' WHERE K = 1").unwrap();
        exec(&spans, "DELETE FROM T WHERE K = 2").unwrap();
        primary.write_held().unwrap();
        give(&db, &log, given, last(&log));
        db.checkpoint().unwrap();
        let checkpointed = last(&log);
        if seed % 2 == 1 {
            exec(&spans, "INSERT INTO T VALUES (2, 'again')").unwrap();
        }
        spans.commit().unwrap();
        committed(&primary, &["UPDATE T SET V = 'after' WHERE K = 1"]);
        if seed >= 2 {
            give(&db, &log, checkpointed, last(&log));
            assert_eq!(shown(&db), shown(&primary), "seed {seed}, given on");
        }
        drop(db);
        let (again, holds) = following(&fs, &log);
        give(&again, &log, holds, last(&log));
        assert_eq!(shown(&again), shown(&primary), "seed {seed}");
        let own = Arc::new(MemoryLog::new());
        again.promote(Arc::clone(&own) as Arc<dyn Log>).unwrap();
        committed(&again, &["UPDATE T SET V = 'promoted' WHERE K = 1", "DELETE FROM T WHERE K = 1"]);
        assert_eq!(rows(&again.session("APP"), "SELECT COUNT(*) FROM T WHERE K = 1").unwrap(), ["0"], "seed {seed}");
    }
}

/// A database serving from files of its own, logging to `log`: T of 1200
/// rows of V 'base', then two transactions open across a checkpoint, the
/// first updating and deleting some rows, the second, whose records follow
/// the first's, one more. By `seed`'s bits: the first past what it holds,
/// its records written as it goes, or not, the checkpoint writing them;
/// the first updating a row again after or not; both committing or, an
/// index on T made while they are open, rolled back. The rows are then
/// changed again.
fn spanned(log: &Arc<MemoryLog>, seed: u64) -> Database {
    let (past, twice, commits) = (seed & 1 == 0, seed & 2 != 0, seed & 4 == 0);
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let source = Database::new();
    source.set_log_storage(Box::new(Given(Arc::clone(log)))).unwrap();
    source.open_files(&fs, Path::new("/source"), &OpenOptions::default()).unwrap();
    source.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(1000))").unwrap();
    let s = source.session("APP");
    for k in 0..1_200_i32 {
        exec(&s, &format!("INSERT INTO T VALUES ({k}, 'base')")).unwrap();
    }
    s.commit().unwrap();
    let (spans, second) = (source.session("APP"), source.session("APP"));
    exec(&spans, &format!("UPDATE T SET V = 'spans' WHERE K < {}", if past { 1_100 } else { 10 })).unwrap();
    exec(&spans, "DELETE FROM T WHERE K = 1150").unwrap();
    exec(&second, "UPDATE T SET V = 'second' WHERE K = 1199").unwrap();
    source.checkpoint().unwrap();
    if twice {
        exec(&spans, "UPDATE T SET V = 'twice' WHERE K = 0").unwrap();
    }
    if commits {
        spans.commit().unwrap();
        second.commit().unwrap();
    } else {
        // An index made meanwhile reads every version of T's rows, and
        // has the transactions roll back.
        source.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
        spans.rollback().unwrap();
        second.rollback().unwrap();
    }
    committed(&source, &["UPDATE T SET V = 'after' WHERE K < 5", "DELETE FROM T WHERE K = 6"]);
    source
}

/// The count of T's rows of each value of V, as `db` shows them.
fn values(db: &Database) -> Vec<String> {
    rows(&db.session("APP"), "SELECT V, COUNT(*) FROM T GROUP BY V ORDER BY V").unwrap()
}

/// Verifies: [DESIGN] RULES.md D57, D470, a state transfer of a checkpoint
/// a transaction spans (`spanned`), for each of its seeds: a database
/// following installs the primary's export, holding the group's log, and
/// given the records after where it then holds it, holds what the
/// primary holds; promoted, it changes those rows again.
#[test]
fn a_transfer_of_a_checkpoint_a_transaction_spans_is_followed() {
    for seed in 0_u64..8 {
        let log = Arc::new(MemoryLog::new());
        let source = spanned(&log, seed);
        let (db, _) = following(&(Arc::new(MemoryFileSystem::new()) as Arc<dyn FileSystem>), &log);
        let holds = Standby::import(&db, source.export().unwrap()).unwrap();
        give(&db, &log, holds, last(&log));
        assert_eq!(values(&db), values(&source), "seed {seed}");
        db.promote(Arc::new(MemoryLog::new()) as Arc<dyn Log>).unwrap();
        committed(&db, &["UPDATE T SET V = 'promoted' WHERE K < 20", "DELETE FROM T WHERE K = 1"]);
        assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'promoted'").unwrap(), ["18"], "seed {seed}");
    }
}

/// Verifies: [DESIGN] RULES.md D57, a database following whose replica's
/// log no longer holds every record before its checkpoint's position, as
/// a crash after a state transfer leaves it once its replica cut the log
/// to the position it holds it through (a view change may replace the
/// records after): opened to follow, its files are of no use, the
/// records the open transactions are undone with gone, so it is made
/// again from nothing, holding the log through 0; the group's state
/// fetched again, its log holding the records again, and given the
/// records after, it holds what the primary holds; for each seed of
/// `spanned`. Regression test (R-TEST-7): it recovered from the checkpoint
/// with the records it had, so a transaction open there whose records
/// were cut off was not undone. In the replication simulation
/// (`reconfigure`, seed 1) replaying an index made after then read one of
/// its versions' older ones, gone with the undo store of the database
/// exported (SQLCODE -916). In one process the rows here come back right
/// even so, as the dropped records' versions are not read through to an
/// older one, so the test asserts the database is made again.
#[test]
fn a_following_database_whose_log_was_cut_is_made_again() {
    for seed in (0_u64..8).rev() {
        let log = Arc::new(MemoryLog::new());
        let source = spanned(&log, seed);
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let (db, _) = following(&fs, &log);
        let installed = Standby::import(&db, source.export().unwrap()).unwrap();
        drop(db);
        // The replica's log as the crash left it, through two records
        // after where the transfer held it; those let go of are empty.
        let records: Vec<(Lsn, Vec<u8>)> = log.read_from(Lsn(1)).map(Result::unwrap).collect();
        let cut = Arc::new(MemoryLog::new());
        let first = records.first().map_or(1, |(at, _)| at.0);
        for _ in 1..first {
            cut.append(&[]).unwrap();
        }
        let (kept, cut_off): (Vec<_>, Vec<_>) = records.into_iter().partition(|(at, _)| at.0 <= installed.0 + 2);
        for (_, record) in &kept {
            cut.append(record).unwrap();
        }
        let (again, holds) = following(&fs, &cut);
        assert_eq!(holds, Lsn(0), "seed {seed}");
        for (_, record) in &cut_off {
            cut.append(record).unwrap();
        }
        let holds = Standby::import(&again, source.export().unwrap()).unwrap();
        give(&again, &log, holds, last(&log));
        assert_eq!(values(&again), values(&source), "seed {seed}");
    }
}
