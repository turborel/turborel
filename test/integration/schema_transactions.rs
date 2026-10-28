//! A `<schema definition>` a session runs [ISO/IEC 9075:1992 §11.1] is
//! part of its transaction [§20.1 GR6b ii, iii]: seen by the session at
//! once, by others once it commits, undone by ROLLBACK; its name held until
//! then; and logged so, recovered only if it committed.

use std::path::Path;

use turborel::db::{Completion, Database, Direct, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, after_crash, code, count, exec, rows};

/// What `s` running `sql` directly completes with, or the code of its error.
fn direct(s: &Session, sql: &str) -> Result<Completion, Code> {
    match s.direct(sql).map_err(|e| e.code)? {
        Direct::Done(done) => Ok(done),
        Direct::Rows(_) => Err(Code::Api),
    }
}

const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER NOT NULL UNIQUE) CREATE VIEW V AS SELECT K FROM T";

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR6b ii, iii] a schema a
/// session creates, with its tables and views, is its transaction's: the
/// session reads and writes them at once, after another transaction's
/// definitions committed too; another session finds none of
/// it until the transaction commits; ROLLBACK undoes it, so the schema may
/// be created again.
#[test]
fn a_schema_is_its_transactions() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let (ann, other) = (db.session("ANN"), db.session("ANN"));
    assert_eq!(direct(&ann, SCHEMA), Ok(Completion::Success));
    ann.run("INSERT INTO T VALUES (1), (2)").unwrap();
    assert_eq!(count(&ann, "SELECT K FROM V"), 2, "its own transaction sees it");
    let b = db.session("B");
    b.run("CREATE TABLE Z (K INTEGER)").unwrap();
    b.commit().unwrap();
    assert_eq!(count(&ann, "SELECT K FROM V"), 2, "made again on newer definitions, its rows kept");
    assert_eq!(code(&other, "SELECT K FROM T"), Code::SyntaxRule, "not yet another session's");
    ann.rollback().unwrap();
    assert_eq!(code(&ann, "SELECT K FROM T"), Code::SyntaxRule, "undone");
    assert_eq!(direct(&ann, SCHEMA), Ok(Completion::Success), "created again");
    ann.run("INSERT INTO T VALUES (3)").unwrap();
    ann.commit().unwrap();
    // A transaction begun before sees the tables created after no more.
    assert_eq!(code(&other, "SELECT K FROM V"), Code::Conflict);
    other.rollback().unwrap();
    assert_eq!(rows(&other, "SELECT K FROM V"), Ok(vec!["3".to_owned()]), "committed, with its rows");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR6b ii] a schema's name
/// is held by the transaction creating it until it ends: another session
/// creating one of the name, or the operator, is refused (no wait, the
/// default), and after a ROLLBACK may; a schema the transaction's schema
/// definition fails to make leaves its earlier work, and no schema.
#[test]
fn a_schema_name_is_held_until_the_transaction_ends() {
    let db = Database::new();
    let (ann, other) = (db.session("ANN"), db.session("ANN"));
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER)").unwrap();
    assert_eq!(direct(&ann, SCHEMA), Ok(Completion::Success));
    assert_eq!(direct(&other, SCHEMA), Err(Code::Conflict), "the name is held");
    assert_eq!(db.schema(SCHEMA).err().map(|e| e.code), Some(Code::Conflict), "from the operator too");
    ann.rollback().unwrap();
    assert_eq!(direct(&other, SCHEMA), Ok(Completion::Success), "released");
    other.run("INSERT INTO T VALUES (1)").unwrap();
    let failing = "CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE W (K INTEGER) CREATE VIEW X AS SELECT NOPE FROM W";
    assert_eq!(direct(&other, failing), Err(Code::SyntaxRule));
    assert_eq!(count(&other, "SELECT K FROM T"), 1, "its earlier work stays");
    other.commit().unwrap();
    assert_eq!(code(&db.session("BOB"), "SELECT K FROM ANN.W"), Code::SyntaxRule, "the failed one made nothing");
    assert_eq!(exec(&db.session("B"), "INSERT INTO U VALUES (1)"), Ok(Completion::Success), "another schema is not held");
}

/// The database in `dir`, opened as after a crash of any that has it open.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1 GR6b ii] RULES.md D25: a
/// session's schema is logged with its transaction: when the database
/// stopped without closing, a committed one is recovered with the rows
/// the transaction wrote in it, and one whose transaction did not commit
/// is not; the checkpoint of a clean close keeps the committed one.
#[test]
fn a_schema_is_recovered_if_its_transaction_committed() {
    let scratch = Scratch::new("schema-transactions");
    let dir = scratch.path("db");
    let crashed = open(&dir);
    let (ann, bob) = (crashed.session("ANN"), crashed.session("BOB"));
    direct(&ann, SCHEMA).unwrap();
    ann.run("INSERT INTO T VALUES (1), (2)").unwrap();
    ann.commit().unwrap();
    direct(&bob, "CREATE SCHEMA AUTHORIZATION BOB CREATE TABLE U (K INTEGER)").unwrap();
    bob.run("INSERT INTO U VALUES (1)").unwrap();
    // BOB's transaction is open at the crash.
    let db = open(&dir);
    assert_eq!(count(&db.session("ANN"), "SELECT K FROM V"), 2, "committed, recovered with its rows");
    assert_eq!(code(&db.session("BOB"), "SELECT K FROM U"), Code::SyntaxRule, "not committed, not recovered");
    db.close().unwrap();
    drop(db);
    let db = open(&dir);
    assert_eq!(count(&db.session("ANN"), "SELECT K FROM T"), 2, "kept by the checkpoint");
    assert_eq!(direct(&db.session("BOB"), "CREATE SCHEMA AUTHORIZATION BOB"), Ok(Completion::Success));
    drop((db, ann, bob, crashed));
}
