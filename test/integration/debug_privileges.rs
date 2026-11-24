//! \[EXTENSION\] The DEBUG privilege on SQL-invoked routines (RULES.md
//! D1000 to D1002): granted and revoked as EXECUTE is [ISO/IEC 9075-2:2023
//! §12.2, §12.7], held by the owner by no grant, shown in
//! ROUTINE_PRIVILEGES, and kept across reopening.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::Scratch;

/// Schema A, owned by A: T (K) and procedure P, function F.
fn owned() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("CREATE PROCEDURE P (IN X INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (X)").unwrap();
    a.run("CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 1").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Each DEBUG on routines of schema A: routine, grantee, grantor and
/// grantability, in order.
fn debugs(s: &Session) -> Vec<String> {
    rows(
        s,
        "SELECT ROUTINE_NAME, GRANTEE, GRANTOR, IS_GRANTABLE FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES \
         WHERE ROUTINE_SCHEMA = 'A' AND PRIVILEGE_TYPE = 'DEBUG' ORDER BY ROUTINE_NAME, GRANTEE, GRANTOR",
    )
}

/// Verifies: [EXTENSION] RULES.md D1000: GRANT DEBUG on a procedure, a
/// function or a routine, by its name, its parameters' types or its
/// specific name, is shown in ROUTINE_PRIVILEGES as DEBUG, and REVOKE takes
/// it away. MC/DC of the routine type: PROCEDURE names P and not F.
#[test]
fn debug_is_granted_and_revoked() {
    let db = owned();
    let a = db.session("A");
    assert_eq!(a.run("GRANT DEBUG ON PROCEDURE F TO B").unwrap_err().code, Code::SyntaxRule, "F is no procedure");
    a.run("GRANT DEBUG ON PROCEDURE P TO B").unwrap();
    a.run("GRANT DEBUG ON FUNCTION F (INTEGER) TO B").unwrap();
    a.run("GRANT DEBUG ON ROUTINE F TO PUBLIC").unwrap();
    a.commit().unwrap();
    assert_eq!(debugs(&a), ["F,B,A,NO", "F,PUBLIC,A,NO", "P,B,A,NO"]);
    a.run("REVOKE DEBUG ON PROCEDURE P FROM B CASCADE").unwrap();
    a.run("REVOKE DEBUG ON FUNCTION F FROM B, PUBLIC RESTRICT").unwrap();
    a.commit().unwrap();
    assert!(debugs(&a).is_empty(), "all revoked");
    let specific = rows(&a, "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'P'").concat();
    a.run(&format!("GRANT DEBUG, EXECUTE ON SPECIFIC PROCEDURE {specific} TO C")).unwrap();
    a.commit().unwrap();
    assert_eq!(debugs(&a), ["P,C,A,NO"], "by its specific name, with EXECUTE");
    let execute = "SELECT GRANTEE FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE PRIVILEGE_TYPE = 'EXECUTE' AND GRANTEE = 'C'";
    assert_eq!(rows(&a, execute), ["C"], "EXECUTE granted beside it");
}

