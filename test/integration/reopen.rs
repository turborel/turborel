//! A database kept in a directory (RULES.md D25): created,
//! closed cleanly and reopened in place with its definitions, rows,
//! indexes and constraints; recovered from its log after stopping without
//! closing; and the refusals of opening and closing.

use std::fs::OpenOptions as FileOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use turborel::db::{Completion, Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::hash::HashFunction;
use turborel::log::record::{Change, Record};
use turborel::log::{FileLog, FileLogOptions, Log};
use turborel::params::Params;
use turborel::txn::NO_TRANSACTION;

use crate::common::{LOCKING, LockingEngine, Scratch, after_crash, crc32c, exec, os, rows};

/// The database in `dir`, opened with a small buffer pool, so pages are
/// written back while it runs; `LockingEngine` registered first.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    crate::common::small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Schema APP: T in the row store, with a UNIQUE key, a B-link index and a
/// hash index; L in `LockingEngine`, whose tables keep nothing.
fn define(db: &Database) {
    db.place_table("APP.L", LOCKING).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, NAME CHAR(20), AMT DECIMAL(9,2))
           CREATE TABLE L (K INTEGER NOT NULL)",
    )
    .unwrap();
    db.create_index("T_NAME", "APP.T", &["NAME"], "btree").unwrap();
    db.create_index("T_AMT", "APP.T", &["AMT"], "hash").unwrap();
}

/// 3,000 rows of T and 10 of L, committed; some updated and deleted; a
/// transaction rolled back, and a later one changing the same row.
fn fill(db: &Database) {
    let s = db.session("APP");
    for k in 0..3_000 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'N{k}', {})", k % 50)).unwrap();
    }
    for k in 0..10 {
        s.run(&format!("INSERT INTO L VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET NAME = 'changed' WHERE K < 100").unwrap();
    s.run("DELETE FROM T WHERE K >= 2900").unwrap();
    s.commit().unwrap();
    s.run("UPDATE T SET AMT = 999 WHERE K = 5").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET AMT = 7 WHERE K = 5").unwrap();
    s.commit().unwrap();
}

/// What the tests compare before and after reopening.
fn answers(db: &Database) -> Vec<Result<Vec<String>, Code>> {
    let s = db.session("APP");
    [
        "SELECT COUNT(*), SUM(AMT) FROM T",
        "SELECT K FROM T WHERE NAME = 'N777'",
        "SELECT COUNT(*) FROM T WHERE NAME = 'changed'",
        "SELECT COUNT(*) FROM T WHERE AMT = 5",
        "SELECT AMT FROM T WHERE K = 5",
        "SELECT COUNT(*) FROM L",
    ]
    .iter()
    .map(|q| rows(&s, q))
    .collect()
}

/// Verifies: [X3.135-1986 §4.16 "committed changes cannot be canceled"];
/// [DESIGN] RULES.md D25, D31, a database closed cleanly and opened again
/// is reopened in place: its rows, its B-link index (used by the planner),
/// its hash index, its UNIQUE constraint, and a table of an engine that
/// keeps nothing, replayed from the log; new rows go on with new ids, and
/// it closes and reopens again; a closed database refuses every call.
#[test]
fn a_closed_database_reopens_in_place() {
    let scratch = Scratch::new("reopen-clean");
    let dir = scratch.path("db");
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Created);
    define(&db);
    fill(&db);
    let before = answers(&db);
    assert_eq!(before[0], Ok(vec!["2900,71052.00".into()]));
    assert_eq!(before[4], Ok(vec!["7.00".into()]));
    db.close().unwrap();
    assert_eq!(db.session("APP").run("COMMIT WORK").err().map(|e| e.code), Some(Code::Api), "closed");
    assert_eq!(db.close().err().map(|e| e.code), Some(Code::Api));
    drop(db);

    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(answers(&db), before);
    let s = db.session("APP");
    assert!(
        s.physical_plan("SELECT K FROM T WHERE NAME = 'N777'", &Params::new()).unwrap().contains("T_NAME"),
        "the reopened index is used"
    );
    assert_eq!(s.run("INSERT INTO T VALUES (7, 'dup', 1)").err().map(|e| e.code), Some(Code::Constraint), "UNIQUE");
    s.run("INSERT INTO T VALUES (5000, 'new', 1)").unwrap();
    s.run("INSERT INTO L VALUES (99)").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);

    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE NAME = 'new'"), Ok(vec!["5000".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM L"), Ok(vec!["11".into()]));
}

