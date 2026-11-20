//! The inverted index, `USING fulltext` (RULES.md D86), through SQL: made,
//! kept as rows change, rolled back, recovered, read by snapshots and by
//! temporary tables, and of its options; TEXT_MATCHES \[EXTENSION\] the
//! function it answers.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

use crate::common::{RunLength, Scramble, Scratch, after_crash, exec, os, rows};

mod options;

/// The least ID of the rows `documents` fills DOCS with.
pub const FILLER: u32 = 1000;

/// Schema S's table DOCS (ID INTEGER, BODY VARCHAR(400)), its rows those
/// of `texts`, ID from 1, and 2000 more from ID `FILLER` of words no test
/// looks for, so a lookup costs less than a scan; and its index DOCS_BODY
/// `USING fulltext`.
pub fn documents(db: &Database, texts: &[&str]) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(400))").unwrap();
    let s = db.session("S");
    for (id, text) in texts.iter().enumerate() {
        exec(&s, &format!("INSERT INTO DOCS VALUES ({}, '{text}')", id + 1)).unwrap();
    }
    let filler: Vec<String> = (FILLER..FILLER + 2000).map(|k| format!("({k}, 'filler row number {k}')")).collect();
    for chunk in filler.chunks(200) {
        exec(&s, &format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    s
}

/// The IDs below `FILLER` of DOCS whose BODY holds the words `words`, in
/// order.
pub fn found(s: &Session, words: &str) -> Vec<String> {
    let rows = rows(s, &format!("SELECT ID FROM DOCS WHERE TEXT_MATCHES(BODY, '{words}') ORDER BY ID")).unwrap();
    let ids = rows.into_iter().map(|r| r.trim().to_owned());
    ids.filter(|id| id.parse::<u32>().is_ok_and(|id| id < FILLER)).collect()
}

/// Whether the plan of a search for `words` reads DOCS_BODY.
pub fn probed(s: &Session, words: &str) -> bool {
    let sql = format!("SELECT ID FROM DOCS WHERE TEXT_MATCHES(BODY, '{words}')");
    s.physical_plan(&sql, &Params::new()).unwrap().contains("using DOCS_BODY")
}

/// Texts of a few words each, for the tests.
pub const TEXTS: [&str; 5] = [
    "The quick brown fox jumps over the lazy dog",
    "A lazy afternoon in the garden",
    "Foxes are quick and clever animals",
    "The garden gnome stood still",
    "Brown bears and brown foxes",
];

/// Verifies: [EXTENSION] RULES.md D34, D86: CREATE INDEX ... USING
/// fulltext indexes a character string column; a TEXT_MATCHES search of
/// it is a lookup of the index, which finds the rows holding every word
/// asked for, normalized alike, whatever their case and accents and
/// wherever they stand, and no other; an INSERT, UPDATE and DELETE keep
/// it so, the rows found always those a scan of the table finds.
#[test]
fn a_search_reads_the_index_and_follows_the_rows() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    assert!(probed(&s, "lazy"), "the index is read");
    assert_eq!(found(&s, "lazy"), ["1", "2"]);
    assert_eq!(found(&s, "LAZY Garden"), ["2"]);
    assert_eq!(found(&s, "brown foxes"), ["5"]);
    assert_eq!(found(&s, "quick"), ["1", "3"]);
    assert_eq!(found(&s, "zebra"), Vec::<String>::new());
    exec(&s, "INSERT INTO DOCS VALUES (6, 'A Café in the garden, quick and lazy')").unwrap();
    exec(&s, "UPDATE DOCS SET BODY = 'Nothing to see here' WHERE ID = 1").unwrap();
    exec(&s, "DELETE FROM DOCS WHERE ID = 2").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(found(&s, "lazy"), ["6"]);
    assert_eq!(found(&s, "cafe garden"), ["6"], "accents normalized");
    assert_eq!(found(&s, "nothing"), ["1"]);
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "DROP INDEX DOCS_BODY").unwrap();
    assert!(!probed(&s, "lazy"));
    assert_eq!(found(&s, "lazy"), ["6"], "a scan finds the same rows");
    assert_eq!(found(&s, "quick"), ["3", "6"]);
}

