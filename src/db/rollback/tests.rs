//! A transaction rolled back beside checkpoints, through a crash (RULES.md
//! D25, D470, D1250).

use std::path::Path;
use std::sync::Arc;

use crate::db::image::Image;
use crate::db::{Database, OpenOptions, Opened};
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::value::{Decimal, Value};

/// The database in `dir` of `fs`, its log in segments of 16 KiB, no
/// checkpoint taken but those asked for.
fn open(fs: &Arc<dyn FileSystem>, dir: &Path) -> (Database, Opened) {
    let number = |n: i64| Value::Exact(Decimal::new(i128::from(n), 0));
    let db = Database::new();
    db.set_setting("LOG.SEGMENT_BYTES", &number(16 * 1024)).unwrap();
    let opened = db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// The first column of the one row `sql` reads in `db`.
fn one(db: &Database, sql: &str) -> String {
    let row = db.session("APP").open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row.first().map(ToString::to_string).unwrap_or_default()
}

/// Whether the image `db`'s last checkpoint recorded has recovery read the
/// log from the position it holds every change before, so from no record
/// of a transaction open then.
fn starts_at_its_end(db: &Database) -> bool {
    let files = db.files().unwrap().unwrap();
    let image = Image::decode(&files.catalog.last().unwrap(), &db.snapshot().unwrap().types).unwrap();
    image.start == image.through
}

/// Verifies: [X3.135-1986 §8.2, §8.9]; [DESIGN] RULES.md D25, D470, D1250,
/// a transaction that updated a row and rolled back, kept after as its
/// session keeps it until the rollback's call ends, a checkpoint taken
/// then, another transaction updating the row and committing, a second
/// checkpoint, a crash: each checkpoint's image has recovery read the log
/// from its own position, no record of the transaction rolled back
/// holding it, so the second leaves the segments before it go; and
/// recovery undoes nothing and reads the row as the second transaction
/// committed it, by its key and through its index. Regression test
/// (R-TEST-7): the first checkpoint wrote the records the transaction
/// still held, with no abort record after, the log was kept from them,
/// and recovery put the row back as they found it, over the commit.
#[test]
fn a_row_rolled_back_keeps_what_was_committed_after_through_a_crash() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let dir = Path::new("/rolled-back");
    let (crashed, _) = open(&fs, dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(8), F CHAR(200))").unwrap();
    crashed.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = crashed.session("APP");
    s.run("INSERT INTO T VALUES (1, 'base', 'f')").unwrap();
    s.commit().unwrap();
    let rolled = crashed.session("APP");
    rolled.run("UPDATE T SET V = 'rolled' WHERE K = 1").unwrap();
    let kept = rolled.current_transaction().unwrap();
    rolled.rollback().unwrap();
    crashed.checkpoint().unwrap();
    assert!(starts_at_its_end(&crashed), "no record of the transaction rolled back holds the log");
    s.run("UPDATE T SET V = 'kept' WHERE K = 1").unwrap();
    // Several segments of the log.
    for k in 2_u32..400_u32 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'more', 'f')")).unwrap();
    }
    s.commit().unwrap();
    crashed.checkpoint().unwrap();
    assert!(starts_at_its_end(&crashed));
    let segments = fs.read_dir(&dir.join("log")).unwrap().len();
    assert!(segments <= 2, "{segments} segments kept");
    drop((s, rolled, kept, crashed));
    let (db, opened) = open(&fs, dir);
    assert!(matches!(opened, Opened::Recovered(report) if report.undone == 0), "{opened:?}");
    assert_eq!(one(&db, "SELECT V FROM T WHERE K = 1"), "kept    ");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T WHERE V = 'kept'"), "1");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T WHERE V = 'base' OR V = 'rolled'"), "0");
}
