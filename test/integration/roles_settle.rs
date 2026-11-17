//! [ISO/IEC 9075-2:2023 §12.6, §12.7 GR8, GR9, GR12] The privileges of
//! every kind a change of role authorizations leaves: USAGE on a domain,
//! a collation, a sequence generator and a distinct type, and EXECUTE on
//! a routine, granted by a role through its applicable roles, settled
//! again as REVOKE of a role or DROP ROLE takes them, with what needs
//! them abandoned.

use turborel::db::{Completion, Database};
use turborel::error::Code;

use crate::common::{exec, failure, rows};
use crate::roles::{committed, owned};

/// `owned`, with A's domain D, collation L, sequence Q, distinct type
/// MONEY and function F; role R2 holding USAGE on each and EXECUTE on F
/// grantably, R2 granted to R1, R1 to B; and C granted each by B as R1,
/// GRANTED BY CURRENT_ROLE, R1 holding them only through R2. C's table N
/// has a column of D, one of MONEY and one collated by L, and C's view W
/// invokes F.
fn settled() -> Database {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R1", "CREATE ROLE R2", "GRANT R2 TO R1", "GRANT R1 TO B"]);
    committed(
        &db,
        "A",
        &[
            "CREATE DOMAIN D AS INTEGER CHECK (VALUE > 0)",
            "CREATE COLLATION L FOR SQL_TEXT FROM CASE_INSENSITIVE NO PAD",
            "CREATE SEQUENCE Q",
            "CREATE TYPE MONEY AS NUMERIC(10, 2)",
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 1",
            "GRANT USAGE ON DOMAIN D TO R2 WITH GRANT OPTION",
            "GRANT USAGE ON COLLATION A.L TO R2 WITH GRANT OPTION",
            "GRANT USAGE ON SEQUENCE Q TO R2 WITH GRANT OPTION",
            "GRANT USAGE ON TYPE MONEY TO R2 WITH GRANT OPTION",
            "GRANT EXECUTE ON FUNCTION F TO R2 WITH GRANT OPTION",
        ],
    );
    let b = db.session("B");
    b.run("SET ROLE 'R1'").unwrap();
    for on in ["DOMAIN A.D", "COLLATION A.L", "SEQUENCE A.Q", "TYPE A.MONEY"] {
        b.run(&format!("GRANT USAGE ON {on} TO C GRANTED BY CURRENT_ROLE")).unwrap();
    }
    b.run("GRANT EXECUTE ON FUNCTION A.F TO C GRANTED BY CURRENT_ROLE").unwrap();
    b.commit().unwrap();
    committed(
        &db,
        "C",
        &[
            "CREATE TABLE C.N (K INTEGER, P A.D, M A.MONEY, S VARCHAR(5) COLLATE A.L)",
            "CREATE VIEW C.W AS SELECT A.F(K) AS G FROM C.N",
            "INSERT INTO C.N (K, P, S) VALUES (1, 2, 'x')",
        ],
    );
    db
}

/// The privileges C holds, as the Information Schema tells them: of
/// domains, collations and sequences, of types, and of routines.
fn held(db: &Database) -> Vec<String> {
    let c = db.session("C");
    let mut all =
        rows(&c, "SELECT OBJECT_TYPE, OBJECT_NAME FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE GRANTEE = 'C' ORDER BY 1, 2").unwrap();
    all.extend(rows(&c, "SELECT UDT_NAME FROM INFORMATION_SCHEMA.UDT_PRIVILEGES WHERE GRANTEE = 'C'").unwrap());
    all.extend(rows(&c, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE GRANTEE = 'C'").unwrap());
    all
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR8, GR9, GR12q] a
/// privilege of each kind a role granted stands while the role holds it
/// grantably through its applicable roles: revoking R1 from B, the user
/// who granted as R1, leaves them; revoking R2 from R1 abandons USAGE on
/// the domain, collation, sequence and type and EXECUTE on the function,
/// so RESTRICT is refused (2B000), while CASCADE takes them and drops the
/// view invoking the function.
#[test]
fn every_kind_of_privilege_falls_with_its_role_12_7() {
    let db = settled();
    let before = held(&db);
    assert_eq!(before, ["COLLATION,L", "DOMAIN,D", "SEQUENCE,Q", "MONEY", "F"]);
    committed(&db, "OWNER", &["REVOKE R1 FROM B RESTRICT"]);
    assert_eq!(held(&db), before, "R1 granted them, not B");
    let o = db.session("OWNER");
    assert_eq!(failure(o.run("REVOKE R2 FROM R1 RESTRICT")), (Code::SyntaxRule, "2B000".to_owned()));
    o.rollback().unwrap();
    assert_eq!(held(&db), before, "nothing changed");
    assert_eq!(exec(&o, "REVOKE R2 FROM R1 CASCADE"), Ok(Completion::Success));
    o.commit().unwrap();
    assert_eq!(held(&db), Vec::<String>::new(), "every privilege R1 granted is gone");
    let c = db.session("C");
    assert_eq!(rows(&c, "SELECT G FROM C.W"), Err(Code::SyntaxRule), "the view was dropped");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.6 GR1, GR2, §12.7 GR12]
/// DROP ROLE of the role holding the privileges of each kind the grants
/// of its grantee stand on: RESTRICT is refused (2B000), CASCADE takes
/// every grant to it or by it, and those then granted by a role holding
/// them no more.
#[test]
fn drop_role_settles_every_kind_of_privilege_12_6() {
    let db = settled();
    let o = db.session("OWNER");
    assert_eq!(failure(o.run("DROP ROLE R2 RESTRICT")), (Code::SyntaxRule, "2B000".to_owned()));
    o.rollback().unwrap();
    assert_eq!(exec(&o, "DROP ROLE R2 CASCADE"), Ok(Completion::Success));
    o.commit().unwrap();
    assert_eq!(held(&db), Vec::<String>::new());
    let a = db.session("A");
    let kept = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE GRANTEE IN ('R1', 'R2')";
    assert_eq!(rows(&a, kept).unwrap(), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR8, GR9] granted by
/// the user who holds them, not by a role, the privileges of each kind
/// stand as roles are revoked and dropped around them.
#[test]
fn privileges_a_user_granted_stand_as_roles_change_12_7() {
    let db = settled();
    committed(&db, "A", &["GRANT USAGE ON DOMAIN D TO B", "GRANT EXECUTE ON FUNCTION F TO B", "GRANT USAGE ON TYPE MONEY TO B"]);
    committed(&db, "OWNER", &["DROP ROLE R2 CASCADE", "DROP ROLE R1 CASCADE"]);
    let b = db.session("B");
    let usage = "SELECT OBJECT_NAME FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE GRANTEE = 'B'";
    assert_eq!(rows(&b, usage).unwrap(), ["D"]);
    assert_eq!(rows(&b, "SELECT UDT_NAME FROM INFORMATION_SCHEMA.UDT_PRIVILEGES WHERE GRANTEE = 'B'").unwrap(), ["MONEY"]);
}
