//! Index entries a row's versions share (RULES.md D750): an update that
//! leaves every column an index's key reads as it was adds no entry to it,
//! the version it replaced keeping its entry for both; readers find the row
//! through it at every snapshot, collection takes back one entry a run of
//! one key, and an index build and a checkpoint take the same entries.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened, Session};

use crate::common::{Scratch, after_crash, number, rows, small_pool};

/// T with indexes on K and V, `count` rows: K its number, V 'v' and its
/// last digit, W zero.
fn define(db: &Database, count: i64) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL, V CHAR(10), W INTEGER)").unwrap();
    db.create_index("T_K", "APP.T", &["K"], "btree").unwrap();
    db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = db.session("APP");
    for k in 0..count {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{}', 0)", k % 10)).unwrap();
    }
    s.commit().unwrap();
    s
}

/// Each index's entries, of `names`.
fn entries(db: &Database, names: &[&str]) -> Vec<usize> {
    names.iter().map(|name| db.index_entries(name).unwrap()).collect()
}

/// Verifies: [DESIGN] RULES.md D750: an update of a column no index reads
/// adds no entry, though a reader holds the versions it replaced: each
/// index keeps one entry a row, and the reader still finds each row's
/// version it sees through them, as a later reader finds the new one; a
/// rolled back update leaves them as they were.
#[test]
fn an_update_of_no_key_column_adds_no_entry() {
    let db = Database::new();
    let s = define(&db, 10);
    let reader = db.session("APP");
    assert_eq!(rows(&reader, "SELECT W FROM T WHERE K = 3"), Ok(vec!["0".into()]), "a snapshot, held open");
    s.run("UPDATE T SET W = W + 1").unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V"]), [10, 10], "none added for the new versions");
    s.commit().unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V"]), [10, 10]);
    assert_eq!(rows(&reader, "SELECT W FROM T WHERE K = 3"), Ok(vec!["0".into()]), "its version through the entry");
    assert_eq!(rows(&reader, "SELECT K FROM T WHERE V = 'v4'"), Ok(vec!["4".into()]));
    reader.commit().unwrap();
    assert_eq!(rows(&reader, "SELECT W FROM T WHERE K = 3"), Ok(vec!["1".into()]), "the new version through it");
    s.run("UPDATE T SET W = 9 WHERE K = 3").unwrap();
    s.rollback().unwrap();
    reader.commit().unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V"]), [10, 10], "after collection too");
    assert_eq!(rows(&reader, "SELECT W FROM T WHERE V = 'v3'"), Ok(vec!["1".into()]));
}

/// Verifies: [DESIGN] RULES.md D750: updates of a row in one transaction,
/// of a column no index reads, again, then of V, leave T_K one entry and
/// T_V the old key's and the new one's while a reader holds the version
/// before them; once it ends, collection takes back the old key's entry
/// alone, and the row is found by its new key, not its old.
#[test]
fn a_run_of_one_key_keeps_one_entry_until_its_key_changes() {
    let db = Database::new();
    let s = define(&db, 10);
    let reader = db.session("APP");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T"), Ok(vec!["10".into()]), "a snapshot, held open");
    s.run("UPDATE T SET W = 1 WHERE K = 3").unwrap();
    s.run("UPDATE T SET W = 2 WHERE K = 3").unwrap();
    s.run("UPDATE T SET V = 'x' WHERE K = 3").unwrap();
    s.commit().unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V"]), [10, 11], "the old key's entry kept for the reader");
    assert_eq!(rows(&reader, "SELECT K, W FROM T WHERE V = 'v3'"), Ok(vec!["3,0".into()]));
    reader.commit().unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V"]), [10, 10], "one a row once collected");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T WHERE V = 'v3'"), Ok(vec!["0".into()]));
    assert_eq!(rows(&reader, "SELECT K, W FROM T WHERE V = 'x'"), Ok(vec!["3,2".into()]));
    assert_eq!(rows(&reader, "SELECT V FROM T WHERE K = 3"), Ok(vec!["x         ".into()]));
}

/// Verifies: [DESIGN] RULES.md D27, D750: an index built while a reader
/// holds versions an update replaced takes an entry of each version of a
/// key the update changed, and one of a key it kept, as collection takes
/// them back: once the reader ends, each holds one entry a row.
#[test]
fn an_index_built_after_a_shared_update_takes_the_entries_collection_does() {
    let db = Database::new();
    let s = define(&db, 10);
    let reader = db.session("APP");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T"), Ok(vec!["10".into()]), "a snapshot, held open");
    s.run("UPDATE T SET W = K + 100").unwrap();
    s.commit().unwrap();
    db.create_index("T_W", "APP.T", &["W"], "btree").unwrap();
    db.create_index("T_KV", "APP.T", &["K", "V"], "btree").unwrap();
    assert_eq!(entries(&db, &["T_W", "T_KV"]), [20, 10], "W's two versions, K and V's one");
    reader.commit().unwrap();
    assert_eq!(entries(&db, &["T_K", "T_V", "T_W", "T_KV"]), [10, 10, 10, 10], "one a row once collected");
    assert_eq!(rows(&reader, "SELECT K FROM T WHERE W = 104"), Ok(vec!["4".into()]));
}

/// The database in `dir`, with a small buffer pool, checkpointed only when
/// asked.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// Verifies: [DESIGN] RULES.md D25, D750: a checkpoint taken while a reader
/// holds versions whose entries the versions after share logs none of
/// them as stale, so recovery from it takes back none a live version needs:
/// each index holds one entry a row, and each row is found by its key.
#[test]
fn a_checkpoint_logs_no_shared_entry_as_stale() {
    let scratch = Scratch::new("shared-entries-checkpoint");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    let s = define(&crashed, 1_000);
    let reader = crashed.session("APP");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T"), Ok(vec!["1000".into()]), "a snapshot, held open");
    s.run("UPDATE T SET W = 1 WHERE K < 200").unwrap();
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'x' WHERE K < 100").unwrap();
    s.commit().unwrap();
    assert_eq!(entries(&crashed, &["T_K", "T_V"]), [1_000, 1_100], "V's old keys kept for the reader");
    crashed.checkpoint().unwrap();
    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(entries(&db, &["T_K", "T_V"]), [1_000, 1_000], "one a row");
    let after = db.session("APP");
    assert_eq!(rows(&after, "SELECT COUNT(*) FROM T WHERE V = 'v3'"), Ok(vec!["90".into()]));
    assert_eq!(rows(&after, "SELECT COUNT(*) FROM T WHERE W = 1 AND K BETWEEN 150 AND 159"), Ok(vec!["10".into()]));
    assert_eq!(rows(&after, "SELECT W FROM T WHERE K = 150"), Ok(vec!["1".into()]));
    drop((db, s, reader, crashed));
}
