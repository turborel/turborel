//! The archive of the log (RULES.md D161, D163, D165). While ARCHIVE.MODE
//! is ON a checkpoint keeps every record not yet archived, and old
//! segments the newest backup does not need are removed. A restore begins
//! a new timeline, whose records never write over the old one's, and a
//! restore follows either. Backups are compressed as BACKUP.COMPRESSION
//! says.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::backup::{BackupOptions, RestoreOptions};
use turborel::db::{Database, OpenOptions};
use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::Value;

use crate::backups::{base, contents, dir_target, open, options, restored};
use crate::common::{RunLength, Scratch, number, os, rows};

/// Turns archiving on in `db`, to `archive`, its jobs run only when asked.
fn archiving(db: &Database, archive: &str) {
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("ARCHIVE.TARGET", &Value::Char(archive.to_owned())).unwrap();
    db.set_setting("ARCHIVE.MODE", &Value::Char("ON".into())).unwrap();
}

/// Inserts row `k` into APP.T and commits.
fn commit_row(db: &Database, k: i64) {
    let s = db.session("APP");
    s.run(&format!("INSERT INTO T VALUES ({k}, 'r{k}')")).unwrap();
    s.run("COMMIT").unwrap();
}

/// The segment files of the log of the database in `dir`.
fn segments(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("log")).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_none_or(|x| x != "records")).count()
}

/// Verifies: [EXTENSION] RULES.md D163: with ARCHIVE.MODE ON and nothing yet
/// archived, checkpoints keep the log's every segment, so the archive
/// begins at its first record; archived, the next checkpoint lets them
/// go. With it OFF, a checkpoint does.
#[test]
fn a_checkpoint_keeps_what_the_archive_lacks() {
    let scratch = Scratch::new("archive-floor");
    let clock = Arc::new(FixedClock::at(1_000));
    let made = |dir: &std::path::Path| {
        let db = Database::new();
        db.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
        db.set_setting("LOG.SEGMENT_BYTES", &number(4096)).unwrap();
        db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
        db
    };
    let (kept, free) = (made(&scratch.path("kept")), made(&scratch.path("free")));
    archiving(&kept, &dir_target(&scratch.path("archive")));
    for db in [&kept, &free] {
        base(db, 200);
        db.checkpoint().unwrap();
        db.checkpoint().unwrap();
    }
    let before = segments(&scratch.path("kept"));
    assert!(before > segments(&scratch.path("free")) + 2, "{before} kept");
    kept.archive_now().unwrap();
    kept.create_user("OWNER", b"x").unwrap();
    kept.set_owner("OWNER").unwrap();
    let first = "SELECT MIN(FIRST_POSITION) FROM INFORMATION_SCHEMA.ARCHIVED_SEGMENTS";
    assert_eq!(rows(&kept.session("OWNER"), first).unwrap(), ["256"], "from the first record");
    kept.checkpoint().unwrap();
    assert!(segments(&scratch.path("kept")) < before, "archived, let go");
}

/// Verifies: [EXTENSION] RULES.md D163: a segment older than
/// ARCHIVE.RETENTION_SECONDS whose records the newest backup does not need
/// is removed, and the history records it; an old one the backup needs
/// is kept, so a restore from that backup through the archive still
/// reaches its end.
#[test]
fn old_segments_are_removed() {
    let scratch = Scratch::new("archive-retention");
    let clock = Arc::new(FixedClock::at(1_000_000));
    let db = open(&scratch.path("db"), &clock);
    let archive = dir_target(&scratch.path("archive"));
    archiving(&db, &archive);
    db.set_setting("ARCHIVE.RETENTION_SECONDS", &number(10)).unwrap();
    base(&db, 5);
    db.archive_now().unwrap();
    clock.set(20_000_000);
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    commit_row(&db, 50);
    db.archive_now().unwrap();
    clock.set(40_000_000);
    commit_row(&db, 60);
    db.archive_now().unwrap();
    let removed = db.backup_history().unwrap().iter().filter(|e| format!("{e:?}").starts_with("Removed")).count();
    assert_eq!(removed, 1, "the first, and not the old one after the backup");
    let opts = RestoreOptions { archive: Some(archive), ..options(&scratch.path("back"), &location) };
    assert_eq!(contents(&restored(&opts, &clock)).len(), 7);
}

