//! Backups while the database serves (RULES.md D160, D166). One taken as
//! writers commit holds a prefix of their commits. One interrupted by a
//! crash leaves no backup, and the history shows it interrupted. One
//! cancelled is removed. Two run at once, and only the owner takes one.
//! The history outlives a reopen and is found again from a target.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use turborel::backup::{BackupTarget, DirectoryTarget, TargetProvider};
use turborel::clock::FixedClock;
use turborel::db::backup::BackupOptions;
use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result, SqlError};

use crate::backups::{base, contents, dir_target, open, options, restored};
use crate::common::{Scratch, after_crash, exec, rows};

/// A directory target that, at its `at`th write, tells `reached` and waits
/// for `go`: true to go on, false to fail.
struct Pausing {
    inner: DirectoryTarget,
    writes: Mutex<u64>,
    at: u64,
    reached: Mutex<Sender<()>>,
    go: Mutex<Receiver<bool>>,
}

impl BackupTarget for Pausing {
    fn location(&self) -> String {
        self.inner.location()
    }
    fn begin(&self, name: &str) -> Result<u64> {
        self.inner.begin(name)
    }
    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()> {
        let n = {
            let mut writes = self.writes.lock().unwrap();
            *writes += 1;
            *writes
        };
        if n == self.at {
            self.reached.lock().unwrap().send(()).unwrap();
            if !self.go.lock().unwrap().recv().unwrap() {
                return Err(SqlError::new(Code::Io, "the target went away"));
            }
        }
        self.inner.write(upload, bytes)
    }
    fn finish(&self, upload: u64) -> Result<()> {
        self.inner.finish(upload)
    }
    fn abandon(&self, upload: u64) -> Result<()> {
        self.inner.abandon(upload)
    }
    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>> {
        self.inner.read(name, offset, max)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix)
    }
    fn remove(&self, name: &str) -> Result<()> {
        self.inner.remove(name)
    }
}

/// The provider of the scheme "pausing", of the one target it holds.
struct PausingProvider(Arc<Pausing>);

impl TargetProvider for PausingProvider {
    fn scheme(&self) -> &'static str {
        "pausing"
    }
    fn open(&self, _: &str) -> Result<Arc<dyn BackupTarget>> {
        Ok(Arc::clone(&self.0) as Arc<dyn BackupTarget>)
    }
}

/// A pausing target in `dir`, registered with `db`, pausing at its `at`th
/// write: how to know it paused, and how to let it go.
fn pausing(db: &Database, dir: &std::path::Path, at: u64) -> (Receiver<()>, Sender<bool>) {
    let (reached, paused) = channel();
    let (go, wait) = channel();
    let inner = DirectoryTarget::new("pausing:t", dir).unwrap();
    let target = Pausing { inner, writes: Mutex::new(0), at, reached: Mutex::new(reached), go: Mutex::new(wait) };
    db.register_target_provider(Arc::new(PausingProvider(Arc::new(target)))).unwrap();
    (paused, go)
}

/// Copies the directory `from` to `to`, as a crash would leave it.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let path = e.unwrap().path();
        let into = to.join(path.file_name().unwrap());
        if path.is_dir() { copy_dir(&path, &into) } else { std::fs::copy(&path, &into).map(drop).unwrap() }
    }
}

/// Verifies: [EXTENSION] RULES.md D160: a backup taken while a writer
/// commits a row a transaction holds every row committed before its
/// stop, a prefix of the writer's, and the rows before it began.
#[test]
fn a_backup_taken_as_writers_commit_holds_a_prefix() {
    let scratch = Scratch::new("backup-online");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = Arc::new(open(&scratch.path("db"), &clock));
    base(&db, 100);
    let location = dir_target(&scratch.path("target"));
    let writer = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || {
            let s = db.session("APP");
            for k in 1_000..1_400 {
                s.run(&format!("INSERT INTO T VALUES ({k}, 'w')")).unwrap();
                s.run("COMMIT").unwrap();
            }
        })
    };
    let before = rows(&db.session("APP"), "SELECT COUNT(*) FROM T").unwrap();
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    writer.join().unwrap();
    let back = restored(&options(&scratch.path("back"), &location), &clock);
    let got: Vec<i64> = contents(&back).iter().map(|r| r.split(',').next().unwrap().parse().unwrap()).collect();
    assert!(got.len() >= before[0].parse::<usize>().unwrap(), "{} rows", got.len());
    let expected: Vec<i64> = (0..100).chain(1_000..).take(got.len()).collect();
    assert_eq!(got, expected, "a prefix of the commits");
}

