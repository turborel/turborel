//! The log's buffer (RULES.md D25, D921): a commit of LOG.COMMIT_SYNC OFF
//! leaves its record in the log's memory, written with those of other
//! commits; a commit that waits writes them all with its own, in one write
//! and one sync; and what a failure leaves, of the process alone or of the
//! system, between the append and the write, between the write and the
//! sync, and after the sync, is a prefix of the commits, as the setting
//! promises.

use std::path::{Path, PathBuf};
use std::time::Duration;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::threads::JobPolicy;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, number, rows};
use crate::faults::{Faults, FaultyFs, Rule};
use crate::log_sync::last_segment;

/// The database in `dir`, made there through a faulty file system, its
/// background work left to the test, with APP's table T.
fn made(dir: &Path) -> (Database, std::sync::Arc<Faults>) {
    let (fs, faults) = FaultyFs::with_faults();
    let db = Database::new();
    db.open_files(&fs, dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER)").unwrap();
    (db, faults)
}

/// Inserts `id` into T in a transaction of its own, its commit waiting
/// for the log if `waits`.
fn commit(db: &Database, id: i32, waits: bool) {
    let s = db.session("APP");
    s.set_setting("LOG.COMMIT_SYNC", &Value::Char(if waits { "ON" } else { "OFF" }.into())).unwrap();
    s.run(&format!("INSERT INTO T (ID) VALUES ({id})")).unwrap();
    s.commit().unwrap();
}

