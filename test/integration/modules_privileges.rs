//! EXECUTE on an SQL-server module [ISO/IEC 9075-4:2006 draft §8.1, §10.1
//! to §10.3, §18.2, §18.6]: what invokes a module's routines needs it,
//! GRANT and REVOKE of it, the revoke's abandonment, the owner's privilege
//! made grantable, and the views that show it.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// User OWNER, the database's owner; schemas A, B and O of their owners: O.OT (K) of row 5, which A may
/// read; A.T (K) of row 1; A's module SHOP: PLACE (X) inserts X into T,
/// COUNTED () counts T's rows.
fn defined() -> Database {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION O CREATE TABLE OT (K INTEGER) GRANT SELECT ON OT TO A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let o = db.session("O");
    o.run("INSERT INTO OT VALUES (5)").unwrap();
    o.commit().unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.run(
        "CREATE MODULE SHOP \
         PROCEDURE PLACE (IN X INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (X); \
         FUNCTION COUNTED () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(K) FROM T); \
         END MODULE",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// The values of `query` in `s`, as text.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The privileges on modules `s` sees.
fn privileges(s: &Session) -> Vec<String> {
    rows(
        s,
        "SELECT GRANTOR, GRANTEE, MODULE_NAME, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.MODULE_PRIVILEGES ORDER BY GRANTOR, GRANTEE",
    )
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §8.1 SR1, §10.2 SR1, §18.4, §18.7; ISO/IEC 9075-2:2023 §11.60 GR1]
/// A routine of a module is invoked by one that holds EXECUTE on the
/// module, which the owner of its schema holds of `_SYSTEM`: before the
/// grant B is refused (SQLCODE -104), and sees neither the module nor its
/// routines; after it, B invokes them and sees both. EXECUTE on a routine
/// of a module, or another privilege on a module, is refused.
#[test]
fn execute_on_the_module_invokes_its_routines_8_1() {
    let db = defined();
    let (a, b) = (db.session("A"), db.session("B"));
    let e = b.run("CALL A.PLACE(2)").unwrap_err();
    assert_eq!((e.code, e.sqlcode()), (Code::Privilege, -104), "{e:?}");
    b.rollback().unwrap();
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MODULES"), ["0"]);
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE MODULE_NAME = 'SHOP'"), ["0"]);
    let e = a.run("GRANT EXECUTE ON PROCEDURE PLACE TO B").unwrap_err();
    assert!(e.rule.as_deref().unwrap_or_default().contains("§11.60 GR1"), "{e:?}");
    a.rollback().unwrap();
    let e = a.run("GRANT SELECT ON MODULE SHOP TO B").unwrap_err();
    assert!(e.rule.as_deref().unwrap_or_default().contains("§10.2 SR1"), "{e:?}");
    a.rollback().unwrap();
    a.run("GRANT EXECUTE ON MODULE SHOP TO B").unwrap();
    a.commit().unwrap();
    b.run("CALL A.PLACE(2)").unwrap();
    assert_eq!(rows(&b, "SELECT A.COUNTED() FROM (VALUES (0)) AS X (Z)"), ["2"]);
    b.commit().unwrap();
    assert_eq!(rows(&b, "SELECT MODULE_NAME, MODULE_DEFINITION FROM INFORMATION_SCHEMA.MODULES"), ["SHOP,NULL"]);
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE MODULE_NAME = 'SHOP'"), ["2"]);
    assert_eq!(privileges(&b), ["A,B,SHOP,EXECUTE,NO"]);
    assert_eq!(privileges(&a), ["A,B,SHOP,EXECUTE,NO", "_SYSTEM,A,SHOP,EXECUTE,YES"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §10.3 GR1, GR5; ISO/IEC 9075-2:2023 §12.7 GR9, GR12q]
/// REVOKE EXECUTE ON MODULE RESTRICT is refused (SQLSTATE 2B000) while a
/// view or a routine of the grantee invokes a routine of the module, or a
/// grant of it depends on the grant option revoked; with CASCADE those go.
#[test]
fn a_revoke_of_execute_on_a_module_abandons_10_3() {
    let db = defined();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT EXECUTE ON MODULE SHOP TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("CREATE VIEW SEEN AS SELECT A.COUNTED() AS N FROM (VALUES (0)) AS X (Z)").unwrap();
    b.run("CREATE PROCEDURE ALSO (IN X INTEGER) MODIFIES SQL DATA CALL A.PLACE(X)").unwrap();
    b.run("GRANT EXECUTE ON MODULE A.SHOP TO C").unwrap();
    b.commit().unwrap();
    let e = a.run("REVOKE EXECUTE ON MODULE SHOP FROM B RESTRICT").unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2B000", "{e:?}");
    a.rollback().unwrap();
    a.run("REVOKE GRANT OPTION FOR EXECUTE ON MODULE SHOP FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(privileges(&a), ["A,B,SHOP,EXECUTE,NO", "_SYSTEM,A,SHOP,EXECUTE,YES"]);
    assert_eq!(rows(&b, "SELECT N FROM SEEN"), ["1"]);
    a.run("REVOKE EXECUTE ON MODULE SHOP FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = 'B'"), ["0"]);
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'B'"), ["0"]);
    let e = a.run("REVOKE EXECUTE ON MODULE SHOP FROM B RESTRICT").unwrap();
    assert!(format!("{e:?}").contains("Warning"), "{e:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 GR2, §10.1 GR1]
/// The owner's EXECUTE on a module is grantable once it holds grantably
/// every privilege its routines need: not while A may read O.OT without
/// the grant option, then once O grants it, so A grants EXECUTE on the
/// module with effect.
#[test]
fn the_owner_s_execute_becomes_grantable_10_1() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE MODULE READER FUNCTION FIVE () RETURNS INTEGER READS SQL DATA RETURN (SELECT K FROM O.OT); END MODULE").unwrap();
    a.commit().unwrap();
    let system = "SELECT IS_GRANTABLE FROM INFORMATION_SCHEMA.MODULE_PRIVILEGES WHERE MODULE_NAME = 'READER'";
    assert_eq!(rows(&a, system), ["NO"]);
    let granted = a.run("GRANT EXECUTE ON MODULE READER TO B").unwrap();
    assert!(format!("{granted:?}").contains("Warning"), "{granted:?}");
    a.rollback().unwrap();
    let o = db.session("O");
    o.run("GRANT SELECT ON OT TO A WITH GRANT OPTION").unwrap();
    o.commit().unwrap();
    assert_eq!(rows(&a, system), ["YES"]);
    a.run("GRANT EXECUTE ON MODULE READER TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&db.session("B"), "SELECT A.FIVE() FROM (VALUES (0)) AS X (Z)"), ["5"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §18.1, §18.3, §18.6]
/// ROLE_MODULE_GRANTS shows a privilege on a module granted an enabled
/// role; MODULE_TABLE_USAGE and MODULE_COLUMN_USAGE show the tables and
/// columns of the schemas the current user owns a module's routines
/// reference, to that owner alone.
#[test]
fn the_views_of_modules_18() {
    let db = defined();
    let owner = db.session("OWNER");
    owner.run("CREATE ROLE CLERK").unwrap();
    owner.run("GRANT CLERK TO B").unwrap();
    owner.commit().unwrap();
    let a = db.session("A");
    a.run("GRANT EXECUTE ON MODULE SHOP TO CLERK").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("SET ROLE 'CLERK'").unwrap();
    let grants = "SELECT GRANTOR, GRANTEE, MODULE_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.ROLE_MODULE_GRANTS";
    assert_eq!(rows(&b, grants), ["A,CLERK,SHOP,EXECUTE"]);
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MODULE_TABLE_USAGE"), ["0"]);
    assert_eq!(rows(&a, "SELECT MODULE_NAME, TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.MODULE_TABLE_USAGE"), ["SHOP,A,T"]);
    let columns = "SELECT MODULE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.MODULE_COLUMN_USAGE";
    assert_eq!(rows(&a, columns), ["SHOP,T,K"]);
}
