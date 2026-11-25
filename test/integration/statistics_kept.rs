//! A table's statistics kept with its pages (RULES.md D1093): a database
//! opened again plans by the statistics it was closed with, no pass made;
//! a table that never had any gathers its first as a plan asks; a table
//! truncated or dropped takes its statistics with it; and those of a
//! table whose rows are sealed are sealed in the data file.

use std::path::Path;

use turborel::db::{Database, OpenOptions};
use turborel::threads::JobPolicy;

use crate::common::{RunLength, Scramble, Scratch, bytes_of, exec, holds, os, rows};

/// A database opened from `dir`, its background work done only when a
/// test asks, the test encodings registered if `registered`.
fn open(dir: &Path, registered: bool) -> Database {
    let db = Database::new();
    if registered {
        db.register_encoding(Box::new(RunLength)).unwrap();
        db.register_encoding(Box::new(Scramble)).unwrap();
    }
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db
}

/// Inserts `n` rows into `table` of schema APP, K 0 to `n` less one and C
/// the text `word` and K, committed.
fn fill(db: &Database, table: &str, (word, n): (&str, u32)) {
    let s = db.session("APP");
    for k in 0..n {
        exec(&s, &format!("INSERT INTO {table} VALUES ({k}, '{word} {k:03}')")).unwrap();
    }
    exec(&s, "COMMIT WORK").unwrap();
}

/// What COLUMN_STATISTICS shows of column C of `table`: its rows and its
/// least value.
fn shown(db: &Database, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT ROW_COUNT, MIN_VALUE FROM INFORMATION_SCHEMA.COLUMN_STATISTICS \
         WHERE TABLE_NAME = '{table}' AND COLUMN_NAME = 'C'"
    );
    rows(&db.session("APP"), &sql).unwrap()
}

/// Verifies: [DESIGN] RULES.md D1093: the statistics a pass gathered are
/// saved by the checkpoint a close takes, and the database opened again
/// has them at once, as that pass found them: a row written after the
/// pass, of a value below every other, is not their least value, as it
/// would be had a pass been made by the asking; a table whose statistics
/// were never gathered has none saved, and once opened gathers them by a
/// pass as it is asked (D1092), which finds its rows as they are. MC/DC
/// of the pass a table opened makes: gathered and saved (none made)
/// against never gathered (made).
/// Regression test (R-TEST-7): a table's statistics were lost at every
/// start, so the first plan of each table after a start read a sample of
/// it before it could be made.
#[test]
fn a_database_opened_has_the_statistics_it_was_closed_with() {
    let scratch = Scratch::new("statistics-kept");
    let dir = scratch.path("db");
    let db = open(&dir, false);
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER, C VARCHAR(12)) CREATE TABLE U (K INTEGER, C VARCHAR(12))")
        .unwrap();
    fill(&db, "T", ("known", 120));
    db.maintain().unwrap();
    fill(&db, "U", ("never", 30));
    assert_eq!(shown(&db, "T"), ["120,known 000"], "gathered in this run");
    for table in ["T", "U"] {
        fill(&db, table, ("after", 1));
    }
    db.close().unwrap();
    let again = open(&dir, false);
    assert_eq!(shown(&again, "T"), ["120,known 000"], "kept as the pass found them, no pass made");
    assert_eq!(shown(&again, "U"), ["31,after 000"], "never gathered: none kept, a pass made by the asking");
    let s = again.session("APP");
    let plan = s.physical_plan("SELECT K FROM T WHERE K < 30", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("est. rows=3") || plan.contains("est. rows=2"), "planned by them: {plan}");
    assert_eq!(shown(&again, "T"), ["120,known 000"], "the plan made no pass");
    again.close().unwrap();
    assert_eq!(shown(&open(&dir, false), "T"), ["120,known 000"], "kept through a close that saved none anew");
}

/// Verifies: [DESIGN] RULES.md D1093, [ISO/IEC 9075-2:2023 §14.10]: the
/// statistics of a table truncated, and of one dropped, go with its
/// storage: saved by a checkpoint before, they are not those of the table
/// emptied, nor of a table made after of the same name, in this run or
/// once the database is opened again.
#[test]
fn a_truncated_or_dropped_tables_statistics_go_with_it() {
    let scratch = Scratch::new("statistics-kept-truncated");
    let dir = scratch.path("db");
    let db = open(&dir, false);
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER, C VARCHAR(12)) CREATE TABLE D (K INTEGER, C VARCHAR(12))")
        .unwrap();
    fill(&db, "T", ("gone", 80));
    fill(&db, "D", ("gone", 80));
    db.maintain().unwrap();
    db.checkpoint().unwrap();
    assert_eq!((shown(&db, "T"), shown(&db, "D")), (vec!["80,gone 000".to_owned()], vec!["80,gone 000".to_owned()]));
    let s = db.session("APP");
    exec(&s, "TRUNCATE TABLE T").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "DROP TABLE D CASCADE").unwrap();
    exec(&s, "CREATE TABLE D (K INTEGER, C VARCHAR(12))").unwrap();
    exec(&s, "COMMIT WORK").unwrap();
    drop(s);
    assert_eq!((shown(&db, "T"), shown(&db, "D")), (vec!["NULL,NULL".to_owned()], vec!["NULL,NULL".to_owned()]));
    db.close().unwrap();
    let again = open(&dir, false);
    // Gathered by the asking, of the rows each holds now.
    assert_eq!((shown(&again, "T"), shown(&again, "D")), (vec!["0,NULL".to_owned()], vec!["0,NULL".to_owned()]));
    assert_eq!(rows(&again.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["0".to_owned()]));
}

/// Verifies: [EXTENSION] RULES.md D1093, D30, D48: the statistics saved
/// of an encrypted table, which hold a sample of its rows and its
/// values' bounds, are sealed as its rows are, so the data file holds
/// none of them; opened with its encodings the table has them, and opened
/// without, the database still opens, the table's statistics not known.
#[test]
fn an_encrypted_tables_statistics_are_sealed() {
    let scratch = Scratch::new("statistics-kept-sealed");
    let dir = scratch.path("db");
    let db = open(&dir, true);
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE Z (K INTEGER, C VARCHAR(12)) WITH (ENCODINGS = 'test scramble')").unwrap();
    fill(&db, "Z", ("hidden", 60));
    db.maintain().unwrap();
    assert_eq!(shown(&db, "Z"), ["60,hidden 000"]);
    db.checkpoint().unwrap();
    assert!(!holds(&bytes_of(&dir, "data"), "hidden"), "no sampled row or bound of Z is stored as it is");
    db.close().unwrap();
    let again = open(&dir, true);
    assert_eq!(shown(&again, "Z"), ["60,hidden 000"], "read through its encodings");
    again.close().unwrap();
    let without = open(&dir, false);
    assert_eq!(shown(&without, "Z"), ["NULL,NULL"], "not known, the table opened");
    without.close().unwrap();
}