/// Verifies: [X3.135-1986 §4.16, §8.9]; [DESIGN] RULES.md D25, a database
/// that stops without closing is recovered from its log: what was
/// committed, a row a rolled-back transaction changed and a later one
/// committed holding the later value, and nothing of a transaction open
/// when it stopped; then it closes and reopens in place.
#[test]
fn a_database_not_closed_is_recovered() {
    let scratch = Scratch::new("reopen-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    define(&crashed);
    fill(&crashed);
    let s = crashed.session("APP");
    s.run("UPDATE T SET AMT = 1 WHERE K = 3").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET AMT = 2 WHERE K = 3").unwrap();
    s.commit().unwrap();
    let expected = answers(&crashed);
    let open_tx = crashed.session("APP");
    open_tx.run("INSERT INTO T VALUES (8000, 'open', 1)").unwrap();
    open_tx.run("DELETE FROM L").unwrap();

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(report) if report.committed >= 3), "{opened:?}");
    assert_eq!(answers(&db), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT AMT FROM T WHERE K = 3"), Ok(vec!["2.00".into()]), "the later commit's value");
    assert_eq!(rows(&db.session("APP"), "SELECT K FROM T WHERE NAME = 'open'"), Ok(vec![]));
    db.close().unwrap();
    drop(db);
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(answers(&db), expected);
    drop((db, open_tx, s));
}

/// Verifies: [DESIGN] RULES.md D25, the catalog file: a record cut short by
/// a crash is cut off, the one before it standing; a database closed
/// cleanly whose log then grew past the position it was closed at is
/// recovered from the log, not reopened.
#[test]
fn a_torn_catalog_record_is_cut_off() {
    let scratch = Scratch::new("reopen-torn");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    define(&db);
    fill(&db);
    let expected = answers(&db);
    db.close().unwrap();
    drop(db);
    FileOptions::new().append(true).open(dir.join("catalog.0")).unwrap().write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9]).unwrap();
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened, "the clean record stands");
    assert_eq!(answers(&db), expected);
    db.close().unwrap();
    drop(db);
    // A record appended to the log after the clean close: not reopened.
    let hash: Arc<dyn HashFunction> = crc32c();
    let (log, _) = FileLog::open(os(), &dir.join("log"), &[hash], &[], FileLogOptions::default()).unwrap();
    let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::Checkpoint }.encode()).unwrap();
    log.sync(at).unwrap();
    drop(log);
    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(answers(&db), expected);
}

/// Verifies: [DESIGN] RULES.md D25, refusals: a database is not closed with
/// a transaction open, nor if it was not opened from files; it is opened
/// from files only while it holds no schema, log or files.
#[test]
fn opening_and_closing_refuse_what_they_cannot_do() {
    let scratch = Scratch::new("reopen-refusals");
    let (db, _) = open(&scratch.path("db"));
    let s = db.session("APP");
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    s.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(db.close().err().map(|e| e.code), Some(Code::Api), "a transaction is open");
    s.commit().unwrap();
    assert_eq!(db.open_files(&os(), &scratch.path("other"), &OpenOptions::default()).err().map(|e| e.code), Some(Code::Api));
    db.close().unwrap();
    assert_eq!(Database::new().close().err().map(|e| e.code), Some(Code::Api), "not opened from files");
    let memory = Database::new();
    memory.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    assert_eq!(memory.open_files(&os(), &scratch.path("third"), &OpenOptions::default()).err().map(|e| e.code), Some(Code::Api));
    let reopened = Database::open(&scratch.path("db")).unwrap();
    assert_eq!(rows(&reopened.session("APP"), "SELECT K FROM T"), Ok(vec!["1".into()]));
}

