//! The file manager (RULES.md D56): a database's files opened through it,
//! no more open at once than FILES.MAX_OPEN, the one used least recently
//! closed and opened again as needed, each shown in
//! INFORMATION_SCHEMA.FILES to the database's owner.

use turborel::db::{Database, OpenOptions};
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, os, rows, small_segments};

/// Verifies: [DESIGN] RULES.md D56, D36: a database of more files than
/// FILES.MAX_OPEN, its log and its older rows' in many small segments,
/// works as any: what it writes is read back, through a checkpoint and a
/// close and an open again; no more than the limit are open at once; and
/// INFORMATION_SCHEMA.FILES shows its owner each file, of its kind, and
/// another user none.
#[test]
fn a_database_of_more_files_than_its_limit() {
    let scratch = Scratch::new("file-manager");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.set_setting("LOG.SEGMENT_SIZE", &Value::Exact(Decimal::new(4096, 0))).unwrap();
        db.set_setting("FILES.MAX_OPEN", &Value::Exact(Decimal::new(4, 0))).unwrap();
        db.open_files(&os(), &dir, &OpenOptions { undo: small_segments(), ..OpenOptions::default() }).unwrap();
        db
    };
    let db = open();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S CHARACTER(200))").unwrap();
    let a = db.session("A");
    for k in 0..400 {
        a.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
    }
    a.commit().unwrap();
    a.run("UPDATE T SET S = 'changed'").unwrap();
    let owner = db.session("OWNER");
    let shown = |sql: &str| rows(&owner, sql).unwrap();
    let files: usize = shown("SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES")[0].parse().unwrap();
    let open_now: usize = shown("SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES WHERE IS_OPEN = 'YES'")[0].parse().unwrap();
    assert!(files > 4 && open_now <= 4, "{files} files, {open_now} open");
    let kinds = shown("SELECT DISTINCT KIND FROM INFORMATION_SCHEMA.FILES ORDER BY KIND");
    assert!(["CATALOG", "DATA", "LOG", "UNDO"].iter().all(|k| kinds.contains(&(*k).to_owned())), "{kinds:?}");
    // As the Operator's Guide shows it.
    assert!(shown("SELECT KIND, COUNT(*) FROM INFORMATION_SCHEMA.FILES GROUP BY KIND").len() >= 4);
    assert_eq!(rows(&db.session("OTHER"), "SELECT COUNT(*) FROM INFORMATION_SCHEMA.FILES"), Ok(vec!["0".into()]));
    a.commit().unwrap();
    db.checkpoint().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE S = 'changed'"), Ok(vec!["400".into()]));
    a.commit().unwrap();
    owner.commit().unwrap();
    db.close().unwrap();
    drop((a, owner, db));
    let again = open();
    assert_eq!(rows(&again.session("A"), "SELECT COUNT(*) FROM T WHERE S = 'changed'"), Ok(vec!["400".into()]));
}

/// Verifies: [DESIGN] RULES.md D56: on Unix, the directories and files a
/// database makes are its user's alone: directories 0700, files 0600.
#[cfg(unix)]
#[test]
fn files_made_are_their_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("file-modes");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1)").unwrap();
    db.close().unwrap();
    let mut seen = 0_usize;
    let mut pending = vec![dir];
    while let Some(at) = pending.pop() {
        let meta = std::fs::metadata(&at).unwrap();
        let expected = if meta.is_dir() { 0o700 } else { 0o600 };
        assert_eq!(meta.permissions().mode() & 0o777, expected, "{}", at.display());
        if meta.is_dir() {
            pending.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
        }
        seen = seen.saturating_add(1);
    }
    assert!(seen > 3, "the directory, and its files: {seen}");
}
