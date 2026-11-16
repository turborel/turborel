//! The EXECUTE privilege on SQL-invoked routines: granted and revoked
//! [ISO/IEC 9075-2:2023 §12.3, §12.7], the owner's, grantable as the body
//! allows [§11.60 GR1], and the routines a revoke abandons [§12.7 GR12o].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};

/// Schema A: T (K) of rows 1 and 2, and function F (X) = X + 1, of
/// CONTAINS SQL; schema B, owned by B, with M (K) of row 5.
fn owned() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (K INTEGER)").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("INSERT INTO T VALUES (1), (2)").unwrap();
    a.run("CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 1").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO M VALUES (5)").unwrap();
    b.commit().unwrap();
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

/// The SQLCODE class and SQLSTATE of a statement that fails.
fn refused<T>(result: Result<T, SqlError>) -> (Option<Code>, String) {
    let e = result.err();
    (e.as_ref().map(|e| e.code), e.map(|e| e.sqlstate().as_str().to_owned()).unwrap_or_default())
}

/// The SQLCODE class of opening a cursor on `query` in `s`, which fails.
fn opened(s: &Session, query: &str) -> Option<Code> {
    s.open_cursor(query).err().map(|e| e.code)
}

/// The grantee, grantor and grantability of each EXECUTE on routines of
/// schema A, in order.
fn grants(s: &Session) -> Vec<String> {
    rows(
        s,
        "SELECT GRANTEE, GRANTOR, IS_GRANTABLE FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES \
         WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME, GRANTEE",
    )
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3, §10.6 SR3c, §9.18 SR6] GRANT
/// EXECUTE on a routine, named as a FUNCTION, a ROUTINE, with its
/// parameters' types or by its specific name, lets the grantee invoke it;
/// without it, no routine is found; REVOKE takes it away. MC/DC of the
/// grant's routine type: FUNCTION and ROUTINE name a function, PROCEDURE
/// does not.
#[test]
fn execute_is_granted_and_revoked_12_3() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    let invoked = "SELECT A.F(K) FROM M";
    assert_eq!(opened(&b, invoked), Some(Code::Privilege), "no EXECUTE");
    assert_eq!(a.run("GRANT EXECUTE ON PROCEDURE F TO B").unwrap_err().code, Code::SyntaxRule, "not a procedure");
    for grant in ["GRANT EXECUTE ON FUNCTION F TO B", "GRANT EXECUTE ON ROUTINE F (INTEGER) TO B"] {
        a.run(grant).unwrap();
        a.commit().unwrap();
        assert_eq!(rows(&b, invoked), ["6"], "{grant}");
        a.run("REVOKE EXECUTE ON FUNCTION F FROM B CASCADE").unwrap();
        a.commit().unwrap();
        assert_eq!(opened(&b, invoked), Some(Code::Privilege), "{grant} revoked");
    }
    let specific = rows(&a, "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'F'").concat();
    a.run(&format!("GRANT EXECUTE ON SPECIFIC FUNCTION {specific} TO B")).unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, invoked), ["6"], "by its specific name");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3 SR4d, §12.7] ALL PRIVILEGES on a
