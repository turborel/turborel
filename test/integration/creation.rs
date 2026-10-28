//! A database's settings fixed at creation (RULES.md D36): its files'
//! page size, checksum, encodings and log segment size, given before it is
//! created, recorded in it, so it opens again with no options; and its
//! buffer pool, given as it opens.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::hash::crc::{CRC32_ISCSI, Crc, CrcParams};
use turborel::hash::{HashFunction, HashId};
use turborel::value::Value;

use crate::common::{RunLength, Scramble, Scratch, after_crash, number, os, rows};

/// A CRC of its own name and id: a hash function a plug-in registers.
fn plugged() -> Box<dyn HashFunction> {
    Box::new(Crc::new(CrcParams { id: HashId(9001), name: "TEST-CRC", ..CRC32_ISCSI }).unwrap())
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// A database opened from `dir`, `prepare`d first.
fn open(dir: &Path, prepare: impl FnOnce(&Database)) -> turborel::error::Result<(Database, Opened)> {
    let db = Database::new();
    prepare(&db);
    db.open_files(&after_crash(), dir, &OpenOptions::default()).map(|opened| (db, opened))
}

/// Schema A with table T, holding 300 rows of text that repeats, written
/// and committed.
fn fill(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(200))").unwrap();
    let s = db.session("A");
    for k in 0..300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'plain text {k}')")).unwrap();
    }
    s.commit().unwrap();
}

/// What the files under `dir` hold, all together.
fn all_bytes(dir: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.extend(all_bytes(&path));
        } else {
            all.extend(std::fs::read(&path).unwrap());
        }
    }
    all
}

/// Verifies: [DESIGN] RULES.md D30, D36: settings given before a database
/// is created fix its files' formats, recorded in it: it opens again with
/// no options and keeps them, its log in segments of the size given, its
/// files checksummed with the hash function given, which must be
/// registered to read them.
#[test]
fn a_database_keeps_the_formats_it_was_created_with() {
    let scratch = Scratch::new("creation-formats");
    let dir = scratch.path("db");
    let (db, opened) = open(&dir, |db| {
        db.register_hash_function(plugged()).unwrap();
        db.set_setting("STORAGE.PAGE_SIZE", &number(16384)).unwrap();
        db.set_setting("STORAGE.CHECKSUM", &text("test-crc")).unwrap();
        db.set_setting("LOG.SEGMENT_SIZE", &number(8192)).unwrap();
    })
    .unwrap();
    assert_eq!(opened, Opened::Created);
    fill(&db);
    let segments = std::fs::read_dir(dir.join("log")).unwrap().count();
    assert!(segments > 2, "segments of 8 KiB: {segments}");
    db.close().unwrap();
    drop(db);

    assert_eq!(open(&dir, |_| {}).err().map(|e| e.code), Some(Code::Api), "TEST-CRC is not registered");
    let (db, opened) = open(&dir, |db| db.register_hash_function(plugged()).unwrap()).unwrap();
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(rows(&db.session("A"), "SELECT COUNT(*) FROM T"), Ok(vec!["300".into()]));
    let kept = |name: &str| db.setting(name).unwrap().to_string();
    assert_eq!(
        (kept("STORAGE.PAGE_SIZE"), kept("STORAGE.CHECKSUM"), kept("LOG.SEGMENT_SIZE")),
        ("16384".into(), "TEST-CRC".into(), "8192".into())
    );
    let shown = rows(
        &db.session("A"),
        "SELECT TRIM(SETTING_VALUE), TRIM(VALUE_SOURCE), TRIM(TAKES_EFFECT) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STORAGE.CHECKSUM'",
    );
    assert_eq!(shown, Ok(vec!["TEST-CRC,DATABASE,CREATION".into()]));
}

