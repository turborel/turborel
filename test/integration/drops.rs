//! \[EXTENSION\] DROP VIEW [ISO/IEC 9075:1992 §11.20] and DROP SCHEMA
//! [§11.2] in transactions (RULES.md D33): what each drops with what it
//! names, what it refuses, the locks it holds, and its change logged.

use std::path::Path;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, count, exec, os, rows, with_views};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.20 SR3, GR1, GR2] RESTRICT
/// refuses a view another view reads, and drops one none does; CASCADE
/// drops it with the views that read it, directly or through another, and
/// the privileges on them; the table under it stays, with the privileges
/// on it; the drop is its transaction's until it commits, and a ROLLBACK
/// brings the view back.
#[test]
fn a_view_dropped_11_20() {
    let db = with_views();
    let (s, other) = (db.session("D"), db.session("D"));
    assert_eq!(exec(&s, "DROP VIEW V RESTRICT").err(), Some(Code::SyntaxRule), "W reads V");
    s.run("DROP VIEW S RESTRICT").unwrap();
    assert_eq!(code(&s, "SELECT K FROM S"), Code::SyntaxRule, "gone for its transaction");
    assert_eq!(count(&other, "SELECT K FROM S"), 3, "not for another");
    other.rollback().unwrap();
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT K FROM S"), 3, "rolled back");
    s.run("DROP VIEW V CASCADE").unwrap();
    s.commit().unwrap();
    for gone in ["SELECT K FROM V", "SELECT K FROM W"] {
        assert_eq!(code(&other, gone), Code::SyntaxRule, "{gone}");
    }
    assert_eq!(count(&other, "SELECT K FROM T"), 3, "T stays");
    let p = db.session("P");
    assert_eq!(count(&p, "SELECT K FROM D.T"), 3, "with P's privilege on it");
    assert_eq!(code(&p, "SELECT K FROM D.V"), Code::SyntaxRule);
    let privileges = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE GRANTEE = 'P' ORDER BY TABLE_NAME";
    assert_eq!(rows(&p, privileges), Ok(vec![format!("{:<128}", "T")]), "those on V gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.20 SR1, SR2, AR1, Format]
/// refused, the transaction going on: a view not defined, a base table,
/// another owner's view, and a DROP VIEW without its behavior.
#[test]
fn what_drop_view_refuses_11_20() {
    let db = with_views();
    let s = db.session("D");
    assert_eq!(exec(&s, "DROP VIEW NOPE CASCADE").err(), Some(Code::SyntaxRule), "SR1");
    assert_eq!(exec(&s, "DROP VIEW T CASCADE").err(), Some(Code::SyntaxRule), "SR2");
    assert_eq!(exec(&s, "DROP VIEW W").err(), Some(Code::Syntax), "no <drop behavior>");
    assert_eq!(exec(&db.session("P"), "DROP VIEW D.V CASCADE").err(), Some(Code::Privilege), "AR1");
    s.commit().unwrap();
    assert_eq!(count(&s, "SELECT K FROM W"), 2);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.2 SR3, GR1, GR2, GR8]
/// RESTRICT refuses a schema that holds a table or view, and drops an
/// empty one; CASCADE drops each of its tables, with their indexes, and
/// its views, and what depends on them in another schema: a view and a
/// foreign key; the schema is gone, and may be created again, its tables
/// new.
#[test]
fn a_schema_dropped_11_2() {
    let db = with_views();
    let s = db.session("D");
    s.run("GRANT REFERENCES ON T TO P").unwrap();
    s.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION P CREATE TABLE F (K INTEGER REFERENCES D.T (K)) CREATE VIEW Q AS SELECT K FROM D.V").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION E").unwrap();
    assert_eq!(exec(&s, "DROP SCHEMA D RESTRICT").err(), Some(Code::SyntaxRule), "it holds T");
    let e = db.session("E");
    let schemata = "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = 'E'";
    assert_eq!(rows(&e, schemata), Ok(vec![format!("{:<128}", "E")]));
    e.run("DROP SCHEMA E RESTRICT").unwrap();
    e.commit().unwrap();
    assert_eq!(rows(&e, schemata), Ok(vec![]));
    s.run("DROP SCHEMA D CASCADE").unwrap();
    s.commit().unwrap();
    let p = db.session("P");
    for gone in ["SELECT K FROM D.T", "SELECT K FROM D.U", "SELECT K FROM D.X", "SELECT K FROM Q"] {
        assert_eq!(code(&p, gone), Code::SyntaxRule, "{gone}");
    }
    p.run("INSERT INTO F VALUES (99)").unwrap();
    assert_eq!(db.index_entries("T_N").map_err(|e| e.code), Err(Code::Api), "its index gone");
    assert_eq!(exec(&s, "CREATE TABLE N (K INTEGER)").err(), Some(Code::SyntaxRule), "no schema D");
    s.rollback().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION D CREATE TABLE T (K INTEGER)").unwrap();
    assert_eq!(count(&s, "SELECT K FROM T"), 0, "a new T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.2 SR2, AR1, Format]
/// refused: a schema that is not (SQLSTATE 3F000), one named in another
/// catalog, another's schema, and a DROP SCHEMA without its behavior; the
/// catalog's own name is taken.
#[test]
fn what_drop_schema_refuses_11_2() {
    let db = with_views();
    db.schema("CREATE SCHEMA AUTHORIZATION E").unwrap();
    let nope = db.session("NOPE").run("DROP SCHEMA NOPE CASCADE").unwrap_err();
    assert_eq!((nope.code, nope.sqlstate().as_str()), (Code::SyntaxRule, "3F000"), "SR2");
    let e = db.session("E");
    assert_eq!(exec(&e, "DROP SCHEMA ELSEWHERE.E CASCADE").err(), Some(Code::SyntaxRule), "SR2: another catalog");
    assert_eq!(exec(&e, "DROP SCHEMA D CASCADE").err(), Some(Code::Privilege), "AR1");
    assert_eq!(exec(&e, "DROP SCHEMA E").err(), Some(Code::Syntax), "no <drop behavior>");
    e.run("DROP SCHEMA TURBOREL.E RESTRICT").unwrap();
    e.commit().unwrap();
    assert_eq!(count(&db.session("D"), "SELECT K FROM T"), 3);
}

/// Verifies: [DESIGN] RULES.md D33, D47: a drop holds the locks of what it
/// takes away, so none of it changes meanwhile (no-wait: SQLCODE -911): no
/// table is defined in a schema being dropped, nor a view over a view a
/// table's CASCADE drops; and a view defined over another keeps that one
/// from being dropped until its transaction ends.
#[test]
fn a_drop_holds_what_it_takes_away() {
    let db = with_views();
    db.schema("CREATE SCHEMA AUTHORIZATION P").unwrap();
    let (s, mine, p) = (db.session("D"), db.session("D"), db.session("P"));
    s.run("DROP SCHEMA D CASCADE").unwrap();
    let waited = mine.run("CREATE TABLE N (K INTEGER)").unwrap_err();
    assert_eq!(waited.code, Code::Conflict, "{waited:?}");
    assert!(waited.msg.contains("schema D"), "{}", waited.msg);
    mine.rollback().unwrap();
    s.rollback().unwrap();
    s.run("DROP TABLE T CASCADE").unwrap();
    assert_eq!(exec(&p, "CREATE VIEW Q AS SELECT K FROM D.V").err(), Some(Code::Conflict), "V goes with T");
    p.rollback().unwrap();
    s.rollback().unwrap();
    p.run("CREATE VIEW Q AS SELECT K FROM D.V").unwrap();
    assert_eq!(exec(&s, "DROP VIEW V CASCADE").err(), Some(Code::Conflict), "Q is being defined over V");
    p.commit().unwrap();
    s.rollback().unwrap();
    s.run("DROP VIEW V CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(code(&p, "SELECT K FROM Q"), Code::SyntaxRule, "Q went with V");
}

/// The database in `dir`.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// What the drops of `drops_are_logged` left: W gone, V there, and schema
/// E gone.
fn after_drops(db: &Database) {
    let d = db.session("D");
    assert_eq!(code(&d, "SELECT K FROM W"), Code::SyntaxRule);
    assert_eq!(count(&d, "SELECT K FROM V"), 2);
    assert_eq!(exec(&db.session("E"), "CREATE TABLE N (K INTEGER)").err(), Some(Code::SyntaxRule), "no schema E");
}

/// Verifies: [DESIGN] RULES.md D25, D33: the drops of a view and of a
/// schema are logged: recovered when the database stopped without closing,
/// and kept by the checkpoint of a clean close.
#[test]
fn drops_are_logged() {
    let scratch = Scratch::new("drops");
    let dir = scratch.path("db");
    let db = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION D CREATE TABLE T (K INTEGER) CREATE VIEW V AS SELECT K FROM T WHERE K > 1 CREATE VIEW W AS SELECT K FROM V")
        .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION E").unwrap();
    let (d, e) = (db.session("D"), db.session("E"));
    d.run("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    d.run("DROP VIEW W RESTRICT").unwrap();
    d.commit().unwrap();
    e.run("DROP SCHEMA E CASCADE").unwrap();
    e.commit().unwrap();
    drop((d, e, db));
    let db = open(&dir);
    after_drops(&db);
    db.close().unwrap();
    drop(db);
    let db = open(&dir);
    after_drops(&db);
    db.schema("CREATE SCHEMA AUTHORIZATION E").unwrap();
}