/// Verifies: [EXTENSION] RULES.md D160, D166: a database crashed while a
/// backup copies its files recovers; its history shows the backup
/// INTERRUPTED, durably; the target holds no backup, as its manifest is
/// written last; a backup taken then restores. The backup let go after
/// fails, removing what it wrote, and is recorded FAILED.
#[test]
fn a_crash_during_a_backup_leaves_no_backup() {
    let scratch = Scratch::new("backup-crash");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = Arc::new(open(&scratch.path("db"), &clock));
    base(&db, 2_000);
    let (paused, go) = pausing(&db, &scratch.path("target"), 3);
    let taking = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || db.backup(&BackupOptions { location: "pausing:t".into(), ..BackupOptions::default() }))
    };
    paused.recv().unwrap();
    copy_dir(&scratch.path("db"), &scratch.path("crashed"));
    let after = Database::new();
    after.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    after.open_files(&after_crash(), &scratch.path("crashed"), &OpenOptions::default()).unwrap();
    after.create_user("OWNER", b"x").unwrap();
    after.set_owner("OWNER").unwrap();
    let shown = "SELECT TRIM(STATE), STOP_POSITION, ENDED FROM INFORMATION_SCHEMA.BACKUPS";
    assert_eq!(rows(&after.session("OWNER"), shown).unwrap(), ["INTERRUPTED,NULL,NULL"]);
    assert_eq!(contents(&after).len(), 2_000);
    let target = dir_target(&scratch.path("target2"));
    after.backup(&BackupOptions { location: target.clone(), ..BackupOptions::default() }).unwrap();
    go.send(false).unwrap();
    assert_eq!(taking.join().unwrap().unwrap_err().code, Code::Io);
    assert_eq!(DirectoryTarget::new("t", &scratch.path("target")).unwrap().list("").unwrap(), Vec::<String>::new(), "removed");
    let states: Vec<String> = db.backup_history().unwrap().iter().map(|e| format!("{e:?}")).collect();
    assert!(states.last().unwrap().contains("Failed"), "{states:?}");
    let back = restored(&options(&scratch.path("back"), &target), &clock);
    assert_eq!(contents(&back).len(), 2_000);
}

