//! Schemas named apart from their owners [ISO/IEC 9075:1992 §11.1]: what
//! is in one is its owner's, its names the schema's; the owner is the
//! AUTHORIZATION written, or the session's; and it is kept by the log and
//! by a checkpoint.

use std::path::Path;

use turborel::db::{Database, Direct, OpenOptions};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

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
    assert_eq!(rows(&u, schemata), Ok(vec!["S,U".to_owned()]));
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

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR9c, SR10, SR20, §6.4,
/// §7.9]: a name may be qualified by its catalog, the one there is, named
/// by the setting CATALOG_NAME: a table's, a column reference's
/// qualifier's, a `<qualifier>.*`'s, in a statement run, prepared, a
/// cursor declared and opened, explained, or a schema defined; it names
/// what the name without it does. Another catalog's name is refused
/// (SQLCODE -103; in direct SQL, SQLSTATE 2A000 [§20.1 GR5]). A definition read again keeps the
/// names it was checked with, though the catalog is renamed since.
#[test]
fn a_name_is_qualified_by_the_one_catalog() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA S AUTHORIZATION U CREATE TABLE TURBOREL.S.T (K INTEGER) CREATE VIEW V AS SELECT TURBOREL.S.T.K FROM TURBOREL.S.T",
    )
    .unwrap();
    let s = db.session("U");
    exec(&s, "INSERT INTO TURBOREL.S.T VALUES (1)").unwrap();
    for sql in ["SELECT TURBOREL.S.T.K FROM TURBOREL.S.T", "SELECT TURBOREL.S.T.* FROM S.T", "SELECT K FROM S.V"] {
        assert_eq!(rows(&s, sql), Ok(vec!["1".into()]), "{sql}");
    }
    assert!(s.prepare("SELECT K FROM TURBOREL.S.T WHERE K = ?").is_ok());
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM TURBOREL.S.T").unwrap();
    assert!(s.explain("SELECT K FROM TURBOREL.S.T", &Params::new()).is_ok());
    for wrong in [
        "SELECT K FROM OTHER.S.T",
        "SELECT OTHER.S.T.K FROM S.T",
        "SELECT OTHER.S.T.* FROM S.T",
        "DROP TABLE OTHER.S.T CASCADE",
        "CREATE VIEW OTHER.S.W AS SELECT K FROM S.T",
    ] {
        let failed = if wrong.starts_with("SELECT") { Err(code(&s, wrong)) } else { exec(&s, wrong).map(drop) };
        assert_eq!(failed, Err(Code::SyntaxRule), "{wrong}");
    }
    assert_eq!(s.prepare("SELECT K FROM OTHER.S.T").err().map(|e| e.code), Some(Code::SyntaxRule));
    exec(&s, "OPEN C").unwrap();
    // A declared cursor's query is read whole as it is opened.
    s.declare_cursor("DECLARE D CURSOR FOR SELECT K FROM OTHER.S.T").unwrap();
    assert_eq!(exec(&s, "OPEN D"), Err(Code::SyntaxRule));
    assert_eq!(
        db.schema("CREATE SCHEMA R AUTHORIZATION U CREATE TABLE OTHER.R.T (K INTEGER)").err().map(|e| e.code),
        Some(Code::SyntaxRule)
    );
    s.commit().unwrap();
    db.set_setting("CATALOG_NAME", &Value::Char("SHOP".into())).unwrap();
    let renamed = db.session("U");
    assert_eq!(rows(&renamed, "SELECT K FROM S.V"), Ok(vec!["1".into()]), "the view as it was checked");
    assert_eq!(rows(&renamed, "SELECT K FROM SHOP.S.T"), Ok(vec!["1".into()]));
    assert_eq!(code(&renamed, "SELECT K FROM TURBOREL.S.T"), Code::SyntaxRule, "renamed");
}

