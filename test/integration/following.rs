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