/// Verifies: [DESIGN] RULES.md D30, D36: a database created with
/// encodings writes every file through them, so its rows' text is in none
/// of its files as written; it is read only with the encodings
/// registered, and recovered through them after a crash.
#[test]
fn a_database_created_with_encodings_writes_through_them() {
    let scratch = Scratch::new("creation-encodings");
    let dir = scratch.path("db");
    let registered = |db: &Database| {
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
    };
    let (crashed, _) = open(&dir, |db| {
        registered(db);
        db.set_setting("STORAGE.ENCODINGS", &text(" TEST RUN-LENGTH , test scramble")).unwrap();
    })
    .unwrap();
    assert_eq!(crashed.setting("STORAGE.ENCODINGS").unwrap().to_string(), "test run-length,test scramble");
    fill(&crashed);
    crashed.checkpoint().unwrap();
    let written = all_bytes(&dir);
    assert!(!written.windows(10).any(|w| w == b"plain text"), "no file holds the text as written");

    assert_eq!(open(&dir, |_| {}).err().map(|e| e.code), Some(Code::Api), "the encodings are not registered");
    let (db, opened) = open(&dir, registered).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("A"), "SELECT S FROM T WHERE K = 7"), Ok(vec![format!("{:200}", "plain text 7")]));
    drop((db, crashed));
}

/// Verifies: [DESIGN] RULES.md D36: a setting fixed at creation is refused
/// once the database is created, by the host program and in SQL; given
/// again as it opens, it must be the one it was created with; a hash
/// function or encoding not registered is refused, and so is one
/// registered twice.
#[test]
fn settings_fixed_at_creation_stay_fixed() {
    let scratch = Scratch::new("creation-fixed");
    let dir = scratch.path("db");
    let (db, _) = open(&dir, |db| db.set_setting("STORAGE.PAGE_SIZE", &number(4096)).unwrap()).unwrap();
    assert_eq!(db.set_setting("STORAGE.PAGE_SIZE", &number(8192)).err().map(|e| e.code), Some(Code::Api));
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let refused = db.session("OWNER").run("SET DATABASE LOG.SEGMENT_SIZE = 8192").err().map(|e| e.code);
    assert_eq!(refused, Some(Code::Api), "in SQL too");
    db.close().unwrap();
    drop(db);
    let given = |size: i128| open(&dir, |db| db.set_setting("STORAGE.PAGE_SIZE", &number(size)).unwrap());
    assert_eq!(given(8192).err().map(|e| e.code), Some(Code::Api), "created with 4096");
    let (db, opened) = given(4096).unwrap();
    assert_eq!(opened, Opened::Reopened);
    drop(db);

    let fresh = Database::new();
    assert_eq!(fresh.set_setting("STORAGE.CHECKSUM", &text("NOPE")).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(fresh.set_setting("STORAGE.ENCODINGS", &text("NOPE")).err().map(|e| e.code), Some(Code::Api));
    fresh.set_setting("STORAGE.CHECKSUM", &text("crc-64/nvme")).unwrap();
    assert_eq!(fresh.setting("STORAGE.CHECKSUM").unwrap().to_string(), "CRC-64/NVME", "as registered");
    fresh.register_encoding(Box::new(RunLength)).unwrap();
    assert_eq!(fresh.register_encoding(Box::new(RunLength)).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(fresh.register_hash_function(Box::new(Crc::new(CRC32_ISCSI).unwrap())).err().map(|e| e.code), Some(Code::Api));
    let scratch_bad = Scratch::new("creation-bad-page");
    let bad = open(&scratch_bad.path("db"), |db| db.set_setting("STORAGE.PAGE_SIZE", &number(5000)).unwrap());
    assert_eq!(bad.err().map(|e| e.code), Some(Code::Api), "not a power of two");
}

/// Verifies: [DESIGN] RULES.md D36, D38: the buffer pool is STORAGE.POOL_BYTES
/// given as the database opens, kept for the next; set while it is open,
/// it holds from the next.
#[test]
fn the_pool_is_given_as_the_database_opens() {
    let scratch = Scratch::new("creation-pool");
    let dir = scratch.path("db");
    let pooled = |bytes: Option<i128>| {
        let db = Database::new();
        if let Some(n) = bytes {
            db.set_setting("STORAGE.POOL_BYTES", &number(n)).unwrap();
        }
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = pooled(Some(16 * 8192));
    assert_eq!(db.memory().unwrap().1, 16 * 8192);
    db.close().unwrap();
    drop(db);
    let db = pooled(None);
    assert_eq!(db.memory().unwrap().1, 16 * 8192, "kept");
    db.set_setting("STORAGE.POOL_BYTES", &number(24 * 8192)).unwrap();
    assert_eq!(db.memory().unwrap().1, 16 * 8192, "until it opens again");
    db.close().unwrap();
    drop(db);
    assert_eq!(pooled(None).memory().unwrap().1, 24 * 8192);
}