/// Verifies: [X3.135-1986 §6.6]; [DESIGN] RULES.md D25, the privileges a
/// `<schema>` grants are kept with its definitions: after a clean close,
/// and after stopping without one.
#[test]
fn privileges_are_kept_6_6() {
    let scratch = Scratch::new("reopen-privileges");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE Q (X INTEGER) GRANT SELECT ON Q TO R").unwrap();
    db.session("O").run("INSERT INTO Q VALUES (1)").unwrap();
    let readable = |db: &Database| (rows(&db.session("R"), "SELECT X FROM O.Q"), rows(&db.session("S"), "SELECT X FROM O.Q"));
    let expected = (Ok(vec!["1".into()]), Err(Code::Privilege));
    assert_eq!(readable(&db), expected);
    db.close().unwrap();
    drop(db);

    let (crashed, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(readable(&crashed), expected);
    crashed.schema("CREATE SCHEMA AUTHORIZATION S2 CREATE TABLE Q2 (X INTEGER) GRANT INSERT ON Q2 TO PUBLIC").unwrap();

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(readable(&db), expected);
    assert_eq!(db.session("ANY").run("INSERT INTO S2.Q2 VALUES (1)").map(drop).map_err(|e| e.code), Ok(()));
    drop(crashed);
}

/// Verifies: [X3.135-1986 §6.5]; [DESIGN] RULES.md D25, views are kept with
/// their schema's definitions, and read and changed through after a clean
/// close and after stopping without one.
#[test]
fn views_are_kept_6_5() {
    let scratch = Scratch::new("reopen-views");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O CREATE TABLE Q (X INTEGER) CREATE VIEW BIG (Y) AS SELECT X FROM Q WHERE X > 5 WITH CHECK OPTION",
    )
    .unwrap();
    db.session("O").run("INSERT INTO BIG VALUES (9)").unwrap();
    db.close().unwrap();
    drop(db);

    let (crashed, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let s = crashed.session("O");
    assert_eq!(rows(&s, "SELECT Y FROM BIG"), Ok(vec!["9".into()]));
    assert_eq!(s.run("INSERT INTO BIG VALUES (1)").map(drop).map_err(|e| e.code), Err(Code::Constraint));
    s.run("INSERT INTO BIG VALUES (7)").unwrap();
    s.commit().unwrap();

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("O"), "SELECT Y FROM BIG ORDER BY Y"), Ok(vec!["7".into(), "9".into()]));
    drop((s, crashed));
}

/// Verifies: [EXTENSION] RULES.md D34, D25: a dropped index stays
/// dropped when the database is reopened, whether it was dropped before a
/// clean close (the catalog's definitions) or before a crash (the log); its
/// name may be used again; the answers do not change.
#[test]
fn dropped_indexes_stay_dropped() {
    let scratch = Scratch::new("reopen-drop");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    define(&db);
    fill(&db);
    let expected = answers(&db);
    db.drop_index("T_NAME").unwrap();
    db.close().unwrap();
    drop(db);
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let plan = |db: &Database| db.session("APP").physical_plan("SELECT K FROM T WHERE NAME = 'N777'", &Params::new()).unwrap();
    assert!(!plan(&db).contains("T_NAME"), "{}", plan(&db));
    db.drop_index("T_AMT").unwrap();
    db.create_index("T_NAME", "APP.T", &["NAME"], "btree").unwrap();
    assert_eq!(answers(&db), expected);
    let (db, opened) = {
        let crashed = db;
        let reopened = open(&dir);
        drop(crashed);
        reopened
    };
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.drop_index("T_AMT").map_err(|e| e.code), Err(Code::Api), "dropped before the crash");
    assert!(plan(&db).contains("T_NAME"), "made again before the crash");
    assert_eq!(answers(&db), expected);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18], RULES.md D33,