/// Schemas S1 and S2 of U: each a table R keyed by K, S1's holding 1 and
/// 2, S2's 9; S1 a domain POS of positive integers. Then, its default
/// schema S1, U defines in S2: T, its CHECK, its foreign key and its
/// column's domain naming S1's; V over S1's R; a domain SMALL of S1's R's
/// keys; an assertion that S1's R has no negative key; and alters T, a
/// CHECK and a column added naming S1's.
fn defined_elsewhere(db: &Database) {
    db.schema("CREATE SCHEMA S1 AUTHORIZATION U CREATE TABLE R (K INTEGER UNIQUE) CREATE DOMAIN POS AS INTEGER CHECK (VALUE > 0)").unwrap();
    db.schema("CREATE SCHEMA S2 AUTHORIZATION U CREATE TABLE R (K INTEGER UNIQUE)").unwrap();
    let u = db.session("U");
    for sql in [
        "INSERT INTO S1.R VALUES (1)",
        "INSERT INTO S1.R VALUES (2)",
        "INSERT INTO S2.R VALUES (9)",
        "SET SCHEMA 'S1'",
        "CREATE TABLE S2.T (K INTEGER CHECK (K IN (SELECT K FROM R)), P POS, FOREIGN KEY (K) REFERENCES R (K))",
        "CREATE VIEW S2.V AS SELECT K FROM R",
        "CREATE DOMAIN S2.SMALL AS INTEGER CHECK (VALUE IN (SELECT K FROM R))",
        "CREATE ASSERTION S2.NONE_NEGATIVE CHECK (NOT EXISTS (SELECT * FROM R WHERE K < 0))",
        "ALTER TABLE S2.T ADD CHECK (P IN (SELECT K FROM R))",
        "ALTER TABLE S2.T ADD COLUMN Q S2.SMALL",
        "INSERT INTO S2.T VALUES (1, 1, 1)",
    ] {
        exec(&u, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    u.commit().unwrap();
}

/// That what `defined_elsewhere` defined names S1's, though read with S2
/// the default schema.
fn names_kept(db: &Database) {
    let u = db.session("U");
    exec(&u, "SET SCHEMA 'S2'").unwrap();
    assert_eq!(rows(&u, "SELECT K FROM V ORDER BY K"), Ok(vec!["1".into(), "2".into()]), "S1's R");
    for (sql, why) in [
        ("INSERT INTO T VALUES (9, 1, 1)", "K's CHECK and foreign key"),
        ("INSERT INTO T VALUES (2, 9, 1)", "the CHECK added"),
        ("INSERT INTO T VALUES (2, 0, 1)", "POS"),
        ("INSERT INTO T VALUES (2, 2, 9)", "SMALL"),
        ("INSERT INTO S1.R VALUES (-1)", "the assertion"),
    ] {
        assert_eq!(exec(&u, sql).err(), Some(Code::Constraint), "{why}");
    }
    exec(&u, "INSERT INTO T VALUES (2, 2, 2)").unwrap();
    exec(&u, "INSERT INTO R VALUES (-1)").unwrap();
    u.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR8b, §11.3, §11.10,
/// §11.19, §11.21, §11.34]; [DESIGN] RULES.md D47: a table, view, domain or
/// assertion is defined in any schema its definer owns, and a table
/// altered there; the names in it written without a schema are of the
/// session's default schema, and stay so, whatever the default schema
/// later, after recovery, and after a checkpoint.
#[test]
fn a_definition_in_another_schema_keeps_its_names() {
    let db = Database::new();
    defined_elsewhere(&db);
    names_kept(&db);
    let scratch = Scratch::new("named-elsewhere");
    let dir = scratch.path("db");
    let kept = open(&dir);
    defined_elsewhere(&kept);
    drop(kept);
    let recovered = open(&dir);
    names_kept(&recovered);
    recovered.close().unwrap();
    drop(recovered);
    names_kept(&open(&dir));
}