/// Verifies: [EXTENSION] RULES.md D160, D166: BACKUP DATABASE is the
/// owner's alone, outside a transaction; cancelled, it fails with SQLCODE
/// -920, removes what it wrote and is recorded CANCELLED; and a
/// non-owner sees no rows of BACKUPS, ARCHIVED_SEGMENTS or RESTORE_POINTS.
#[test]
fn a_backup_statement_is_the_owner_s_and_is_cancelled() {
    let scratch = Scratch::new("backup-cancel");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = Arc::new(open(&scratch.path("db"), &clock));
    base(&db, 2_000);
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let statement = "BACKUP DATABASE TO 'pausing:t' LABEL 'cancelled'";
    assert_eq!(exec(&db.session("APP"), statement).unwrap_err(), Code::Privilege);
    let owner = db.session("OWNER");
    let (paused, go) = pausing(&db, &scratch.path("target"), 3);
    let cancel = owner.canceller();
    let taking = std::thread::spawn(move || exec(&owner, statement));
    paused.recv().unwrap();
    cancel.cancel();
    go.send(true).unwrap();
    assert_eq!(taking.join().unwrap().unwrap_err(), Code::Cancelled);
    let owner = db.session("OWNER");
    let shown = "SELECT TRIM(STATE), TRIM(LABEL) FROM INFORMATION_SCHEMA.BACKUPS";
    assert_eq!(rows(&owner, shown).unwrap(), ["CANCELLED,cancelled"]);
    assert_eq!(DirectoryTarget::new("t", &scratch.path("target")).unwrap().list("").unwrap(), Vec::<String>::new());
    exec(&db.session("OWNER"), "CREATE RESTORE POINT BEFORE_LOAD").unwrap();
    assert_eq!(exec(&db.session("OWNER"), "CREATE RESTORE POINT BEFORE_LOAD").unwrap_err(), Code::Api);
    assert_eq!(rows(&owner, "SELECT TRIM(POINT_NAME) FROM INFORMATION_SCHEMA.RESTORE_POINTS").unwrap(), ["BEFORE_LOAD"]);
    let other = db.session("APP");
    for view in ["BACKUPS", "ARCHIVED_SEGMENTS", "RESTORE_POINTS"] {
        assert_eq!(rows(&other, &format!("SELECT * FROM INFORMATION_SCHEMA.{view}")).unwrap(), Vec::<String>::new(), "{view}");
    }
    assert_eq!(exec(&db.session("APP"), "CREATE RESTORE POINT P").unwrap_err(), Code::Privilege);
    db.schema("CREATE SCHEMA AUTHORIZATION OWNER CREATE TABLE O (K INTEGER)").unwrap();
    exec(&owner, "INSERT INTO O VALUES (1)").unwrap();
    assert_eq!(exec(&owner, "RESCAN BACKUP TARGET 'memory:x'").unwrap_err(), Code::TransactionState, "in a transaction");
}

/// Verifies: [EXTENSION] RULES.md D160, D166: two backups run at once, to
/// two targets, each completes and verifies; the history, durable, shows
/// both after the database is reopened; a backup's objects removed from
/// its target, RESCAN BACKUP TARGET marks it GONE; and another database
/// rescanning the target finds the backup it still holds.
#[test]
fn backups_run_at_once_and_their_history_is_found_again() {
    let scratch = Scratch::new("backup-history");
    let clock = Arc::new(FixedClock::at(1_000));
    let dir = scratch.path("db");
    let db = Arc::new(open(&dir, &clock));
    base(&db, 500);
    let (a, b) = (dir_target(&scratch.path("a")), dir_target(&scratch.path("b")));
    let threads: Vec<_> = [a.clone(), b.clone()]
        .into_iter()
        .map(|location| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || db.backup(&BackupOptions { location, ..BackupOptions::default() }).unwrap())
        })
        .collect();
    let ids: Vec<String> = threads.into_iter().map(|t| t.join().unwrap().id).collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(db.verify_backup(&a, "").unwrap().backups, 1);
    assert_eq!(db.verify_backup(&b, "").unwrap().backups, 1);
    db.close().unwrap();
    drop(db);
    let db = open(&dir, &clock);
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let shown = "SELECT TRIM(KIND), TRIM(STATE) FROM INFORMATION_SCHEMA.BACKUPS";
    assert_eq!(rows(&db.session("OWNER"), shown).unwrap(), ["FULL,COMPLETED", "FULL,COMPLETED"]);
    std::fs::remove_dir_all(scratch.path("b").join("backups")).unwrap();
    exec(&db.session("OWNER"), &format!("RESCAN BACKUP TARGET '{b}'")).unwrap();
    let states = "SELECT TRIM(STATE) FROM INFORMATION_SCHEMA.BACKUPS ORDER BY 1";
    assert_eq!(rows(&db.session("OWNER"), states).unwrap(), ["COMPLETED", "GONE"]);
    let other = open(&scratch.path("other"), &clock);
    let found = other.rescan_target(&a).unwrap();
    assert_eq!((found.backups, found.gone), (1, 0));
    assert_eq!(other.rescan_target(&a).unwrap().backups, 0, "found once");
}
