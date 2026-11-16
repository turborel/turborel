//! [ISO/IEC 9075-2:2023 §4.42, §12.4, §12.5] Roles: who creates them, the
//! grants of them, and the privileges a role brings to a cell: to a
//! statement once SET ROLE enables it, and to a definition only if its
//! owner is the role (RULES.md D75).

use turborel::db::{Completion, Database};
use turborel::error::Code;

use crate::common::{exec, failure, rows};

/// Users OWNER, the database's owner, B and C; schema A with table T,
/// holding 1; schemas B and C, empty.
pub fn owned() -> Database {
    let db = Database::new();
    for user in ["OWNER", "B", "C"] {
        db.create_user(user, b"x").unwrap();
    }
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY)").unwrap();
    for auth in ["B", "C"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {auth}")).unwrap();
    }
    committed(&db, "A", &["INSERT INTO T VALUES (1)"]);
    db
}

/// Runs `statements` as `auth`, and commits.
pub fn committed(db: &Database, auth: &str, statements: &[&str]) {
    let s = db.session(auth);
    for sql in statements {
        s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    s.commit().unwrap();
}

/// `sql` run as direct SQL in `s` [ISO/IEC 9075-2:2023 §21.1], as a
/// `<schema definition>` is.
pub fn direct(s: &turborel::db::Session, sql: &str) -> Result<(), Code> {
    s.direct(sql).map(drop).map_err(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.4 AR1, SR1, IW141]; [EXTENSION]
/// ALTER USER ... CREATE ROLE: the database's owner creates a role, and a
/// user it allows, until it is allowed no more; a role's name may be no
/// authorization identifier known already, and no user is created of a
/// role's name. INFORMATION_SCHEMA.USERS tells who may create one.
#[test]
fn the_owner_and_users_it_allows_create_roles_12_4() {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R"]);
    let b = db.session("B");
    assert_eq!(exec(&b, "CREATE ROLE S"), Err(Code::Privilege), "IW141: B is not allowed");
    committed(&db, "OWNER", &["ALTER USER B CREATE ROLE"]);
    assert_eq!(exec(&b, "CREATE ROLE S"), Ok(Completion::Success));
    b.commit().unwrap();
    let o = db.session("OWNER");
    let shown = "SELECT USER_NAME, CREATE_ROLE FROM INFORMATION_SCHEMA.USERS ORDER BY USER_NAME";
    assert_eq!(rows(&o, shown).unwrap(), ["B,YES", "C,NO", "OWNER,YES"]);
    o.commit().unwrap();
    o.run("ALTER USER B NO CREATE ROLE").unwrap();
    assert_eq!(exec(&b, "CREATE ROLE S2"), Err(Code::Privilege), "allowed no more");
    for taken in ["R", "S", "B", "A", "\"_SYSTEM\""] {
        assert_eq!(exec(&o, &format!("CREATE ROLE {taken}")), Err(Code::SyntaxRule), "SR1: {taken} is known");
    }
    assert!(db.create_user("R", b"x").is_err(), "R is a role's name");
    assert_eq!(exec(&o, "CREATE ROLE R2 WITH ADMIN CURRENT_ROLE"), Err(Code::SyntaxRule), "§12.8 SR2c: OWNER has no role");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.5 SR1, GR1, GR2-GR5]: a role is
/// granted by one holding it WITH ADMIN OPTION, else SQLSTATE 0P000; a
/// grant again WITH ADMIN OPTION makes it grantable; no role is granted to
/// itself nor to a role applicable for it, a cycle; a role granted must be
/// one.
#[test]
fn granting_a_role_takes_its_admin_option_12_5() {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R", "CREATE ROLE S", "GRANT R TO B"]);
    let b = db.session("B");
    assert_eq!(failure(b.run("GRANT R TO C")), (Code::Privilege, "0P000".to_owned()), "GR1");
    b.rollback().unwrap();
    committed(&db, "OWNER", &["GRANT R TO B WITH ADMIN OPTION"]);
    assert_eq!(exec(&b, "GRANT R TO C"), Ok(Completion::Success), "GR5: now grantable");
    b.commit().unwrap();
    let o = db.session("OWNER");
    assert_eq!(exec(&o, "GRANT R TO R"), Err(Code::SyntaxRule), "SR1: itself");
    o.run("GRANT S TO R").unwrap();
    assert_eq!(exec(&o, "GRANT R TO S"), Err(Code::SyntaxRule), "SR1: S is applicable for R, a cycle");
    assert_eq!(exec(&o, "GRANT X TO B"), Err(Code::SyntaxRule), "no role X");
    o.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.42.4, §11.1 SR2b, §11.32 AR] T332: a
/// role may own a schema, which a session creates and uses as the role's
/// once SET ROLE enables it; what is defined there is checked with the
/// role's applicable privileges, its roles' among them, while a definition
/// a user owns is not helped by the role the session set.
#[test]
fn a_role_owns_a_schema_and_defines_with_its_roles_12_4() {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R", "CREATE ROLE S", "GRANT S TO R", "GRANT R TO B", "GRANT S TO B"]);
    committed(&db, "A", &["GRANT SELECT ON T TO S"]);
    let b = db.session("B");
    assert_eq!(direct(&b, "CREATE SCHEMA AUTHORIZATION R"), Err(Code::Privilege), "R is not enabled");
    b.rollback().unwrap();
    b.run("SET ROLE 'R'").unwrap();
    direct(&b, "CREATE SCHEMA AUTHORIZATION R").unwrap();
    b.run("CREATE TABLE R.L (K INTEGER)").unwrap();
    b.run("CREATE VIEW R.V AS SELECT K FROM A.T").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&b, "SELECT K FROM R.V").unwrap(), ["1"]);
    b.commit().unwrap();
    b.run("SET ROLE 'S'").unwrap();
    assert_eq!(exec(&b, "CREATE VIEW B.V AS SELECT K FROM A.T"), Err(Code::Privilege), "B's own view: S's privileges are not B's");
    assert_eq!(rows(&b, "SELECT K FROM A.T").unwrap(), ["1"], "the statement's are");
    b.rollback().unwrap();
    let c = db.session("C");
    assert_eq!(exec(&c, "INSERT INTO R.L VALUES (1)"), Err(Code::Privilege));
}