/// Verifies: [EXTENSION] RULES.md D165: a restore begins a timeline past
/// every one of the archive, its history written there; the restored
/// database archives under it, never over the old timeline, which its
/// first database goes on writing; a restore along either timeline holds
/// that timeline's commits, and not the other's.
#[test]
fn a_restore_begins_a_timeline() {
    let scratch = Scratch::new("archive-timelines");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    let archive = dir_target(&scratch.path("archive"));
    archiving(&db, &archive);
    base(&db, 5);
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    commit_row(&db, 10);
    db.archive_now().unwrap();
    let to_end =
        |dir: &str, timeline| RestoreOptions { archive: Some(archive.clone()), timeline, ..options(&scratch.path(dir), &location) };
    let report = Database::new().restore(&to_end("r1", None)).unwrap();
    assert_eq!(report.timeline, 2);
    assert!(scratch.path("archive").join("archive").join("00000002.history").exists());
    let r1 = open(&scratch.path("r1"), &clock);
    assert_eq!(r1.setting("ARCHIVE.TIMELINE").unwrap().to_string(), "2");
    r1.set_job_policy(JobPolicy::Manual).unwrap();
    commit_row(&r1, 20);
    r1.archive_now().unwrap();
    commit_row(&db, 30);
    db.archive_now().unwrap();
    assert!(scratch.path("archive").join("archive").join("00000002").exists());
    let keys = |db: &Database| contents(db).iter().map(|r| r.split(',').next().unwrap().to_owned()).collect::<Vec<_>>();
    let second = restored(&to_end("r2", Some(2)), &clock);
    assert_eq!(keys(&second), ["0", "1", "2", "3", "4", "10", "20"]);
    let first = restored(&to_end("r3", None), &clock);
    assert_eq!(keys(&first), ["0", "1", "2", "3", "4", "10", "30"]);
    assert_eq!(first.setting("ARCHIVE.TIMELINE").unwrap().to_string(), "4", "past the 3 the restore before began");
}

/// A run-length encoding standing as LZ4, of its id.
struct AsLz4;

impl Encoding for AsLz4 {
    fn id(&self) -> EncodingId {
        EncodingId(1)
    }
    fn name(&self) -> &'static str {
        "LZ4"
    }
    fn overhead(&self, context: &str) -> usize {
        RunLength.overhead(context)
    }
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        RunLength.encode(context, input, out)
    }
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> turborel::error::Result<()> {
        RunLength.decode(context, input, out)
    }
}

/// Verifies: [EXTENSION] RULES.md D161: with BACKUP.COMPRESSION ON a
/// backup's objects pass through LZ4 first, smaller than without, and
/// restore; with LZ4 not registered, a backup is refused, saying so.
#[test]
fn backups_are_compressed_as_asked() {
    let scratch = Scratch::new("backup-compressed");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    base(&db, 1_000);
    db.set_setting("BACKUP.COMPRESSION", &Value::Char("ON".into())).unwrap();
    let e = db.backup(&BackupOptions { location: dir_target(&scratch.path("none")), ..BackupOptions::default() }).unwrap_err();
    assert!(e.code == Code::Api && e.msg.contains("LZ4"), "{e}");
    db.register_encoding(Box::new(AsLz4)).unwrap();
    let location = dir_target(&scratch.path("small"));
    let small = db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    db.set_setting("BACKUP.COMPRESSION", &Value::Char("OFF".into())).unwrap();
    let plain = db.backup(&BackupOptions { location: dir_target(&scratch.path("plain")), ..BackupOptions::default() }).unwrap();
    assert!(small.bytes * 2 < plain.bytes, "{} against {}", small.bytes, plain.bytes);
    let restorer = Database::new();
    restorer.register_encoding(Box::new(AsLz4)).unwrap();
    restorer.restore(&options(&scratch.path("back"), &location)).unwrap();
    let back = Database::new();
    back.register_encoding(Box::new(AsLz4)).unwrap();
    back.open_files(&os(), &scratch.path("back"), &OpenOptions::default()).unwrap();
    assert_eq!(contents(&back).len(), 1_000);
}