/// Verifies: [EXTENSION] RULES.md D86: TEXT_MATCHES is true of a document
/// holding every word that is not a stop word, of a search of stop words
/// alone too, null of a null document or search; a lookup of the index
/// gives the same, a search of stop words alone reading every document.
#[test]
fn text_matches_reads_stop_words_and_nulls_as_a_scan_does() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    exec(&s, "INSERT INTO DOCS VALUES (6, NULL)").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(found(&s, "the lazy"), ["1", "2"], "THE a stop word");
    assert_eq!(found(&s, "the and"), ["1", "2", "3", "4", "5"], "stop words alone: every document");
    let nulls = "SELECT COUNT(*) FROM DOCS WHERE TEXT_MATCHES(BODY, CAST(NULL AS VARCHAR(10))) IS NULL";
    assert_eq!(rows(&s, nulls).unwrap(), ["2006"], "a null search: null of every document");
    assert_eq!(rows(&s, "SELECT TEXT_MATCHES(BODY, 'x') IS NULL FROM DOCS WHERE ID = 6").unwrap(), ["TRUE"]);
}

/// Verifies: [EXTENSION] RULES.md D19, D86: the entries of a change are
/// taken back as the change is: by ROLLBACK, and by a statement that
/// fails, which leaves the statements before it found.
#[test]
fn rollback_takes_the_entries_back() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    exec(&s, "INSERT INTO DOCS VALUES (6, 'zebra crossing')").unwrap();
    exec(&s, "UPDATE DOCS SET BODY = 'zebra stripes' WHERE ID = 1").unwrap();
    assert_eq!(found(&s, "zebra"), ["1", "6"]);
    exec(&s, "ROLLBACK WORK").unwrap();
    assert_eq!(found(&s, "zebra"), Vec::<String>::new());
    assert_eq!(found(&s, "fox"), ["1"]);
    exec(&s, "INSERT INTO DOCS VALUES (7, 'zebra one')").unwrap();
    // Its second row's ID is taken, so the statement fails whole.
    assert_eq!(exec(&s, "INSERT INTO DOCS VALUES (8, 'zebra two'), (3, 'zebra three')").err(), Some(Code::Constraint));
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(found(&s, "zebra"), ["7"]);
    assert_eq!(found(&s, "clever"), ["3"]);
}

/// Verifies: [EXTENSION] RULES.md D29, D86: versions of a row each have
/// their entries, so a transaction reading a snapshot finds a row by the
/// words of the version it sees, not those another transaction committed
/// after; once it ends, the row is found by its new words alone.
#[test]
fn a_snapshot_finds_rows_by_the_words_it_sees() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    crate::common::serializable(&db);
    let reader = db.session("S");
    assert_eq!(found(&reader, "gnome"), ["4"]);
    exec(&s, "UPDATE DOCS SET BODY = 'A garden of stones' WHERE ID = 4").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    assert_eq!(found(&reader, "gnome"), ["4"], "the version it sees");
    assert_eq!(found(&reader, "stones"), Vec::<String>::new());
    exec(&reader, "COMMIT WORK").unwrap();
    assert_eq!(found(&reader, "gnome"), Vec::<String>::new());
    assert_eq!(found(&reader, "stones"), ["4"]);
}

