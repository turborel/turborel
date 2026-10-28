//! A table's rows sealed through the encodings its option ENCODINGS names
//! (RULES.md D30, D41, D48): its records and its values out of line, and
//! its log records, read back through the encodings each was written with,
//! whatever the option says by then; names not registered refused; a
//! database opened without them still opens, its table's rows refused until
//! they are registered. An encrypted table is indexed only where the
//! database's files are encrypted too.

use std::path::Path;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{RunLength, Scramble, Scratch, exec, os, rows};

/// The bytes of the files in `dir`, and below it, whose names begin with
/// `prefix`: `data` for the row store's file and its shadows, `log` for
/// the log's segments.
fn bytes_of(dir: &Path, prefix: &str) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let named = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix));
        if path.is_dir() {
            all.extend(bytes_of(&path, if named { "" } else { prefix }));
        } else if named {
            all.extend(std::fs::read(&path).unwrap());
        }
    }
    all
}

/// Whether `bytes` hold `text`.
fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes())
}

/// A database opened from `dir`, the test encodings registered if
/// `registered`.
fn open(dir: &Path, registered: bool) -> Database {
    let db = Database::new();
    if registered {
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
    }
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// The S of row K = `k`: short enough for the record if `k` is even, else
/// long enough to go out of line.
fn text(k: u32) -> String {
    if k.is_multiple_of(2) { format!("secret {k}") } else { format!("secret {k} ").repeat(300) }
}

/// Rows of T, of keys `keys`.
fn insert(db: &Database, keys: std::ops::Range<u32>) {
    let s = db.session("S");
    for k in keys {
        exec(&s, &format!("INSERT INTO T VALUES ({k}, '{}')", text(k))).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
}

/// The rows of T, by key.
fn all(db: &Database) -> Vec<String> {
    rows(&db.session("S"), "SELECT K, S FROM T ORDER BY K").unwrap()
}

/// The rows of T, if it holds the keys `keys` as `insert` wrote them.
fn written(keys: impl Iterator<Item = u32>) -> Vec<String> {
    keys.map(|k| format!("{k},{:4000}", text(k))).collect()
}

/// Verifies: [EXTENSION] RULES.md D30, D41: rows of a table whose option
/// ENCODINGS names an encoding are sealed through it, records and values
/// out of line alike, so the data file holds none of their text, as it
/// does for a table naming none; they read back, through a checkpoint and
/// a reopening; the option is listed as registered, in any case given.
#[test]
fn a_tables_rows_pass_through_its_encodings() {
    let scratch = Scratch::new("table-encodings");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, S CHARACTER(4000)) WITH (ENCODINGS = 'TEST SCRAMBLE')")
        .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION P CREATE TABLE PLAIN (S CHARACTER(20))").unwrap();
    let p = db.session("P");
    exec(&p, "INSERT INTO PLAIN VALUES ('in plain view')").unwrap();
    exec(&p, "COMMIT WORK").unwrap();
    insert(&db, 0..6);
    assert_eq!(all(&db), written(0..6));
    let listed = "SELECT OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE OPTION_NAME = 'ENCODINGS'";
    let shown: Vec<Vec<String>> =
        rows(&db.session("S"), listed).unwrap().iter().map(|r| r.split(',').map(|v| v.trim().to_owned()).collect()).collect();
    assert_eq!(shown, vec![vec!["test scramble".to_owned(), "TABLE".to_owned()]]);
    db.checkpoint().unwrap();
    let stored = bytes_of(&dir, "data");
    assert!(stored.windows(13).any(|w| w == b"in plain view"), "a table naming no encoding is stored as it is");
    assert!(!stored.windows(7).any(|w| w == b"secret "), "no row of T is stored as it is");
    db.close().unwrap();
    let again = open(&dir, true);
    assert_eq!(all(&again), written(0..6));
    again.close().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D30, D41: ALTER TABLE SET (ENCODINGS
/// = ...) changes in place: rows written after pass through the new
/// encodings, those before are read through their own, and updates and
/// deletes of either work; DEFAULT names none again.
#[test]
fn a_change_of_encodings_is_made_in_place() {
    let db = Database::new();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHARACTER(4000)) WITH (ENCODINGS = 'test scramble')",
    )
    .unwrap();
    insert(&db, 0..4);
    let s = db.session("S");
    exec(&s, "ALTER TABLE T SET (ENCODINGS = 'test run-length, test scramble')").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    insert(&db, 4..8);
    exec(&s, "ALTER TABLE T SET (ENCODINGS = DEFAULT)").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    insert(&db, 8..10);
    assert_eq!(all(&db), written(0..10));
    exec(&s, "UPDATE T SET S = 'short' WHERE K < 6").unwrap();
    exec(&s, "DELETE FROM T WHERE K = 9").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE S = 'short'"), Ok(vec!["6".to_owned()]));
    assert_eq!(all(&db).len(), 9);
}

/// Verifies: [EXTENSION] RULES.md D41: an encoding not registered is
/// refused in CREATE TABLE and ALTER TABLE (SQLCODE -103); a database
/// opened without the encodings its table names still opens, its rows
/// refused (SQLCODE -910), read and written; with them registered again,
/// it reads them.
#[test]
fn encodings_not_registered_are_refused() {
    let scratch = Scratch::new("table-encodings-missing");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    let made = db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE U (K INTEGER) WITH (ENCODINGS = 'zstd')");
    assert_eq!(made.err().map(|e| e.code), Some(Code::SyntaxRule));
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, S CHARACTER(4000)) WITH (ENCODINGS = 'test scramble')")
        .unwrap();
    insert(&db, 0..2);
    let s = db.session("S");
    assert_eq!(exec(&s, "ALTER TABLE T SET (ENCODINGS = 'test scramble,zstd')").err(), Some(Code::SyntaxRule));
    exec(&s, "ROLLBACK WORK").unwrap();
    db.close().unwrap();

    let without = open(&dir, false);
    let s = without.session("S");
    assert_eq!(rows(&s, "SELECT K FROM T"), Err(Code::Api));
    assert_eq!(exec(&s, "INSERT INTO T VALUES (9, 'x')").err(), Some(Code::Api));
    exec(&s, "ROLLBACK WORK").unwrap();
    without.close().unwrap();
    assert_eq!(all(&open(&dir, true)), written(0..2));
}

