//! The fence of a database's files (RULES.md D56): a database whose
//! directory's lock another opener took, as a crashed process's is taken,
//! changes its files no more, however its sessions end.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use turborel::db::{Database, OpenOptions};

use crate::common::{Scratch, after_crash, rows};

/// The bytes of every file under `dir` but its lock, by path.
fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut all = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|n| n != "lock") {
                all.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    all
}

/// Verifies: [DESIGN] RULES.md D56: a session of a database dropped, its
/// files opened again twice since, ends with a change of its own: it rolls
/// back in memory, so no byte of the files changes, and a later open finds
/// the log whole and what the second opener committed, not its change.
#[test]
fn a_stale_session_ends_without_touching_the_files() {
    let scratch = Scratch::new("fenced-session");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let stale = db.session("A");
    stale.run("INSERT INTO T VALUES (1)").unwrap();
    stale.commit().unwrap();
    stale.run("INSERT INTO T VALUES (2)").unwrap();
    drop(db);
    let second = open();
    let s = second.session("A");
    s.run("INSERT INTO T VALUES (3)").unwrap();
    s.commit().unwrap();
    drop((s, second));
    drop(open());
    let before = contents(&dir);
    drop(stale);
    assert!(before == contents(&dir), "the stale session's end changed the files");
    let last = open();
    assert_eq!(rows(&last.session("A"), "SELECT K FROM T ORDER BY K").unwrap(), ["1", "3"]);
}

/// Verifies: [DESIGN] RULES.md D56: a database closed refuses a change of
/// its files through a session left; the session's transaction ends.
#[test]
fn a_closed_database_changes_nothing() {
    let scratch = Scratch::new("fenced-closed");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("A");
    db.close().unwrap();
    let before = contents(&dir);
    assert!(s.run("INSERT INTO T VALUES (1)").and_then(|_| s.commit()).is_err(), "a closed database commits nothing");
    drop(s);
    assert!(before == contents(&dir), "the files changed after the database closed");
}