/// Verifies: [EXTENSION] RULES.md D1002: ALL PRIVILEGES on a routine is
/// EXECUTE alone, the standard's privileges, granting and revoking no
/// DEBUG.
#[test]
fn all_privileges_leaves_debug() {
    let db = owned();
    let a = db.session("A");
    a.run("GRANT DEBUG ON PROCEDURE P TO B").unwrap();
    a.run("GRANT ALL PRIVILEGES ON PROCEDURE P TO C").unwrap();
    a.run("REVOKE ALL PRIVILEGES ON PROCEDURE P FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(debugs(&a), ["P,B,A,NO"]);
}

/// Verifies: [EXTENSION] RULES.md D1001: the owner holds DEBUG by no grant,
/// grantably; one without it may not grant it (SQLCODE -104); one granted
/// it without the grant option is warned (SQLSTATE 01007) and grants
/// nothing; one with it grants it on.
#[test]
fn debug_is_granted_on_by_its_grant_option() {
    let db = owned();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    assert!(debugs(&a).is_empty(), "the owner's is no grant");
    assert_eq!(b.run("GRANT DEBUG ON PROCEDURE A.P TO C").unwrap_err().code, Code::Privilege, "B holds none");
    b.rollback().unwrap();
    a.run("GRANT DEBUG ON PROCEDURE P TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(b.run("GRANT DEBUG ON PROCEDURE A.P TO C").unwrap().sqlstate().as_str(), "01007", "not grantable");
    b.commit().unwrap();
    a.run("GRANT DEBUG ON PROCEDURE P TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT DEBUG ON PROCEDURE A.P TO C").unwrap();
    b.commit().unwrap();
    assert_eq!(debugs(&c), ["P,C,B,NO"], "C sees its own grant");
    assert_eq!(debugs(&a), ["P,B,A,YES"], "A sees its own grant, not B's");
}

/// Verifies: [EXTENSION] RULES.md D1000: REVOKE DEBUG with RESTRICT of a
/// grant another was granted on by is refused (SQLSTATE 2B000); with
/// CASCADE both go; GRANT OPTION FOR takes the grant option alone and the
/// grants made by it; revoking what was not granted warns (01006).
#[test]
fn debug_revokes_cascade() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT DEBUG ON PROCEDURE P TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT DEBUG ON PROCEDURE A.P TO C").unwrap();
    b.commit().unwrap();
    let restricted = a.run("REVOKE DEBUG ON PROCEDURE P FROM B RESTRICT").unwrap_err();
    assert_eq!(restricted.sqlstate().as_str(), "2B000");
    assert!(restricted.to_string().contains("revoking DEBUG"), "{restricted}");
    a.run("REVOKE GRANT OPTION FOR DEBUG ON PROCEDURE P FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(debugs(&a), ["P,B,A,NO"], "C's grant went with B's option");
    a.run("REVOKE DEBUG ON PROCEDURE P FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(debugs(&a), Vec::<String>::new());
    assert_eq!(a.run("REVOKE DEBUG ON PROCEDURE P FROM B CASCADE").unwrap().sqlstate().as_str(), "01006");
}

/// Verifies: [EXTENSION] RULES.md D1000: DEBUG is a routine's privilege
/// alone: of a table, a module's routine or a module it is refused
/// (SQLCODE -103), and USAGE is refused of a routine.
#[test]
fn debug_is_of_routines_alone() {
    let db = owned();
    let a = db.session("A");
    for refused in [
        "GRANT DEBUG ON TABLE T TO B",
        "GRANT DEBUG ON T TO B",
        "GRANT USAGE ON PROCEDURE P TO B",
        "GRANT DEBUG ON MODULE SHOP TO B",
        "GRANT DEBUG ON FUNCTION SHOP_F TO B",
    ] {
        if refused.contains("SHOP") {
            a.run("CREATE MODULE SHOP FUNCTION SHOP_F () RETURNS INTEGER CONTAINS SQL RETURN 1; END MODULE").ok();
            a.commit().unwrap();
        }
        assert_eq!(a.run(refused).unwrap_err().code, Code::SyntaxRule, "{refused}");
    }
}

/// Verifies: [EXTENSION] RULES.md D1000: a grant of DEBUG is kept across
/// reopening, by its logged statement without a checkpoint and by the
/// snapshot after one.
#[test]
fn debug_is_kept_across_reopening() {
    let scratch = Scratch::new("debug-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
        let a = db.session("A");
        a.run("CREATE PROCEDURE P (IN X INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (X)").unwrap();
        a.run("GRANT DEBUG ON PROCEDURE P TO B WITH GRANT OPTION").unwrap();
        a.commit().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        let a = db.session("A");
        assert_eq!(debugs(&a), ["P,B,A,YES"], "by the log");
        a.run("GRANT DEBUG ON PROCEDURE P TO C").unwrap();
        a.commit().unwrap();
        db.checkpoint().unwrap();
        drop(a);
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    assert_eq!(debugs(&a), ["P,B,A,YES", "P,C,A,NO"], "by the snapshot");
    drop(a);
    db.close().unwrap();
}