/// Verifies: [EXTENSION] RULES.md D25, D48: a table's log records are
/// sealed through its encodings, so the log holds none of an encrypted
/// table's text, as it does a plain table's; recovery reads them back
/// through the encodings after a crash.
#[test]
fn an_encrypted_tables_log_records_are_sealed() {
    let scratch = Scratch::new("table-encodings-log");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, S CHARACTER(4000)) WITH (ENCODINGS = 'test scramble')")
        .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION P CREATE TABLE PLAIN (S CHARACTER(20))").unwrap();
    let p = db.session("P");
    exec(&p, "INSERT INTO PLAIN VALUES ('in plain view')").unwrap();
    exec(&p, "COMMIT WORK").unwrap();
    insert(&db, 0..4);
    let logged = bytes_of(&dir, "log");
    assert!(holds(&logged, "in plain view"), "a plain table's rows are logged as they are");
    assert!(!holds(&logged, "secret "), "an encrypted table's are not");
    drop((p, db));
    let again = open(&dir, true);
    assert_eq!(all(&again), written(0..4), "recovered through them");
}

/// Verifies: [EXTENSION] RULES.md D48: an index keeps its keys in order, in
/// the clear, so where the database's files are not encrypted, an
/// encrypted table is refused one (SQLCODE -103): by CREATE INDEX, by a
/// UNIQUE constraint, and by ALTER TABLE encrypting a table with one. A
/// table merely compressed may have them; and where the files are
/// encrypted (STORAGE.ENCODINGS), an encrypted table may too.
#[test]
fn an_encrypted_table_is_indexed_only_in_encrypted_files() {
    let scratch = Scratch::new("table-encodings-indexes");
    let db = open(&scratch.path("plain"), true);
    let unique = "CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER NOT NULL UNIQUE) WITH (ENCODINGS = 'test scramble')";
    assert_eq!(db.schema(unique).err().map(|e| e.code), Some(Code::SyntaxRule));
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'test scramble')").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE T (K INTEGER NOT NULL UNIQUE) WITH (ENCODINGS = 'test run-length')").unwrap();
    assert_eq!(db.create_index("T_K", "S.T", &["K"], "btree").err().map(|e| e.code), Some(Code::SyntaxRule));
    let c = db.session("C");
    assert_eq!(exec(&c, "ALTER TABLE T SET (ENCODINGS = 'test scramble')").err(), Some(Code::SyntaxRule), "it has an index");
    exec(&c, "ROLLBACK WORK").unwrap();
    db.close().unwrap();

    let dir = scratch.path("encrypted");
    let db = Database::new();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    db.set_setting("STORAGE.ENCODINGS", &turborel::value::Value::Char("test scramble".into())).unwrap();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    db.schema(unique).unwrap();
    db.close().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D41, D48: a table of an engine that does
/// not seal its rows (`StorageEngine::seals`) may name no encodings
/// (SQLCODE -103), though it lists the option.
#[test]
fn an_engine_that_does_not_seal_takes_no_encodings() {
    let db = crate::common::locking_database();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.place_table("L.T", crate::common::LOCKING).unwrap();
    let made = db.schema("CREATE SCHEMA AUTHORIZATION L CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'test run-length')");
    assert_eq!(made.err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Fills table `table` of schema S with `n` rows, each `prefix` and its key.
fn filled_with(db: &Database, table: &str, (prefix, n): (&str, usize)) {
    let s = db.session("S");
    let rows: Vec<String> = (0..n).map(|k| format!("({k}, '{prefix} {k:05}')")).collect();
    for chunk in rows.chunks(200) {
        exec(&s, &format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
}

/// Verifies: [EXTENSION] RULES.md D38, D48: a spill area's files are
/// written as they are while nothing is encrypted, and through an
/// encryption, with no key named (an ephemeral key), once a table is: so
/// what spills, here a sort's runs, never reaches the disk in the clear.
#[test]
fn spills_are_encrypted_while_anything_is() {
    let scratch = Scratch::new("table-encodings-spills");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE P (K INTEGER, S CHARACTER(60))").unwrap();
    filled_with(&db, "P", ("plain", 6000));
    db.set_setting("WORK_MEMORY", &turborel::value::Value::Exact(turborel::value::Decimal::new(64 << 10, 0))).unwrap();
    let s = db.session("S");
    let spilled = |query: &str| {
        let mut c = s.open_cursor(query).unwrap();
        assert!(c.fetch().unwrap().is_some());
        bytes_of(&dir.join("tmp"), "")
    };
    assert!(holds(&spilled("SELECT S FROM P ORDER BY S"), "plain 0"), "nothing encrypted: spilled as it is");
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'test scramble')").unwrap();
    assert!(!holds(&spilled("SELECT S FROM P ORDER BY S"), "plain 0"), "a table encrypted: every spill is");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM P").unwrap(), ["6000".to_owned()]);
    let sorted = rows(&s, "SELECT S FROM P ORDER BY S").unwrap();
    assert_eq!((sorted.len(), sorted[0].trim_end()), (6000, "plain 00000"), "read back through it");
}
