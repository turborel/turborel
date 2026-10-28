//! Schemas named apart from their owners [ISO/IEC 9075:1992 §11.1]: what
//! is in one is its owner's, its names the schema's; the owner is the
//! AUTHORIZATION written, or the session's; and it is kept by the log and
//! by a checkpoint.

use std::path::Path;

use turborel::db::{Database, Direct, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, count, exec, os, rows};

/// Schema S, owned by U: T, keyed by K; C, whose K references T's and is
/// checked to be in T; V over T; P may read T.
fn named(db: &Database) {
    db.schema(
        "CREATE SCHEMA S AUTHORIZATION U
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE)
           CREATE TABLE C (K INTEGER REFERENCES T (K), CHECK (K IN (SELECT K FROM T)))
           CREATE VIEW V AS SELECT K FROM T WHERE K > 1
           GRANT SELECT ON T TO P",
    )
    .unwrap();
}

/// The completion of `sql` given to `s` as direct SQL, or its SQLCODE.
fn direct(s: &turborel::db::Session, sql: &str) -> Result<(), Code> {
    match s.direct(sql) {
        Ok(Direct::Done(_)) => Ok(()),
        Ok(Direct::Rows(_)) => panic!("{sql} gave rows"),
        Err(e) => Err(e.code),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.1 GR2, GR3; §5.4 SR] a
/// schema's tables and views are named by the schema, its unqualified
/// names inside it being of it: a view, a foreign key and a CHECK of the
/// schema read its own tables; its owner holds every privilege on them,
/// changes and drops them, defines more in it under SET SCHEMA, and
/// grants on them; another holds what is granted, and no more; the
/// Information Schema gives the schema with its owner.
#[test]
fn what_is_in_a_schema_is_its_owners_11_1() {
    let db = Database::new();
    named(&db);
    let u = db.session("U");
    u.run("INSERT INTO S.T VALUES (1), (2), (3)").unwrap();
    u.run("INSERT INTO S.C VALUES (2)").unwrap();
    assert_eq!(exec(&u, "INSERT INTO S.C VALUES (9)").err(), Some(Code::Constraint), "9 is not in S.T");
    assert_eq!(count(&u, "SELECT K FROM S.V"), 2);
    u.run("SET SCHEMA 'S'").unwrap();
    u.run("CREATE TABLE N (K INTEGER)").unwrap();
    u.run("ALTER TABLE T ADD COLUMN M INTEGER").unwrap();
    u.run("DROP VIEW V RESTRICT").unwrap();
    u.commit().unwrap();
    let p = db.session("P");
    assert_eq!(count(&p, "SELECT K FROM S.T"), 3, "granted");
    assert_eq!(exec(&p, "INSERT INTO S.T VALUES (4, 4)").err(), Some(Code::Privilege), "not granted");
    for of_s in ["DROP TABLE S.N CASCADE", "ALTER TABLE S.N ADD COLUMN L INTEGER", "CREATE INDEX S_N ON S.N (K)"] {
        assert_eq!(exec(&p, of_s).err(), Some(Code::Privilege), "{of_s}");
        p.rollback().unwrap();
    }
    let schemata = "SELECT SCHEMA_NAME, SCHEMA_OWNER FROM INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = 'S'";
    assert_eq!(rows(&u, schemata), Ok(vec![format!("{:<128},{:<128}", "S", "U")]));
    u.run("DROP SCHEMA S CASCADE").unwrap();
    u.commit().unwrap();
    assert_eq!(code(&p, "SELECT K FROM S.T"), Code::SyntaxRule, "dropped by its owner");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.1 GR2] a schema named after
/// one identifier and owned by another is the owner's: the identifier it
/// is named after holds nothing on it, and cannot drop it.
#[test]
fn a_schema_is_its_owners_not_its_names() {
    let db = Database::new();
    db.schema("CREATE SCHEMA BOB AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    let bob = db.session("BOB");
    assert_eq!(exec(&bob, "INSERT INTO T VALUES (1)").err(), Some(Code::Privilege));
    assert_eq!(exec(&bob, "DROP SCHEMA BOB CASCADE").err(), Some(Code::Privilege));
    let ann = db.session("ANN");
    ann.run("INSERT INTO BOB.T VALUES (1)").unwrap();
    ann.commit().unwrap();
    assert_eq!(count(&ann, "SELECT K FROM BOB.T"), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.1 SR1–SR3, AR1] a
/// `<schema name clause>` may name the schema, its owner, or both: through
/// a session, with no AUTHORIZATION, the session's identifier owns it
/// [SR2b], and naming another is refused (SQLCODE -104); outside one,
/// `Database::schema` needs AUTHORIZATION (SQLCODE -103). The catalog's
/// own name may qualify the schema's, another is refused (SQLSTATE
/// 3D000); a name taken is refused [SR3].
#[test]
fn whose_a_schema_is_11_1() {
    let db = Database::new();
    let u = db.session("U");
    direct(&u, "CREATE SCHEMA W CREATE TABLE T (K INTEGER)").unwrap();
    u.run("INSERT INTO W.T VALUES (1)").unwrap();
    u.commit().unwrap();
    assert_eq!(direct(&u, "CREATE SCHEMA X AUTHORIZATION ANN"), Err(Code::Privilege), "another's");
    assert_eq!(db.schema("CREATE SCHEMA X").err().map(|e| e.code), Some(Code::SyntaxRule), "no session's to imply");
    db.schema("CREATE SCHEMA TURBOREL.X AUTHORIZATION U").unwrap();
    let elsewhere = db.schema("CREATE SCHEMA ELSEWHERE.Y AUTHORIZATION U").unwrap_err();
    assert_eq!((elsewhere.code, elsewhere.sqlstate().as_str()), (Code::SyntaxRule, "3D000"));
    assert_eq!(db.schema("CREATE SCHEMA W AUTHORIZATION ANN").err().map(|e| e.code), Some(Code::SyntaxRule), "SR3");
    assert_eq!(db.schema("CREATE SCHEMA").err().map(|e| e.code), Some(Code::Syntax));
}

/// The database in `dir`.
fn open(dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// That schema W of `owners_are_logged` is U's: U writes it, ANN may not.
fn owned_by_u(db: &Database) {
    let u = db.session("U");
    u.run("INSERT INTO W.T VALUES (2)").unwrap();
    u.rollback().unwrap();
    assert_eq!(exec(&db.session("ANN"), "INSERT INTO W.T VALUES (2)").err(), Some(Code::Privilege));
}

/// Verifies: [DESIGN] RULES.md D25: a schema's owner, implicit or written,
/// is logged with it, so recovery makes it again with the same owner, and
/// a checkpoint keeps it.
#[test]
fn owners_are_logged() {
    let scratch = Scratch::new("named-schemas");
    let dir = scratch.path("db");
    let db = open(&dir);
    direct(&db.session("U"), "CREATE SCHEMA W CREATE TABLE T (K INTEGER)").unwrap();
    drop(db);
    let db = open(&dir);
    owned_by_u(&db);
    db.close().unwrap();
    drop(db);
    let db = open(&dir);
    owned_by_u(&db);
}
