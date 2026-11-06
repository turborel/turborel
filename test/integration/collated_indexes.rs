//! Indexes on collated columns [RULES.md D60]: sort keys in the index, the
//! plans that may use them and those that must not, and what survives a
//! reopening.

use std::sync::Arc;

use turborel::db::{Cursor, Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::Scratch;

fn lines(mut cursor: Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

fn query(s: &Session, sql: &str) -> Vec<String> {
    lines(s.open_cursor(sql).unwrap())
}

/// Schema A: T, keyed by NAME under CASE_INSENSITIVE, with B, and enough
/// rows that a probe is cheaper than a scan.
fn indexed() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE NOT NULL UNIQUE, B INTEGER)")
        .unwrap();
    let a = db.session("A");
    for n in 0..200 {
        a.run(&format!("INSERT INTO T VALUES ('Name{n}', {n})")).unwrap();
    }
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D60: an equality under a column's collation
/// probes its index of sort keys, and gives the row's text, not a key; one
/// by another collation scans, its keys no probe of that index, and a key
/// of sort keys covers no query.
#[test]
fn an_index_of_sort_keys_is_probed_by_its_own_collation() {
    let db = indexed();
    let a = db.session("A");
    let plan = |sql: &str| a.explain(sql, &Params::new()).unwrap();
    assert!(plan("SELECT B FROM T WHERE NAME = 'NAME7'").contains("Index"), "{}", plan("SELECT B FROM T WHERE NAME = 'NAME7'"));
    assert_eq!(query(&a, "SELECT NAME FROM T WHERE NAME = 'NAME7'"), ["Name7"], "the text, not a key");
    assert!(!plan("SELECT B FROM T WHERE NAME COLLATE UCS_BASIC = 'Name7'").contains("Index"));
    assert_eq!(query(&a, "SELECT B FROM T WHERE NAME COLLATE UCS_BASIC = 'Name7'"), ["7"]);
    assert_eq!(query(&a, "SELECT B FROM T WHERE NAME COLLATE UCS_BASIC = 'NAME7'").len(), 0);
    assert_eq!(query(&a, "SELECT B FROM T WHERE NAME COLLATE UNICODE = 'Name7'"), ["7"], "another collation's keys are no probe of it");
    let ordered = query(&a, "SELECT NAME FROM T ORDER BY NAME COLLATE UCS_BASIC");
    assert_eq!((ordered.first().map(String::as_str), ordered.get(1).map(String::as_str)), (Some("Name0"), Some("Name1")));
    assert_eq!(ordered.len(), 200);
}

/// Verifies: [EXTENSION] RULES.md D60, D29: a read through an index of sort
/// keys tells the engine it may have read any row, so a phantom is seen: of
/// two that each find no row, then insert what the other looked for, the
/// second fails its commit (SQLCODE -911), though no row either read
/// changed, as a read of no row leaves only its condition.
#[test]
fn a_read_through_sort_keys_is_tracked_for_its_commit() {
    let db = indexed();
    crate::common::serializable(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(query(&a, "SELECT B FROM T WHERE NAME = 'nobody'").len(), 0);
    assert_eq!(query(&b, "SELECT B FROM T WHERE NAME = 'somebody'").len(), 0);
    a.run("INSERT INTO T VALUES ('SOMEBODY', 1000)").unwrap();
    b.run("INSERT INTO T VALUES ('NOBODY', 1001)").unwrap();
    a.commit().unwrap();
    assert_eq!(b.commit().err().map(|e| e.code), Some(Code::Conflict));
}

/// Verifies: [EXTENSION] RULES.md D60, D25: a database kept in a directory
/// keeps a collation defined and its column's, its index still in the order
/// of the collation, reopened from a checkpoint after it closes, and from
/// its log alone, as after a failure.
#[test]
fn a_collation_survives_reopening() {
    for closed in [true, false] {
        let scratch = Scratch::new(if closed { "collated-closed" } else { "collated-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION A
               CREATE COLLATION TIGHT FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
               CREATE TABLE T (C VARCHAR(9) COLLATE A.TIGHT UNIQUE)",
        )
        .unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES ('Bob')").unwrap();
        a.commit().unwrap();
        if closed {
            db.close().unwrap();
        }
        drop(a);
        drop(db);
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        let a = db.session("A");
        assert_eq!(query(&a, "SELECT C FROM T WHERE C = 'BOB'"), ["Bob"]);
        assert_eq!(a.run("INSERT INTO T VALUES ('bob')").err().map(|e| e.code), Some(Code::Constraint));
        a.rollback().unwrap();
        a.run("INSERT INTO T VALUES ('bob ')").unwrap();
        a.rollback().unwrap();
        db.close().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.11; RULES.md D60] a subquery's
/// kept rows are by a string as it is: strings a comparison holds equal, of
/// trailing spaces, that the subquery tells apart, each run it (a defect:
/// the second took the first's rows).
#[test]
fn a_subquery_tells_apart_strings_equal_when_padded() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, V VARCHAR(5))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'a'), (2, 'a ')").unwrap();
    let found = query(&a, "SELECT K, (SELECT CHAR_LENGTH(T.V) FROM T AS I WHERE I.K = 1) FROM T ORDER BY K");
    assert_eq!(found, ["1,1", "2,2"]);
}

/// Verifies: [EXTENSION] RULES.md D60, D49: the column store and temporary
/// tables take collated columns, as any table does.
#[test]
fn other_tables_take_collations() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K VARCHAR(9) COLLATE CASE_INSENSITIVE UNIQUE) USING columnstore
           CREATE GLOBAL TEMPORARY TABLE G (K VARCHAR(9) COLLATE CASE_INSENSITIVE UNIQUE) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO C VALUES ('Bob')").unwrap();
    a.run("INSERT INTO G VALUES ('Bob')").unwrap();
    assert_eq!(a.run("INSERT INTO C VALUES ('bob')").err().map(|e| e.code), Some(Code::Constraint));
    a.rollback().unwrap();
    a.run("INSERT INTO G VALUES ('Bob')").unwrap();
    assert_eq!(a.run("INSERT INTO G VALUES ('bob')").err().map(|e| e.code), Some(Code::Constraint));
    a.rollback().unwrap();
}
