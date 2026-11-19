//! UUID, a built-in type of 16 octets (RULES.md D180 to D183): columns of
//! it, its literal and casts, comparison, keys and indexes, temporary
//! tables, the Information Schema's description, files kept, and its
//! generation functions.

mod generated;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, os, refused, rows, sorted};

/// A UUID of version 4 [RFC 9562 Appendix B.2].
const ONE: &str = "919108f7-52d1-4320-9bac-f847db4148a8";
/// A UUID below it.
const TWO: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";

/// A database of schema A, table T of a UUID K, the primary key, and a
/// name N, its two rows inserted as text, upper case and braced.
fn identified() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K UUID PRIMARY KEY, N VARCHAR(5))").unwrap();
    let s = db.session("A");
    s.run(&format!("INSERT INTO T VALUES ('{}', 'ONE'), ('{{{TWO}}}', 'TWO')", ONE.to_uppercase())).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D180, D181: a UUID column takes text in
/// INSERT, shows its canonical text, orders by octets, is found by its
/// literal and by CAST of text with no hyphens, refuses a duplicate key
/// and text not a UUID's (22018); CAST to a character string and a binary
/// string give its text and octets, and a binary string of 16 octets
/// casts back, one of 2 octets refused with 22018; a UUID is not compared
/// with text.
#[test]
fn uuid_columns_store_compare_and_cast() {
    let db = identified();
    let s = db.session("A");
    assert_eq!(rows(&s, "SELECT K, N FROM T ORDER BY K"), Ok(vec![format!("{TWO},TWO"), format!("{ONE},ONE")]));
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE K = UUID '{ONE}'")), Ok(vec!["ONE".into()]));
    let hyphenless = ONE.replace('-', "");
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE K = CAST('{hyphenless}' AS UUID)")), Ok(vec!["ONE".into()]));
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE K > UUID '{TWO}'")), Ok(vec!["ONE".into()]));
    assert_eq!(rows(&s, "SELECT CAST(K AS VARCHAR(36)) FROM T WHERE N = 'ONE'"), Ok(vec![ONE.into()]));
    let octets = rows(&s, "SELECT CAST(K AS VARBINARY(16)) FROM T WHERE N = 'TWO'").unwrap();
    assert_eq!(octets, vec!["0F0E0D0C0B0A49088706050403020100".to_owned()]);
    assert_eq!(rows(&s, "SELECT N FROM T WHERE K = CAST(X'0F0E0D0C0B0A49088706050403020100' AS UUID)"), Ok(vec!["TWO".into()]));
    assert_eq!(s.run(&format!("INSERT INTO T VALUES ('{ONE}', 'DUP')")).unwrap_err().code, Code::Constraint);
    assert_eq!(s.run("INSERT INTO T VALUES ('not-a-uuid', 'BAD')").unwrap_err().sqlstate().as_str(), "22018");
    assert_eq!(refused(&s, "SELECT CAST(X'0102' AS UUID) FROM T").1, "22018");
    assert_eq!(refused(&s, "SELECT CAST(1 AS UUID) FROM T").1, "22005");
    assert_eq!(code(&s, "SELECT N FROM T WHERE K = 'text'"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D180: a hash index and a btree index on
/// a UUID column find its rows; GROUP BY and DISTINCT group equal UUIDs;
/// a view's text keeps its literal, which reads back.
#[test]
fn uuid_indexes_and_grouping() {
    let db = identified();
    db.create_index("T_K_HASH", "A.T", &["K"], "hash").unwrap();
    db.create_index("T_NK", "A.T", &["N", "K"], "btree").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (UUID 'ffffffff-ffff-4fff-bfff-ffffffffffff', 'THREE')").unwrap();
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE K = UUID '{TWO}'")), Ok(vec!["TWO".into()]));
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE N = 'ONE' AND K = UUID '{ONE}'")), Ok(vec!["ONE".into()]));
    assert_eq!(rows(&s, &format!("SELECT N FROM T WHERE N = 'ONE' AND K > UUID '{ONE}'")), Ok(vec![]));
    assert_eq!(rows(&s, "SELECT COUNT(DISTINCT K) FROM (SELECT K FROM T UNION ALL SELECT K FROM T) AS U"), Ok(vec!["3".into()]));
    assert_eq!(sorted(&s, "SELECT K, COUNT(*) FROM (SELECT K FROM T UNION ALL SELECT K FROM T) AS U GROUP BY K").len(), 3);
    s.run(&format!("CREATE VIEW V AS SELECT N FROM T WHERE K = UUID '{ONE}'")).unwrap();
    assert_eq!(rows(&s, "SELECT N FROM V"), Ok(vec!["ONE".into()]));
}

/// Verifies: [EXTENSION] RULES.md D180: COLUMNS gives a UUID column's
/// DATA_TYPE as UUID, its length, octets, precision and datetime
/// precision null, as no descriptor of a length or a number holds of it.
#[test]
fn uuid_columns_are_described() {
    let db = identified();
    let s = db.session("A");
    let described = "SELECT DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH, NUMERIC_PRECISION, DATETIME_PRECISION \
                     FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'K'";
    assert_eq!(rows(&s, described), Ok(vec!["UUID,NULL,NULL,NULL,NULL".into()]));
}

/// Verifies: [EXTENSION] RULES.md D180: a global and a declared local
/// temporary table hold UUIDs, each session's own, keyed by them.
#[test]
fn temporary_tables_hold_uuids() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE GLOBAL TEMPORARY TABLE G (K UUID NOT NULL UNIQUE) ON COMMIT PRESERVE ROWS").unwrap();
    let (one, two) = (db.session("A"), db.session("A"));
    one.run(&format!("INSERT INTO G VALUES ('{ONE}'), ('{TWO}')")).unwrap();
    two.run(&format!("INSERT INTO G VALUES ('{ONE}')")).unwrap();
    assert_eq!(rows(&one, "SELECT K FROM G ORDER BY K"), Ok(vec![TWO.to_owned(), ONE.to_owned()]));
    assert_eq!(rows(&two, "SELECT K FROM G"), Ok(vec![ONE.to_owned()]));
    assert_eq!(one.run(&format!("INSERT INTO G VALUES (UUID '{TWO}')")).unwrap_err().code, Code::Constraint);
    one.run("DECLARE LOCAL TEMPORARY TABLE MODULE.L (K UUID PRIMARY KEY) ON COMMIT PRESERVE ROWS").unwrap();
    one.run("INSERT INTO MODULE.L SELECT K FROM G").unwrap();
    assert_eq!(rows(&one, &format!("SELECT K FROM MODULE.L WHERE K = UUID '{ONE}'")), Ok(vec![ONE.to_owned()]));
}

/// Verifies: [EXTENSION] RULES.md D180: UUIDs kept in a database's files,
/// and their primary key, are there when it opens again.
#[test]
fn uuids_are_kept() {
    let scratch = Scratch::new("uuids");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K UUID PRIMARY KEY)").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES ('{ONE}'), ('{TWO}')")).unwrap();
    a.commit().unwrap();
    drop((a, db));
    let db = open();
    let s = db.session("A");
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K"), Ok(vec![TWO.to_owned(), ONE.to_owned()]));
    assert_eq!(rows(&s, &format!("SELECT K FROM T WHERE K = UUID '{ONE}'")), Ok(vec![ONE.to_owned()]));
}