/// D25: a table dropped by a transaction that committed stays dropped when
/// the database is reopened, after a clean close or a crash; one whose
/// transaction was open at the crash is still there, with its rows.
/// Regression test: a reopen rebuilt the indexes it does not keep before
/// moving the clock past the timestamps its tables' versions were stamped
/// with, and so looked for versions older than those, which a clean close
/// does not keep. The crashed database's session, ended after the reopen,
/// wrote its rollback over the log the reopened one wrote to, which the
/// next open refuses as damage.
#[test]
fn dropped_tables_stay_dropped() {
    let scratch = Scratch::new("reopen-drop-table");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    define(&db);
    fill(&db);
    let s = db.session("APP");
    s.run("DROP TABLE L CASCADE").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT K FROM L").map_err(|_| ()), Err(()), "dropped before the close");
    s.run("DELETE FROM T WHERE K = 0").unwrap();
    s.run("DROP TABLE T CASCADE").unwrap();
    let kept = rows(&s, "SELECT COUNT(*) FROM T").map_err(|_| ());
    assert_eq!(kept, Err(()), "dropped for its transaction");
    // Kept to the end, its transaction open: a process that crashed writes
    // nothing more, and its rollback would write to the log the next opener
    // writes to.
    let crashed = (s, db);
    let (recovered, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = recovered.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T"), Ok(vec!["2900".into()]), "its transaction was open: T and its rows stay");
    s.run("DROP TABLE T CASCADE").unwrap();
    s.commit().unwrap();
    let (db, opened) = {
        let crashed = recovered;
        let reopened = open(&dir);
        drop((s, crashed));
        reopened
    };
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T").map_err(|_| ()), Err(()), "committed before the crash");
    drop(crashed);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11, §11.15], RULES.md D33,
/// RULES.md D25: a table altered by a transaction that committed is
/// reopened altered, after a clean close or a crash: its rows in the new
/// generation of its storage, with their values, its index reopened or
/// rebuilt; one altered by a transaction open at the crash is as it was.
#[test]
fn altered_tables_stay_altered() {
    let scratch = Scratch::new("reopen-alter-table");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    define(&db);
    fill(&db);
    let s = db.session("APP");
    s.run("ALTER TABLE T ADD COLUMN C INTEGER").unwrap();
    s.run("UPDATE T SET C = K + 1 WHERE K < 10").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(C) FROM T"), Ok(vec!["2900,55".into()]));
    assert_eq!(rows(&s, "SELECT K, C FROM T WHERE NAME = 'N777'"), Ok(vec!["777,NULL".into()]), "through T_NAME");
    s.run("ALTER TABLE T DROP COLUMN AMT CASCADE").unwrap();
    s.commit().unwrap();
    s.run("ALTER TABLE T DROP COLUMN C CASCADE").unwrap();
    let (recovered, opened) = {
        let crashed = db;
        let reopened = open(&dir);
        drop((s, crashed));
        reopened
    };
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = recovered.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(C) FROM T"), Ok(vec!["2900,55".into()]), "C's drop was open");
    assert_eq!(rows(&s, "SELECT AMT FROM T").map_err(|_| ()), Err(()), "AMT's drop committed");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE NAME = 'changed' AND K = 5"), Ok(vec!["5".into()]));
    assert_eq!(recovered.index_entries("T_AMT").map_err(|e| e.code), Err(Code::Api), "its index went with it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.6–§11.9, §11.16], RULES.md
/// D25: constraints defined in a schema, a CHECK with a subquery among
/// them, and added by ALTER TABLE are kept across a clean close and a
/// crash, and enforced after either.
#[test]
fn constraints_are_kept() {
    let scratch = Scratch::new("reopen-constraints");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION K
           CREATE TABLE P (K INTEGER PRIMARY KEY, V INTEGER CHECK (V > 0))
           CREATE TABLE C (K INTEGER REFERENCES P ON DELETE CASCADE, W INTEGER)
           CREATE TABLE E (K INTEGER CHECK (K IN (SELECT K FROM P)))",
    )
    .unwrap();
    let s = db.session("K");
    for sql in ["INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (2, 2)", "INSERT INTO C VALUES (1, 1)", "INSERT INTO C VALUES (2, 2)"]
    {
        s.run(sql).unwrap();
    }
    s.run("ALTER TABLE C ADD UNIQUE (W)").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);
    let (db, opened) = open(&dir);
    assert_eq!(opened, Opened::Reopened);
    let enforced = |db: &Database| {
        let s = db.session("K");
        let codes: Vec<_> = [
            "INSERT INTO P VALUES (1, 5)",
            "INSERT INTO P VALUES (3, -1)",
            "INSERT INTO C VALUES (9, 9)",
            "INSERT INTO C VALUES (1, 1)",
            "INSERT INTO E VALUES (9)",
        ]
        .into_iter()
        .map(|sql| s.run(sql).map_err(|e| e.code).err())
        .collect();
        s.rollback().unwrap();
        codes
    };
    assert_eq!(enforced(&db), vec![Some(Code::Constraint); 5]);
    let s = db.session("K");
    s.run("ALTER TABLE C ADD CONSTRAINT W_UP CHECK (W < 10)").unwrap();
    s.run("DELETE FROM P WHERE K = 2").unwrap();
    s.commit().unwrap();
    let (db, opened) = {
        let crashed = db;
        let reopened = open(&dir);
        drop((s, crashed));
        reopened
    };
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(enforced(&db), vec![Some(Code::Constraint); 5]);
    let s = db.session("K");
    assert_eq!(rows(&s, "SELECT W FROM C"), Ok(vec!["1".into()]), "the cascade stood");
    assert_eq!(s.run("INSERT INTO C VALUES (1, 50)").map_err(|e| e.code).err(), Some(Code::Constraint), "W_UP");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR8b, §10.6], RULES.md
/// D47: a constraint named without a schema, of a table of another schema
/// than the session's default one, CREATE TABLE's and ALTER TABLE's, is
/// made again by recovery in the default schema it was defined in.
#[test]
fn constraint_names_are_recovered_in_their_schema() {
    let scratch = Scratch::new("reopen-constraint-names");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.schema("CREATE SCHEMA S2 AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE TABLE S2.T (K INTEGER CONSTRAINT KEYED PRIMARY KEY)").unwrap();
    s.run("ALTER TABLE S2.T ADD CONSTRAINT POSITIVE CHECK (K > 0)").unwrap();
    s.commit().unwrap();
    let sql = "SELECT CONSTRAINT_SCHEMA, CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = 'S2'";
    let (db, opened) = {
        let crashed = db;
        let reopened = open(&dir);
        drop((s, crashed));
        reopened
    };
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = db.session("U");
    assert_eq!(crate::common::sorted(&s, sql), vec!["U,KEYED", "U,POSITIVE"]);
    let refused = s.run("INSERT INTO S2.T VALUES (-1)").unwrap_err();
    assert_eq!(refused.subject.and_then(|subject| subject.constraint), Some(("U".to_owned(), "POSITIVE".to_owned())));
}

/// Verifies: [DESIGN] RULES.md D25; [ISO/IEC 9075:1992 §11.11 GR2 note]: a
/// view of all of a table that had a column added, and a CHECK whose
/// subquery takes all of one, are reopened seeing the columns they saw, as
/// each keeps with its definition how many of each table's columns it saw.
#[test]
fn views_reopen_seeing_what_they_saw() {
    let scratch = Scratch::new("reopen-view-horizon");
    let dir = scratch.path("db");
    let (db, _) = open(&dir);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE T (K INTEGER)
           CREATE TABLE C (A INTEGER CHECK (A IN (SELECT * FROM T)))
           CREATE VIEW V AS SELECT * FROM T",
    )
    .unwrap();
    let s = db.session("H");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.run("ALTER TABLE T ADD COLUMN M INTEGER").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);
    let (db, _) = open(&dir);
    let s = db.session("H");
    assert_eq!(rows(&s, "SELECT * FROM V"), Ok(vec!["1".into()]));
    assert_eq!(exec(&s, "INSERT INTO C VALUES (1)"), Ok(Completion::Success), "T's K alone");
    assert_eq!(exec(&s, "INSERT INTO C VALUES (2)").map_err(|_| ()), Err(()), "2 is not in T");
}

