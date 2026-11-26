//! Backups and their restores (RULES.md D160 to D166). A backup taken
//! while the database serves restores to the rows committed before it,
//! and an incremental chain restores to its newest. A backup is verified
//! without restoring it, and damage is found. A restore stops at a time,
//! a position or a restore point, along the archive.

use std::path::Path;
use std::sync::Arc;

use turborel::backup::BackupKind;
use turborel::backup::seed::RecoveryTarget;
use turborel::clock::FixedClock;
use turborel::db::backup::{BackupOptions, RestoreOptions};
use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, SqlState};

use crate::common::{Scratch, number, os, rows};

/// A database kept in `dir`, its clock `clock`.
pub(crate) fn open(dir: &Path, clock: &Arc<FixedClock>) -> Database {
    let db = Database::new();
    db.set_clock(Arc::clone(clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// The rows of APP.T, in order.
pub(crate) fn contents(db: &Database) -> Vec<String> {
    rows(&db.session("APP"), "SELECT K, V FROM T ORDER BY K").unwrap()
}

/// Schema APP with table T and `n` rows.
pub(crate) fn base(db: &Database, n: i64) {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V VARCHAR(200))").unwrap();
    let s = db.session("APP");
    for k in 0..n {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{k:0>120}')")).unwrap();
    }
}

/// The location of a directory target at `path`.
pub(crate) fn dir_target(path: &Path) -> String {
    format!("dir:{}", path.display())
}

/// Restores into `into` as `options` say, from a fresh database, then
/// opens what was restored.
pub(crate) fn restored(options: &RestoreOptions, clock: &Arc<FixedClock>) -> Database {
    let restorer = Database::new();
    restorer.set_clock(Arc::clone(clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    restorer.restore(options).unwrap();
    open(&options.dir, clock)
}

/// Restore options of the newest backup of `location` into `dir`.
pub(crate) fn options(dir: &Path, location: &str) -> RestoreOptions {
    RestoreOptions {
        dir: dir.to_path_buf(),
        location: location.to_owned(),
        backup: String::new(),
        archive: None,
        target: RecoveryTarget::End,
        timeline: None,
    }
}

/// Verifies: [EXTENSION] RULES.md D160, D162: a full backup restores to the
/// rows committed before it, and none after; the restored database
/// reopens as any; the backup's chain verifies; a staging directory a
/// crashed restore left is replaced.
#[test]
fn a_full_backup_restores_what_was_committed_before_it() {
    let scratch = Scratch::new("backup-full");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    base(&db, 20);
    let location = dir_target(&scratch.path("target"));
    let report = db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    assert_eq!(report.kind, BackupKind::Full);
    assert_eq!(report.written, report.chunks);
    db.session("APP").run("INSERT INTO T VALUES (99, 'after')").unwrap();
    let verified = db.verify_backup(&location, "").unwrap();
    assert_eq!(verified.backups, 1);
    std::fs::create_dir_all(scratch.path("back.restoring").join("left")).unwrap();
    let back = restored(&options(&scratch.path("back"), &location), &clock);
    assert!(!scratch.path("back.restoring").exists(), "a staging directory left by a crash is replaced");
    assert!(!scratch.path("back").join("left").exists(), "and nothing it held is kept");
    assert_eq!(contents(&back), (0..20).map(|k| format!("{k},v{k:0>120}")).collect::<Vec<_>>());
    back.close().unwrap();
    let again = open(&scratch.path("back"), &clock);
    assert_eq!(contents(&again).len(), 20);
}

/// Verifies: [EXTENSION] RULES.md D162: an incremental backup holds only
/// the chunks changed since its parent, and a chain of two restores to
/// the rows of its newest.
#[test]
fn an_incremental_chain_restores_its_newest() {
    let scratch = Scratch::new("backup-incremental");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    base(&db, 4000);
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    db.session("APP").run("UPDATE T SET V = 'changed' WHERE K = 7").unwrap();
    let inc = BackupOptions { location: location.clone(), incremental: Some(String::new()), label: "second".into() };
    let report = db.backup(&inc).unwrap();
    assert_eq!(report.kind, BackupKind::Incremental);
    assert!(report.written < report.chunks, "{report:?}");
    db.session("APP").run("DELETE FROM T").unwrap();
    assert_eq!(db.verify_backup(&location, &report.id).unwrap().backups, 2);
    let back = restored(&options(&scratch.path("back"), &location), &clock);
    let got = contents(&back);
    assert_eq!(got.len(), 4000);
    assert!(got.contains(&"7,changed".to_owned()));
}

/// Verifies: [EXTENSION] RULES.md D161: a byte of a backup's object
/// changed is found by verification and by a restore, naming the object,
/// with SQLSTATE TBB01, and the restore leaves no directory.
#[test]
fn a_damaged_backup_is_found() {
    let scratch = Scratch::new("backup-damaged");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    base(&db, 20);
    let target = scratch.path("target");
    let location = dir_target(&target);
    let report = db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let data = target.join("backups").join(&report.id).join("files").join("data");
    let mut bytes = std::fs::read(&data).unwrap();
    let middle = bytes.len() >> 1;
    bytes[middle] ^= 0x40;
    std::fs::write(&data, bytes).unwrap();
    let e = db.verify_backup(&location, "").unwrap_err();
    assert_eq!((e.code, e.sqlstate()), (Code::Corrupt, SqlState::BACKUP_DAMAGED), "{e}");
    assert!(e.msg.contains("files/data"), "{e}");
    let restorer = Database::new();
    let e = restorer.restore(&options(&scratch.path("back"), &location)).unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::BACKUP_DAMAGED, "{e}");
    assert!(!scratch.path("back").exists() && !scratch.path("back.restoring").exists());
}

/// Verifies: [EXTENSION] RULES.md D163, D164: with the log archived, a
/// restore stops at a time between two commits, at a restore point, at a
/// position, or at the archive's end; one not met is refused, SQLSTATE
/// TBB02.
#[test]
fn a_restore_stops_at_its_target() {
    let scratch = Scratch::new("backup-pitr");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    let archive = dir_target(&scratch.path("archive"));
    db.set_setting("ARCHIVE.TARGET", &turborel::value::Value::Char(archive.clone().into())).unwrap();
    db.set_setting("ARCHIVE.MODE", &turborel::value::Value::Char("ON".into())).unwrap();
    base(&db, 5);
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let s = db.session("APP");
    clock.set(2_000);
    s.run("INSERT INTO T VALUES (10, 'at2000')").unwrap();
    s.run("COMMIT").unwrap();
    clock.set(3_000);
    let mark = db.create_restore_point("MARK").unwrap();
    s.run("INSERT INTO T VALUES (11, 'at3000')").unwrap();
    s.run("COMMIT").unwrap();
    clock.set(4_000);
    s.run("INSERT INTO T VALUES (12, 'at4000')").unwrap();
    s.run("COMMIT").unwrap();
    db.archive_now().unwrap();
    let cases = [
        (RecoveryTarget::Time(2_500), 6),
        (RecoveryTarget::Point("MARK".into()), 6),
        (RecoveryTarget::Position(mark), 6),
        (RecoveryTarget::Time(3_500), 7),
        (RecoveryTarget::End, 8),
    ];
    for (n, (target, expect)) in cases.into_iter().enumerate() {
        let dir = scratch.path(&format!("back{n}"));
        let opts = RestoreOptions { archive: Some(archive.clone()), target, ..options(&dir, &location) };
        let back = restored(&opts, &clock);
        assert_eq!(contents(&back).len(), expect, "case {n}");
        back.close().unwrap();
    }
    let opts = RestoreOptions {
        archive: Some(archive),
        target: RecoveryTarget::Point("NONE".into()),
        ..options(&scratch.path("none"), &location)
    };
    let e = Database::new().restore(&opts).unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::RECOVERY_TARGET_NOT_REACHED, "{e}");
}

/// Verifies: [EXTENSION] RULES.md D162, R-TEST-8: a restore is refused by
/// a database with files open (pair 1, 3) or with log storage of its own
/// (2, 3), and runs from one of neither.
#[test]
fn a_restore_runs_from_a_database_of_nothing() {
    let scratch = Scratch::new("backup-refused");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    base(&db, 3);
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let into = options(&scratch.path("back"), &location);
    assert_eq!(db.restore(&into).err().map(|e| e.code), Some(Code::Api), "1");
    let logged = Database::new();
    logged.set_log_storage(Box::new(crate::contracts::HeldLogs::default())).unwrap();
    assert_eq!(logged.restore(&into).err().map(|e| e.code), Some(Code::Api), "2");
    assert!(!scratch.path("back").exists());
    assert_eq!(contents(&restored(&into, &clock)).len(), 3, "3");
}

/// Loads rows `from..to` into APP.G and commits, a batch of a group at a time.
fn load_groups(db: &Database, (from, to): (i64, i64)) {
    let s = db.session("APP");
    let p = s.prepare("INSERT INTO G (K, V) VALUES (?, ?)").unwrap();
    let given: Vec<Vec<turborel::value::Value>> = (from..to).map(|k| vec![number(k), number(k % 7)]).collect();
    for batch in given.chunks(4096) {
        s.execute_prepared_rows(&p, batch, turborel::params::Load::Going).unwrap();
    }
    s.execute_prepared_rows(&p, &[], turborel::params::Load::Ending).unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D160, D1146: a backup carries the groups
/// loads wrote to segments, those a checkpoint named as files and those
/// since as the log's records, and restores every row of them.
#[test]
fn a_backup_restores_the_groups_of_loads() {
    let scratch = Scratch::new("backup-groups");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP CREATE TABLE G (K INTEGER NOT NULL, V INTEGER) USING columnstore \
         WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'GROUPS')",
    )
    .unwrap();
    load_groups(&db, (0, 9_000));
    db.checkpoint().unwrap();
    load_groups(&db, (9_000, 17_192));
    let sums = "SELECT COUNT(*), SUM(K), SUM(V) FROM G";
    let held = rows(&db.session("APP"), sums).unwrap();
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let back = restored(&options(&scratch.path("back"), &location), &clock);
    assert_eq!(rows(&back.session("APP"), sums).unwrap(), held);
}