/// routine is EXECUTE, granted to PUBLIC to all.
#[test]
fn all_privileges_on_a_routine_is_execute_12_3() {
    let db = owned();
    let a = db.session("A");
    a.run("GRANT ALL PRIVILEGES ON FUNCTION F TO PUBLIC").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&db.session("C"), "SELECT A.F(1) FROM (VALUES (0)) AS X (Z)"), ["2"]);
    assert_eq!(grants(&a), ["A,_SYSTEM,YES", "PUBLIC,A,NO"]);
    a.run("REVOKE ALL PRIVILEGES ON FUNCTION F FROM PUBLIC CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(grants(&a), ["A,_SYSTEM,YES"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 GR1, §12.3 GR] the owner holds
/// EXECUTE from _SYSTEM, grantable only if the owner holds each privilege
/// the body needs grantable; one not grantable may not be granted on.
/// MC/DC of the grantability: F reads nothing (YES), R reads B.M with
/// SELECT grantable (YES), S reads it with SELECT not grantable (NO).
#[test]
fn the_owners_execute_is_grantable_as_the_body_allows_11_60() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    b.run("GRANT SELECT ON M TO A WITH GRANT OPTION").unwrap();
    b.commit().unwrap();
    a.run("CREATE FUNCTION R () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(K) FROM B.M)").unwrap();
    a.commit().unwrap();
    b.run("REVOKE GRANT OPTION FOR SELECT ON M FROM A CASCADE").unwrap();
    b.commit().unwrap();
    a.run("CREATE FUNCTION S () RETURNS INTEGER READS SQL DATA RETURN (SELECT MIN(K) FROM B.M)").unwrap();
    a.commit().unwrap();
    assert_eq!(grants(&a), ["A,_SYSTEM,YES", "A,_SYSTEM,YES", "A,_SYSTEM,NO"]);
    assert_eq!(a.run("GRANT EXECUTE ON FUNCTION S TO C").unwrap().sqlstate().as_str(), "01007", "privilege not granted");
    a.run("GRANT EXECUTE ON FUNCTION F TO C WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&db.session("C"), "SELECT A.F(1) FROM (VALUES (0)) AS X (Z)"), ["2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12o, SR3b, §11.62 GR1] a REVOKE
/// that leaves a routine's owner without a privilege its body needs is
/// refused with RESTRICT (2B000), and with CASCADE drops the routine and
/// what invokes it.
#[test]
fn a_revoke_abandons_a_routine_12_7() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    b.run("GRANT SELECT ON M TO A").unwrap();
    b.commit().unwrap();
    a.run("CREATE FUNCTION R () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(K) FROM B.M)").unwrap();
    a.run("CREATE VIEW V (R) AS SELECT R() FROM T").unwrap();
    a.run("CREATE PROCEDURE P (OUT N INTEGER) READS SQL DATA SELECT COUNT(*) INTO N FROM B.M").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT R FROM V WHERE R > 0"), ["5", "5"]);
    assert_eq!(refused(b.run("REVOKE SELECT ON M FROM A RESTRICT")), (Some(Code::SyntaxRule), "2B000".to_owned()));
    b.run("REVOKE SELECT ON M FROM A CASCADE").unwrap();
    b.commit().unwrap();
    let left = "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME";
    assert_eq!(rows(&a, left), ["F"]);
    assert_eq!(opened(&a, "SELECT * FROM V"), Some(Code::SyntaxRule), "the view that invoked R is gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12o, §11.60 SR23 AR] a routine
/// that invokes another of another schema needs EXECUTE on it, and a
/// revoke of that EXECUTE abandons it.
#[test]
fn a_revoke_of_execute_abandons_an_invoker_12_7() {
    let db = owned();
    let (a, b) = (db.session("A"), db.session("B"));
    assert_eq!(b.run("CREATE FUNCTION G (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN A.F(X) * 2").unwrap_err().code, Code::Privilege);
    a.run("GRANT EXECUTE ON FUNCTION F TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE FUNCTION G (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN A.F(X) * 2").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&b, "SELECT G(K) FROM M"), ["12"]);
    assert_eq!(refused(a.run("REVOKE EXECUTE ON FUNCTION F FROM B RESTRICT")), (Some(Code::SyntaxRule), "2B000".to_owned()));
    a.run("REVOKE EXECUTE ON FUNCTION F FROM B CASCADE").unwrap();
    a.commit().unwrap();
    let left = "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'B'";
    assert_eq!(rows(&b, left), Vec::<String>::new(), "G is gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.1 GR3, §11.60, §12.3] a schema
/// definition may create a routine and grant EXECUTE on it, the grant
/// applied after the routine is made.
#[test]
fn a_schema_definition_grants_on_its_routine_11_1() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION D \
         GRANT EXECUTE ON FUNCTION TWICE TO E \
         CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X * 2",
    )
    .unwrap();
    assert_eq!(rows(&db.session("E"), "SELECT D.TWICE(4) FROM (VALUES (0)) AS X (Z)"), ["8"]);
}
