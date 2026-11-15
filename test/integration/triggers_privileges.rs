//! The TRIGGER privilege (Feature T217) and the privileges a trigger is
//! defined and executed with [ISO/IEC 9075-2:2023 §11.49 AR1 to AR3,
//! §12.3, §12.7, §15.21 GR6b].

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N) and L, a log; schema B, owned by B.
fn owned() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER) CREATE TABLE L (WHO VARCHAR(10), K INTEGER)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (WHO VARCHAR(10), K INTEGER)").unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 AR2, AR3, §12.3] a trigger on
/// another's table needs TRIGGER on it, and SELECT too if it names a
/// transition variable or table; GRANT TRIGGER gives it.
#[test]
fn a_trigger_needs_the_trigger_privilege() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    let on_t = "CREATE TRIGGER B.W AFTER INSERT ON A.T FOR EACH STATEMENT INSERT INTO B.M VALUES ('B', 0)";
    assert_eq!(b.run(on_t).unwrap_err().code, Code::Privilege);
    a.run("GRANT TRIGGER ON T TO B").unwrap();
    a.commit().unwrap();
    let named = "CREATE TRIGGER B.V AFTER INSERT ON A.T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO B.M VALUES ('B', NR.K)";
    assert_eq!(b.run(named).unwrap_err().code, Code::Privilege);
    b.run(on_t).unwrap();
    b.commit().unwrap();
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT WHO, K FROM M"), ["B,0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.21 GR6b, §6.4 GR] a triggered
/// action runs as the owner of the trigger's schema: CURRENT_USER is that
/// owner, and the statement's user needs no privilege on what it writes.
#[test]
fn an_action_runs_as_the_schema_owner() {
    let db = owned();
    let a = db.session("A");
    a.run("CREATE TRIGGER WHO AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES (CURRENT_USER, NR.K)").unwrap();
    a.run("GRANT INSERT ON T TO C").unwrap();
    a.commit().unwrap();
    let c = db.session("C");
    c.run("INSERT INTO A.T VALUES (7, 70)").unwrap();
    c.commit().unwrap();
    assert_eq!(rows(&a, "SELECT WHO, K FROM L"), ["A,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3, §12.7] ALL PRIVILEGES holds
/// TRIGGER, TABLE_PRIVILEGES shows it, and REVOKE of it abandons a
/// trigger that needs it: refused with RESTRICT, dropped with CASCADE.
#[test]
fn revoking_trigger_abandons_the_trigger() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT ALL PRIVILEGES ON T TO B").unwrap();
    a.commit().unwrap();
    let shown = "SELECT PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE GRANTEE = 'B' AND TABLE_NAME = 'T' AND PRIVILEGE_TYPE = 'TRIGGER'";
    assert_eq!(rows(&b, shown), ["TRIGGER"]);
    b.run("CREATE TRIGGER B.W AFTER DELETE ON A.T FOR EACH STATEMENT INSERT INTO B.M VALUES ('B', 0)").unwrap();
    b.commit().unwrap();
    assert_eq!(a.run("REVOKE TRIGGER ON T FROM B RESTRICT").unwrap_err().code, Code::SyntaxRule);
    a.run("REVOKE TRIGGER ON T FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, shown), Vec::<String>::new());
    a.run("DELETE FROM T").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT WHO FROM M"), Vec::<String>::new());
    assert_eq!(b.run("DROP TRIGGER B.W").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 SR3b] REVOKE CASCADE of a
/// privilege a triggered action uses drops the trigger.
#[test]
fn revoking_what_an_action_writes_drops_the_trigger() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    b.run("GRANT INSERT ON M TO A").unwrap();
    b.commit().unwrap();
    a.run("CREATE TRIGGER TO_M AFTER INSERT ON T FOR EACH STATEMENT INSERT INTO B.M VALUES ('A', 0)").unwrap();
    a.commit().unwrap();
    assert_eq!(b.run("REVOKE INSERT ON M FROM A RESTRICT").unwrap_err().code, Code::SyntaxRule);
    b.run("REVOKE INSERT ON M FROM A CASCADE").unwrap();
    b.commit().unwrap();
    a.run("INSERT INTO T VALUES (1, 1)").unwrap();
    assert_eq!(a.run("DROP TRIGGER TO_M").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 AR1, §11.50 AR1] only the owner
/// of a schema defines or drops a trigger in it.
#[test]
fn only_the_owner_defines_and_drops() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    let made = "CREATE TRIGGER A.X AFTER INSERT ON A.T FOR EACH STATEMENT DELETE FROM A.L";
    assert_eq!(b.run(made).unwrap_err().code, Code::Privilege);
    a.run(made).unwrap();
    a.commit().unwrap();
    assert_eq!(b.run("DROP TRIGGER A.X").unwrap_err().code, Code::Privilege);
}
