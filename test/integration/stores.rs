//! Where a database kept in a directory keeps its parts (RULES.md D25):
//! its catalog, the definitions as they are, not every one made, where its
//! catalog storage keeps it; and its log, where its log storage does.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;

use crate::common::{LockingEngine, Scratch, after_crash, rows};

/// The database in `dir`, opened with a small buffer pool; `LockingEngine`
/// registered first.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    crate::common::small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// The bytes of the catalog files in `dir`.
fn catalog_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("catalog"))
        .map(|e| e.metadata().unwrap().len())
        .sum()
}

/// Verifies: [DESIGN] RULES.md D25, the catalog file holds the definitions
/// as they are, not every one made: a database that created and dropped an
/// index a hundred times, set a setting a hundred times and altered a
/// table, closed, keeps a catalog the size of one that did not, and opened
/// again has the definitions as they were: the altered table's rows, by its
/// storage's generation, the index kept, the setting's last value.
#[test]
fn the_catalog_holds_the_definitions_as_they_are() {
    let scratch = Scratch::new("reopen-compacted");
    let (plain, churned) = (scratch.path("plain"), scratch.path("churned"));
    for dir in [&plain, &churned] {
        let (db, _) = open(dir);
        db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
        db.session("S").run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
        db.session("S").commit().unwrap();
        db.close().unwrap();
    }
    let (db, _) = open(&churned);
    for n in 0..100 {
        db.create_index("T_V", "S.T", &["V"], "btree").unwrap();
        db.drop_index("T_V").unwrap();
        db.set_setting("WORK_MEMORY", &turborel::value::Value::Exact(turborel::value::Decimal::new((1 << 20) + n, 0))).unwrap();
    }
    db.create_index("T_V", "S.T", &["V"], "btree").unwrap();
    let s = db.session("S");
    s.run("ALTER TABLE T ADD COLUMN W INTEGER DEFAULT 7").unwrap();
    s.commit().unwrap();
    db.close().unwrap();
    let (db, opened) = open(&plain);
    db.close().unwrap();
    assert!(matches!(opened, Opened::Reopened));
    assert!(catalog_bytes(&churned) < 2 * catalog_bytes(&plain), "{} against {}", catalog_bytes(&churned), catalog_bytes(&plain));
    let (db, opened) = open(&churned);
    assert!(matches!(opened, Opened::Reopened));
    let s = db.session("S");
    assert_eq!(rows(&s, "SELECT K, V, W FROM T WHERE V = 20"), Ok(vec!["2,20,7".into()]));
    assert_eq!(db.index_entries("T_V").unwrap(), 2);
    let listed = rows(&s, "SELECT TRIM(SETTING_VALUE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'WORK_MEMORY'").unwrap();
    assert_eq!(listed, [((1 << 20) + 99).to_string()]);
}

/// Verifies: [EXTENSION] RULES.md D25: a database kept in a directory
/// keeps its catalog with the storage it is given, none in the directory:
/// closed, it is reopened in place from it, and stopped, recovered from it;
/// opened with another, which finds no catalog, it is refused, as its files
/// are there; and its storage is not changed once it is opened.
#[test]
fn a_catalog_is_kept_where_its_storage_keeps_it() {
    let scratch = Scratch::new("reopen-held-catalog");
    let dir = scratch.path("db");
    let held = crate::contracts::HeldCatalogs::default();
    let open = || {
        let db = Database::new();
        db.set_catalog_storage(Box::new(held.clone())).unwrap();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Created));
    assert_eq!(db.set_catalog_storage(Box::new(held.clone())).err().map(|e| e.code), Some(Code::Api), "once opened");
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    db.session("S").run("INSERT INTO T VALUES (1)").unwrap();
    db.session("S").commit().unwrap();
    db.close().unwrap();
    assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("catalog")));
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Reopened), "{opened:?}");
    db.session("S").run("INSERT INTO T VALUES (2)").unwrap();
    db.session("S").commit().unwrap();
    drop(db);
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("S"), "SELECT K FROM T"), Ok(vec!["1".into(), "2".into()]));
    drop(db);
    let refused = Database::new().open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap_err();
    assert_eq!(refused.code, Code::Api);
    assert!(refused.msg.contains("files, finds no catalog"), "{refused}");
}

/// Verifies: [EXTENSION] RULES.md D25: a database kept in a directory
/// keeps its log with the log storage it is given, and its catalog with the
/// catalog storage, neither in the directory: closed, it is reopened in
/// place from them, and stopped, recovered from its log; given its catalog's
/// storage but not its log's, it is refused, naming the cause; and its log's
/// storage is not changed once it is opened.
#[test]
fn a_log_is_kept_where_its_storage_keeps_it() {
    let scratch = Scratch::new("stores-held-log");
    let dir = scratch.path("db");
    let (logs, catalogs) = (crate::contracts::HeldLogs::default(), crate::contracts::HeldCatalogs::default());
    let open = |with_log: bool| {
        let db = Database::new();
        db.set_catalog_storage(Box::new(catalogs.clone())).unwrap();
        if with_log {
            db.set_log_storage(Box::new(logs.clone())).unwrap();
        }
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default());
        (db, opened)
    };
    let (db, opened) = open(true);
    assert!(matches!(opened, Ok(Opened::Created)));
    assert_eq!(db.set_log_storage(Box::new(logs.clone())).err().map(|e| e.code), Some(Code::Api), "once opened");
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    db.session("S").run("INSERT INTO T VALUES (1)").unwrap();
    db.session("S").commit().unwrap();
    db.close().unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n == "log" || n.starts_with("catalog")), "{names:?}");
    let (db, opened) = open(true);
    assert!(matches!(opened, Ok(Opened::Reopened)), "{opened:?}");
    db.session("S").run("INSERT INTO T VALUES (2)").unwrap();
    db.session("S").commit().unwrap();
    drop(db);
    let (db, opened) = open(true);
    assert!(matches!(opened, Ok(Opened::Recovered(_))), "{opened:?}");
    assert_eq!(rows(&db.session("S"), "SELECT K FROM T"), Ok(vec!["1".into(), "2".into()]));
    drop(db);
    let refused = open(false).1.unwrap_err();
    assert_eq!(refused.code, Code::Api);
    assert!(refused.msg.contains("finds no log"), "{refused}");
}