/// Verifies: [DESIGN] RULES.md D25, D30: a database is open in one place at
/// a time: while one has its directory open, another opening it is refused
/// (SQLCODE -915) before it clears or changes anything there; once the
/// first is closed, the other opens it.
#[test]
fn a_database_is_open_in_one_place_at_a_time() {
    let scratch = Scratch::new("reopen-locked");
    let dir = scratch.path("db");
    let first = Database::new();
    first.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    first.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER)").unwrap();
    first.session("S").run("INSERT INTO T VALUES (1)").unwrap();
    // A fresh open removes everything in undo/.
    let marker = dir.join("undo").join("marker");
    std::fs::write(&marker, b"in use").unwrap();
    let second = Database::new();
    let refused = second.open_files(&os(), &dir, &OpenOptions::default()).unwrap_err();
    assert_eq!(refused.code, Code::Io, "{}", refused.msg);
    assert!(marker.exists(), "nothing of the first's cleared: its older row versions");
    assert_eq!(rows(&first.session("S"), "SELECT K FROM T"), Ok(vec!["1".into()]));
    first.close().unwrap();
    second.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&second.session("S"), "SELECT K FROM T"), Ok(vec!["1".into()]));
    assert!(!marker.exists(), "cleared by the open that holds the lock");
}
