//! USAGE on a distinct type and EXECUTE on its functions [ISO/IEC
//! 9075-2:2023 §12.2, §12.3, §12.7, §10.6; RULES.md D69]: granted, with
//! and without the grant option, and revoked, RESTRICT refusing what would
//! be abandoned (2B000) and CASCADE dropping it.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{money, rows};

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The SQLSTATE `sql` completes with in `s`.
fn completed(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}")).sqlstate().as_str().to_owned()
}

/// The SQLCODE class and SQLSTATE of the refusal of `sql` in `s`, the
/// statement then rolled back.
fn refused(s: &Session, sql: &str) -> (Code, String) {
    let e = s.run(sql).expect_err(sql);
    s.rollback().unwrap();
    (e.code, e.sqlstate().as_str().to_owned())
}

/// `money`'s database, schemas B and C, and the sessions of A and B.
fn granted() -> (Database, Session, Session) {
    let db = money();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    (db, a, b)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3 SR4a, SR4c, SR4d; §10.6 SR1,
/// SR3c; §6.1 AR1] USAGE is granted on a type and EXECUTE on a function,
/// by its specific name or its name and parameters' types; the type is
/// then named, and the function invoked, by the grantee. Refused: a type
/// or specific name not there, a name designating more than one function,
/// UNDER on a distinct type, another privilege on a type, and EXECUTE on a
/// table.
#[test]
fn usage_and_execute_granted_12_3() {
    let (_db, a, b) = granted();
    assert_eq!(b.run("CREATE TABLE U (M A.MONEY)").unwrap_err().code, Code::Privilege);
    b.rollback().unwrap();
    a.run("CREATE TYPE EUROS AS NUMERIC(10, 2) CAST (DISTINCT AS SOURCE) WITH AMOUNT").unwrap();
    a.run("CREATE TYPE YEN AS INTEGER CAST (DISTINCT AS SOURCE) WITH AMOUNT").unwrap();
    a.commit().unwrap();
    for sql in [
        "GRANT USAGE ON TYPE NOPE TO B",
        "GRANT EXECUTE ON SPECIFIC FUNCTION NOPE TO B",
        "GRANT EXECUTE ON FUNCTION AMOUNT TO B",
        "GRANT UNDER ON TYPE MONEY TO B",
        "GRANT SELECT ON TYPE MONEY TO B",
        "GRANT EXECUTE ON TABLE T TO B",
    ] {
        assert_eq!(refused(&a, sql).0, Code::SyntaxRule, "{sql}");
    }
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION AMOUNT(YEN) TO B").unwrap();
    a.run("GRANT EXECUTE ON SPECIFIC FUNCTION MONEY_TO_DISTINCT TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE TABLE U (K INTEGER, M A.MONEY)").unwrap();
    b.run("INSERT INTO U VALUES (1, A.MONEY(2))").unwrap();
    assert_eq!(query(&b, "SELECT A.MONEY(3) FROM U"), ["3.00"]);
    let e = b.open_cursor("SELECT A.AMOUNT(A.YEN(3)) FROM U").err().expect("refused");
    assert!(e.code == Code::Privilege && e.to_string().contains("A.YEN"), "EXECUTE on AMOUNT of YEN, not on YEN: {e}");
    let held = "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE GRANTEE = 'B' ORDER BY 1";
    assert_eq!(query(&b, held), ["MONEY_TO_DISTINCT", "YEN_TO_SOURCE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.2 GR, §12.3] a privilege is granted
/// on by one who holds it grantably alone: B, holding USAGE and EXECUTE
/// with no grant option, grants nothing (01007); holding them WITH GRANT
/// OPTION, grants them to C.
#[test]
fn granted_on_with_the_grant_option_12_2() {
    let (db, a, b) = granted();
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION MONEY TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(completed(&b, "GRANT USAGE ON TYPE A.MONEY TO C"), "01007");
    assert_eq!(completed(&b, "GRANT EXECUTE ON FUNCTION A.MONEY TO C"), "01007");
    b.commit().unwrap();
    let c = db.session("C");
    assert_eq!(query(&c, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.UDT_PRIVILEGES"), ["0"]);
    a.run("GRANT USAGE ON TYPE MONEY TO B WITH GRANT OPTION").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION MONEY TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    assert_eq!(completed(&b, "GRANT USAGE ON TYPE A.MONEY TO C"), "00000");
    assert_eq!(completed(&b, "GRANT EXECUTE ON FUNCTION A.MONEY TO C"), "00000");
    b.commit().unwrap();
    assert_eq!(query(&c, "SELECT GRANTOR, UDT_NAME FROM INFORMATION_SCHEMA.UDT_PRIVILEGES"), ["B,MONEY"]);
    assert_eq!(query(&c, "SELECT GRANTOR, SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES"), ["B,MONEY_TO_DISTINCT"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12h ii, GR12q, GR12z; §12.7
/// GR12] REVOKE with RESTRICT is refused (2B000) where it would abandon a
/// column of a type based on the type, or a view invoking a function;
/// with CASCADE, the column and the view go. A privilege not granted is
/// not revoked (01006).
#[test]
fn revoke_restrict_and_cascade_12_7() {
    let (_db, a, b) = granted();
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION MONEY TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE TABLE U (K INTEGER, M A.MONEY)").unwrap();
    b.run("INSERT INTO U VALUES (1, 2)").unwrap();
    b.run("CREATE VIEW W AS SELECT K, A.MONEY(K) AS M FROM U").unwrap();
    b.commit().unwrap();
    assert_eq!(refused(&a, "REVOKE USAGE ON TYPE MONEY FROM B RESTRICT"), (Code::SyntaxRule, "2B000".to_owned()));
    assert_eq!(refused(&a, "REVOKE EXECUTE ON FUNCTION MONEY FROM B RESTRICT"), (Code::SyntaxRule, "2B000".to_owned()));
    assert_eq!(completed(&a, "REVOKE EXECUTE ON FUNCTION MONEY FROM C RESTRICT"), "01006");
    a.run("REVOKE EXECUTE ON FUNCTION MONEY FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'W'"), ["0"]);
    assert_eq!(query(&b, "SELECT K, M FROM U"), ["1,2.00"]);
    a.run("REVOKE USAGE ON TYPE MONEY FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT * FROM U"), ["1"], "column M gone, its row kept");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12z, §11.23 SR4] a REVOKE or a
/// DROP TYPE with CASCADE that would drop every column of a table is
/// refused before it drops any, the transaction left as it was.
/// Regression: the other columns were dropped first and the transaction
/// doomed, a query in it reading no rows of their table.
#[test]
fn every_column_of_a_table_is_not_dropped_11_23() {
    let (_db, a, b) = granted();
    a.run("INSERT INTO T VALUES (1, 5)").unwrap();
    a.run("GRANT USAGE ON TYPE MONEY TO B").unwrap();
    a.commit().unwrap();
    b.run("CREATE TABLE U (M A.MONEY)").unwrap();
    b.commit().unwrap();
    let e = a.run("DROP TYPE MONEY CASCADE").unwrap_err();
    assert!(e.to_string().contains("§11.23 SR4"), "{e}");
    assert_eq!(query(&a, "SELECT K, P FROM T"), ["1,5.00"], "nothing dropped");
    a.commit().unwrap();
    assert_eq!(a.run("REVOKE USAGE ON TYPE MONEY FROM B CASCADE").unwrap_err().code, Code::SyntaxRule);
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT K, P FROM T"), ["1,5.00"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12, GR12h ii] REVOKE CASCADE
/// takes what was granted on with the privilege revoked: B's grant to C,
/// and C's column of the type. MC/DC of the grant option: of GRANT OPTION
/// FOR alone, B keeps USAGE and C loses it; of the privilege, B loses it
/// too.
#[test]
fn revoke_takes_what_was_granted_on_12_7() {
    let (db, a, b) = granted();
    a.run("GRANT USAGE ON TYPE MONEY TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT USAGE ON TYPE A.MONEY TO C").unwrap();
    b.commit().unwrap();
    let c = db.session("C");
    c.run("CREATE TABLE V (K INTEGER, M A.MONEY)").unwrap();
    c.commit().unwrap();
    let held = "SELECT GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.UDT_PRIVILEGES WHERE GRANTEE IN ('B', 'C')";
    assert_eq!(query(&c, held), ["C,NO"]);
    a.run("REVOKE GRANT OPTION FOR USAGE ON TYPE MONEY FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, held), ["B,NO"]);
    assert_eq!(query(&c, held), Vec::<String>::new());
    assert_eq!(query(&c, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V'"), ["K"]);
    a.run("REVOKE USAGE ON TYPE MONEY FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, held), Vec::<String>::new());
}