/// The database in `dir`, recovering after a crash.
fn reopened(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D25, D86: entries are not logged; a
/// database opened again after a crash has the index of its last
/// checkpoint mended by the rows recovery redid, committed changes after
/// the checkpoint found and one uncommitted not, and again after another.
#[test]
fn the_index_is_recovered_after_a_crash() {
    let scratch = Scratch::new("fulltext-recovery");
    let dir = scratch.path("db");
    let db = reopened(&dir);
    let s = documents(&db, &TEXTS);
    db.checkpoint().unwrap();
    exec(&s, "INSERT INTO DOCS VALUES (6, 'zebra crossing')").unwrap();
    exec(&s, "UPDATE DOCS SET BODY = 'zebra stripes' WHERE ID = 4").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "DELETE FROM DOCS WHERE ID = 1").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "INSERT INTO DOCS VALUES (7, 'zebra uncommitted')").unwrap();
    // The writer stays open across the crash, its insert never committed.
    drop(db);
    for _ in 0..2 {
        let db = reopened(&dir);
        let s = db.session("S");
        assert!(probed(&s, "zebra"));
        assert_eq!(found(&s, "zebra"), ["4", "6"]);
        assert_eq!(found(&s, "gnome"), Vec::<String>::new());
        assert_eq!(found(&s, "fox"), Vec::<String>::new(), "row 1 deleted");
        exec(&s, "COMMIT WORK").unwrap();
        db.checkpoint().unwrap();
    }
    drop(s);
}

/// Verifies: [EXTENSION] RULES.md D49, D86: a temporary table's full-text
/// index holds each session's own rows, made as its instance is, and a
/// search of it finds them alone.
#[test]
fn a_temporary_table_has_a_full_text_index_of_its_own_rows() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE GLOBAL TEMPORARY TABLE NOTES (ID INTEGER, BODY VARCHAR(100)) ON COMMIT PRESERVE ROWS")
        .unwrap();
    let s = db.session("S");
    exec(&s, "CREATE INDEX NOTES_BODY ON NOTES (BODY) USING fulltext").unwrap();
    let other = db.session("S");
    exec(&s, "INSERT INTO NOTES VALUES (1, 'buy milk'), (2, 'call the vet')").unwrap();
    let filler: Vec<String> = (FILLER..FILLER + 2000).map(|k| format!("({k}, 'filler row number {k}')")).collect();
    for chunk in filler.chunks(200) {
        exec(&s, &format!("INSERT INTO NOTES VALUES {}", chunk.join(", "))).unwrap();
    }
    exec(&other, "INSERT INTO NOTES VALUES (9, 'buy bread')").unwrap();
    // Each session reads its index once its writing transaction ends.
    exec(&s, "COMMIT WORK").unwrap();
    exec(&other, "COMMIT WORK").unwrap();
    let search = "SELECT ID FROM NOTES WHERE TEXT_MATCHES(BODY, 'buy') ORDER BY ID";
    assert!(s.physical_plan(search, &Params::new()).unwrap().contains("using NOTES_BODY"));
    assert_eq!(rows(&s, search).unwrap(), ["1"]);
    assert_eq!(rows(&other, search).unwrap(), ["9"]);
}

/// Verifies: [EXTENSION] RULES.md D48, D86: a full-text index keeps a
/// table's words in the clear, so an encrypted table is refused one where
/// the database's files are not encrypted (SQLCODE -103), and given one
/// where they are; its words then reach the data file encrypted.
#[test]
fn an_encrypted_tables_words_are_indexed_only_in_encrypted_files() {
    let scratch = Scratch::new("fulltext-encrypted");
    let open = |dir: &Path, files: Option<&str>| {
        let db = Database::new();
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
        if let Some(files) = files {
            db.set_setting("STORAGE.ENCODINGS", &Value::Char(files.into())).unwrap();
        }
        db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
        db
    };
    let table = "CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (BODY VARCHAR(100)) WITH (ENCODINGS = 'test scramble')";
    let index = "CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext";
    let plain = open(&scratch.path("plain"), None);
    plain.schema(table).unwrap();
    assert_eq!(exec(&plain.session("S"), index).err(), Some(Code::SyntaxRule));
    plain.close().unwrap();
    let dir = scratch.path("encrypted");
    let hidden = open(&dir, Some("test scramble"));
    hidden.schema(table).unwrap();
    let s = hidden.session("S");
    exec(&s, index).unwrap();
    exec(&s, "INSERT INTO DOCS VALUES ('confidential paragraph')").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    hidden.checkpoint().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM DOCS WHERE TEXT_MATCHES(BODY, 'paragraph')").unwrap(), ["1"]);
    assert!(!crate::common::holds(&crate::common::bytes_of(&dir, "data"), "paragraph"), "no word in the clear");
}