/// Copies the files under `from` to `to`, as they are now: what a failure
/// of the process alone leaves, the system keeping what was written.
fn image(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target: PathBuf = to.join(path.file_name().unwrap());
        if path.is_dir() {
            image(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// The ids of T in the database in `dir`, opened after a failure and
/// recovered, in order.
fn recovered(dir: &Path) -> Vec<String> {
    let db = Database::new();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let ids = rows(&db.session("APP"), "SELECT ID FROM T ORDER BY ID").unwrap();
    db.close().unwrap();
    ids
}

/// `ids` as the rows a query gives.
fn ids(ids: &[i32]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D25, D921, a failure of the process alone:
/// between the append of a commit of LOG.COMMIT_SYNC OFF to the log's
/// buffer and the buffer's write, the files hold none of the commits left
/// there and every commit before them; once the buffer is written, here
/// by the log sync job under LOG.SYNC_AFTER_MS 0, which writes and syncs
/// nothing, they hold them all; a commit that waits is held from the
/// moment it returns, with every commit before it.
#[test]
fn a_failure_of_the_process_loses_only_the_commits_still_buffered() {
    let scratch = Scratch::new("log-buffer-process");
    let dir = scratch.path("db");
    let (db, _) = made(&dir);
    db.set_setting("LOG.SYNC_AFTER_MS", &number(0_i32)).unwrap();
    commit(&db, 1, true);
    commit(&db, 2, false);
    commit(&db, 3, false);
    image(&dir, &scratch.path("buffered"));
    assert_eq!(rows(&db.session("APP"), "SELECT ID FROM T ORDER BY ID").unwrap(), ids(&[1, 2, 3]), "seen at once");
    db.run_due().unwrap();
    image(&dir, &scratch.path("written"));
    commit(&db, 4, false);
    commit(&db, 5, true);
    image(&dir, &scratch.path("waited"));
    assert_eq!(recovered(&scratch.path("buffered")), ids(&[1]), "between the append and the write");
    assert_eq!(recovered(&scratch.path("written")), ids(&[1, 2, 3]), "between the write and a sync");
    assert_eq!(recovered(&scratch.path("waited")), ids(&[1, 2, 3, 4, 5]), "a commit that waits writes those before it");
    drop(db);
}

/// Verifies: [DESIGN] RULES.md D25, D921, a failure of the system (each
/// file cut to its last sync): commits of LOG.COMMIT_SYNC OFF are lost
/// whether still in the log's buffer or written and not synced, as before
/// the log had a buffer, every commit before them kept; once the log sync
/// job syncs, or a commit that waits, they are kept.
#[test]
fn a_failure_of_the_system_loses_the_commits_not_synced() {
    for (stage, expected) in [("buffered", vec![1]), ("written", vec![1]), ("synced", vec![1, 2, 3]), ("waited", vec![1, 2, 3, 4])] {
        let scratch = Scratch::new(&format!("log-buffer-system-{stage}"));
        let dir = scratch.path("db");
        let (db, faults) = made(&dir);
        if stage == "written" {
            db.set_setting("LOG.SYNC_AFTER_MS", &number(0_i32)).unwrap();
        }
        commit(&db, 1, true);
        commit(&db, 2, false);
        commit(&db, 3, false);
        match stage {
            "written" | "synced" => db.run_due().unwrap(),
            "waited" => commit(&db, 4, true),
            _ => {}
        }
        drop(db);
        faults.cut_to_synced(&last_segment(&dir));
        assert_eq!(recovered(&dir), ids(&expected), "{stage}");
    }
}

/// Verifies: [DESIGN] RULES.md D921, D25: a write of the log's buffer that
/// a failure tears, half of it reaching the file, leaves a prefix of the
/// commits: recovery cuts the torn end, keeps every commit before the
/// buffer's, and the database opened again takes changes.
#[test]
fn a_torn_write_of_the_buffer_leaves_a_prefix() {
    let scratch = Scratch::new("log-buffer-torn");
    let dir = scratch.path("db");
    let (db, faults) = made(&dir);
    db.set_setting("LOG.SYNC_AFTER_MS", &number(0_i32)).unwrap();
    commit(&db, 1, true);
    for id in 2..=9 {
        commit(&db, id, false);
    }
    faults.inject(Rule::tear().on("log/segment").times(1));
    assert!(db.run_due().is_err(), "the buffer's write, torn");
    image(&dir, &scratch.path("torn"));
    let kept = recovered(&scratch.path("torn"));
    assert!(kept.len() < 9 && kept == ids(&(1..=i32::try_from(kept.len()).unwrap()).collect::<Vec<_>>()), "a prefix: {kept:?}");
    assert_eq!(kept.first(), Some(&"1".to_owned()), "the commit that waited");
    let again = Database::new();
    again.open_files(&after_crash(), &scratch.path("torn"), &OpenOptions::default()).unwrap();
    commit(&again, 100, true);
    again.close().unwrap();
    drop(db);
}

/// Verifies: [DESIGN] RULES.md D921, D25: a commit that waits, alone,
/// still makes one write of the log and one sync of its segment; three
/// commits of LOG.COMMIT_SYNC OFF make no write, and the commit that
/// waits after them one write for the four and one sync.
#[test]
fn a_lone_commit_that_waits_makes_one_write_and_one_sync() {
    let scratch = Scratch::new("log-buffer-lone");
    let dir = scratch.path("db");
    let (db, faults) = made(&dir);
    commit(&db, 1, true);
    // Counted: a rule that delays each write of a segment by nothing.
    faults.inject(Rule::delay(Duration::ZERO).on("log/segment"));
    let (writes, _) = (faults.injected(), faults.take_syncs());
    commit(&db, 2, true);
    let segment_syncs = |faults: &Faults| faults.take_syncs().iter().filter(|(path, _)| path.to_string_lossy().contains("segment")).count();
    assert_eq!((faults.injected() - writes, segment_syncs(&faults)), (1, 1), "one write, one sync");
    let writes = faults.injected();
    for id in 3..=5 {
        commit(&db, id, false);
    }
    assert_eq!((faults.injected() - writes, segment_syncs(&faults)), (0, 0), "held in the buffer");
    commit(&db, 6, true);
    assert_eq!((faults.injected() - writes, segment_syncs(&faults)), (1, 1), "one write for the four, one sync");
    drop(db);
}
