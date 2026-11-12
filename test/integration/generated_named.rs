//! Generated columns whose generation expressions name a domain or a
//! collation [ISO/IEC 9075-2:2023 §11.40 SR2a, §11.44, §12.7 GR12h, GR12z;
//! ISO/IEC 9075:1992 §11.2, §11.37 SR18; RULES.md D63]: DROP DOMAIN, DROP
//! COLLATION, DROP SCHEMA and REVOKE USAGE refuse them with RESTRICT and
//! drop them first with CASCADE.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, after_crash};

fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class of a refusal.
fn refused<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// The columns of table `table`, by name.
fn columns(s: &Session, table: &str) -> Vec<String> {
    query(s, &format!("SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = '{table}' ORDER BY COLUMN_NAME"))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.40 SR2a, §12.7 GR12z]
/// DROP DOMAIN with RESTRICT is refused while a generation expression
/// CASTs to the domain; with CASCADE each such generated column is dropped
/// first, as ALTER TABLE DROP COLUMN CASCADE drops it, its table's rows and
/// other columns kept. If the drop fails once one is dropped, the
/// transaction only rolls back (SQLCODE -913 at its commit); if it fails
/// before, it goes on. MC/DC of that decision's two conditions: T2's
/// generated column, its only one, failing after T1's went dooms; T0's
/// failing first does not; T1's alone, dropped, succeeds.
#[test]
fn domain_dropped_11_40() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN D AS INTEGER CHECK (VALUE > 0)
           CREATE TABLE T1 (A INTEGER, G INTEGER GENERATED ALWAYS AS (CAST(A AS D)))
           CREATE TABLE T2 (G INTEGER GENERATED ALWAYS AS (CAST(1 AS D)))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T1 (A) VALUES (1)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("DROP DOMAIN D RESTRICT")), Some(Code::SyntaxRule), "SR2a");
    a.commit().unwrap();
    assert_eq!(refused(a.run("DROP DOMAIN D CASCADE")), Some(Code::SyntaxRule), "T2's only column");
    assert_eq!(refused(a.commit()), Some(Code::CommitFailed), "doomed");
    assert_eq!(columns(&a, "T1"), ["A", "G"]);
    a.run("CREATE TABLE T0 (G INTEGER GENERATED ALWAYS AS (CAST(2 AS D)))").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("DROP DOMAIN D CASCADE")), Some(Code::SyntaxRule), "T0's only column, first");
    a.commit().unwrap();
    a.run("DROP TABLE T0 RESTRICT").unwrap();
    a.run("DROP TABLE T2 RESTRICT").unwrap();
    a.commit().unwrap();
    a.run("DROP DOMAIN D CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(columns(&a, "T1"), ["A"]);
    assert_eq!(query(&a, "SELECT * FROM T1"), ["1"]);
    a.run("INSERT INTO T1 VALUES (-1)").unwrap();
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.44 SR4, §12.7 GR12z]
/// DROP COLLATION with RESTRICT is refused while a generation expression
/// names the collation; with CASCADE each such generated column is dropped
/// first, and a column that only compares by it otherwise stays.
#[test]
fn collation_dropped_11_44() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           CREATE TABLE T (A VARCHAR(9), G INTEGER GENERATED ALWAYS AS (CASE WHEN A = 'x' COLLATE A.FOLD THEN 1 ELSE 0 END))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (A) VALUES ('X'), ('y')").unwrap();
    assert_eq!(query(&a, "SELECT A, G FROM T ORDER BY A"), ["X,1", "y,0"]);
    a.commit().unwrap();
    let e = a.run("DROP COLLATION A.FOLD RESTRICT").unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "SR4");
    assert!(e.msg.contains("generated column A.T.G"), "{}", e.msg);
    a.rollback().unwrap();
    a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(columns(&a, "T"), ["A"]);
    assert_eq!(query(&a, "SELECT A FROM T ORDER BY A"), ["X", "y"]);
}

/// Schema A holds domain D and collation FOLD, each granted B, and table
/// OWN whose generated column CASTs to D; B has table T, of a row, whose
/// generated columns CAST to D and compare by FOLD.
fn granted(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN D AS INTEGER
           CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD
           CREATE TABLE OWN (X INTEGER, G INTEGER GENERATED ALWAYS AS (CAST(X AS D)))
           GRANT USAGE ON DOMAIN D TO B
           GRANT USAGE ON COLLATION A.FOLD TO B",
    )
    .unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE T (X INTEGER, C VARCHAR(9),
             G INTEGER GENERATED ALWAYS AS (CAST(X AS A.D)),
             H INTEGER GENERATED ALWAYS AS (CASE WHEN C = 'x' COLLATE A.FOLD THEN 1 ELSE 0 END))",
    )
    .unwrap();
    let b = db.session("B");
    b.run("INSERT INTO T (X, C) VALUES (1, 'X')").unwrap();
    b.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR18; ISO/IEC
/// 9075-2:2023 §12.7 GR12h, GR12z] REVOKE USAGE on a domain or collation
/// with RESTRICT is refused while a generation expression of a schema it
/// leaves without USAGE names it; with CASCADE each such generated column
/// is dropped first, its table's rows kept; the owner's own stays.
#[test]
fn usage_revoked_12_7() {
    let db = Database::new();
    granted(&db);
    let a = db.session("A");
    assert_eq!(refused(a.run("REVOKE USAGE ON DOMAIN D FROM B RESTRICT")), Some(Code::SyntaxRule), "SR18, a domain");
    a.rollback().unwrap();
    a.run("REVOKE USAGE ON DOMAIN D FROM B CASCADE").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(columns(&b, "T"), ["C", "H", "X"]);
    assert_eq!(columns(&a, "OWN"), ["G", "X"], "the owner's");
    assert_eq!(refused(a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B RESTRICT")), Some(Code::SyntaxRule), "SR18, a collation");
    a.rollback().unwrap();
    a.run("REVOKE USAGE ON COLLATION A.FOLD FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(columns(&b, "T"), ["C", "X"]);
    assert_eq!(query(&b, "SELECT * FROM T"), ["1,X"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.2 GR1, GR3, GR5; ISO/IEC
/// 9075-2:2023 §12.7 GR12z] DROP SCHEMA with CASCADE drops first the
/// generated columns of other schemas' tables that name its domains or
/// collations; its own tables go with it.
#[test]
fn schema_dropped_11_2() {
    let db = Database::new();
    granted(&db);
    let a = db.session("A");
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(columns(&b, "T"), ["C", "X"]);
    assert_eq!(query(&b, "SELECT * FROM T"), ["1,X"]);
    assert_eq!(columns(&a, "OWN").len(), 0, "gone with its schema");
}

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D63, D25 the generated columns dropped
/// before a domain and a collation, each logged apart from the drop, are
/// gone after a close, a crash, and a crash after a checkpoint, the drops
/// replayed in order.
#[test]
fn recovered() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("generated-named-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        granted(&before);
        if checkpoint {
            before.checkpoint().unwrap();
        }
        let a = before.session("A");
        a.run("DROP DOMAIN D CASCADE").unwrap();
        a.run("DROP COLLATION A.FOLD CASCADE").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let b = db.session("B");
        assert_eq!(columns(&b, "T"), ["C", "X"], "{case}");
        assert_eq!(query(&b, "SELECT * FROM T"), ["1,X"], "{case}");
        b.rollback().unwrap();
        let a = db.session("A");
        assert_eq!(columns(&a, "OWN"), ["X"], "{case}");
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}
