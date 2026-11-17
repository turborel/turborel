//! Table/method privileges [ISO/IEC 9075-2:2023 §4.42.2, §6.23 AR1,
//! §11.3 GR7, §12.2 AR3, GR6, §12.3 SR6, SR8b i, §12.7 SR1c] (Feature
//! S024): SELECT on a typed table grants SELECT on the table/method pair
//! of each method of its type its grantor holds EXECUTE on WITH GRANT
//! OPTION, SELECT of a `<privilege method list>` grants it on the methods
//! named, and a method reference of a scope of the table needs it.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: PERSON of NAME with the method GREET; PEOPLE of PERSON, of
/// one row; HOLD of a reference to it, scoped by PEOPLE; T2, of no type.
/// B holds SELECT on HOLD and SELECT WITH HIERARCHY OPTION on PEOPLE,
/// both grantable, and EXECUTE on GREET, not grantable.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED
             METHOD GREET () RETURNS CHARACTER VARYING(30) CONTAINS SQL",
    )
    .unwrap();
    for auth in ["B", "C", "D"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {auth}")).unwrap();
    }
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE METHOD GREET () RETURNS CHARACTER VARYING(30) FOR PERSON RETURN 'HI ' || SELF.NAME",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE HOLD (R REF(PERSON) SCOPE PEOPLE)",
            "CREATE TABLE T2 (X INTEGER)",
            "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
            "INSERT INTO HOLD SELECT ID FROM PEOPLE",
            "GRANT SELECT ON HOLD TO B WITH GRANT OPTION",
            "GRANT SELECT ON PEOPLE TO B WITH HIERARCHY OPTION WITH GRANT OPTION",
            "GRANT EXECUTE ON METHOD GREET () FOR PERSON TO B, D",
        ],
    );
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Asserts `sql` in `s`, read by a cursor if a query, is refused with
/// `code` citing `cited`; rolls back.
fn refused(s: &Session, sql: &str, (code, cited): (Code, &str)) {
    let e = if sql.starts_with("SELECT") { s.open_cursor(sql).err() } else { s.run(sql).err() };
    let e = e.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!(e.code, code, "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// The table/method privileges on PEOPLE, as grantor, grantee, method and
/// grantability.
const SHOWN: &str = "SELECT GRANTOR, GRANTEE, SPECIFIC_NAME, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_METHOD_PRIVILEGES \
                     WHERE TABLE_NAME = 'PEOPLE' AND SPECIFIC_NAME NOT LIKE 'NAME%' ORDER BY 1, 2";

/// Verifies: [ISO/IEC 9075-2:2023 §6.23 AR1, §11.3 GR7, §12.2 GR6] The
/// owner holds SELECT on the table/method pair of each method; SELECT
/// granted by the owner grants it on each, so its grantee invokes a method
/// of a reference scoped by the table; SELECT granted by one who holds no
/// EXECUTE on the method WITH GRANT OPTION grants it on none, and the
/// method reference is refused.
#[test]
fn a_method_reference_needs_the_table_method_privilege_6_23() {
    let db = database();
    let (a, b, d) = (db.session("A"), db.session("B"), db.session("D"));
    assert_eq!(query(&a, "SELECT R -> GREET() FROM HOLD"), ["HI ANN"]);
    assert_eq!(query(&b, "SELECT R -> GREET() FROM A.HOLD"), ["HI ANN"]);
    ok(&b, &["GRANT SELECT ON A.HOLD TO D", "GRANT SELECT ON A.PEOPLE TO D WITH HIERARCHY OPTION"]);
    assert_eq!(query(&d, "SELECT R -> NAME FROM A.HOLD"), ["ANN"]);
    refused(&d, "SELECT R -> GREET() FROM A.HOLD", (Code::Privilege, "§6.23 AR1"));
    let shown = query(&a, SHOWN);
    assert!(shown.iter().any(|r| r.starts_with("A,B,") && r.ends_with(",YES")), "{shown:?}");
    let by_b = query(&b, SHOWN);
    assert!(!by_b.iter().any(|r| r.starts_with("B,D,")), "{by_b:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.3 SR6, SR8b i, §12.2 AR3, §12.7
/// SR1c] SELECT of a `<privilege method list>` grants SELECT on the
/// table/method pair of each method named alone, of a table of a
/// structured type, by one who holds EXECUTE on it; REVOKE of the list
/// takes it, and REVOKE of SELECT CASCADE takes those it gave with it and
/// those granted on them.
#[test]
fn select_of_a_method_list_is_granted_and_revoked_12_3() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT SELECT (METHOD GREET () FOR PERSON) ON PEOPLE TO C"]);
    assert!(query(&a, SHOWN).iter().any(|r| r.starts_with("A,C,") && r.ends_with(",NO")));
    ok(&a, &["REVOKE SELECT (METHOD GREET () FOR PERSON) ON PEOPLE FROM C RESTRICT"]);
    assert!(!query(&a, SHOWN).iter().any(|r| r.starts_with("A,C,")));
    refused(&a, "GRANT SELECT (METHOD GREET () FOR PERSON) ON T2 TO C", (Code::SyntaxRule, "§12.3 SR6"));
    ok(&b, &["GRANT SELECT (METHOD GREET () FOR A.PERSON) ON A.PEOPLE TO D WITH GRANT OPTION"]);
    // [ISO/IEC 9075-11 §5.59] shown to its grantor.
    let shown = query(&b, SHOWN);
    assert!(shown.iter().any(|r| r.starts_with("B,D,") && r.ends_with(",YES")), "{shown:?}");
    ok(&a, &["GRANT SELECT ON PEOPLE TO C WITH GRANT OPTION"]);
    let c = db.session("C");
    refused(&c, "GRANT SELECT (METHOD GREET () FOR A.PERSON) ON A.PEOPLE TO D", (Code::Privilege, "§12.2 AR3"));
    ok(&a, &["REVOKE SELECT ON PEOPLE FROM B CASCADE"]);
    let shown = [query(&a, SHOWN), query(&b, SHOWN)].concat();
    assert!(!shown.iter().any(|r| r.starts_with("A,B,") || r.starts_with("B,D,")), "{shown:?}");
    refused(&b, "SELECT R -> GREET() FROM A.HOLD", (Code::Privilege, "PEOPLE"));
}
