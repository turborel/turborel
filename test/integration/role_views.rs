//! [ISO/IEC 9075-11:2006 draft §5.8, §5.9, §5.30, §5.38 to §5.43] The
//! Information Schema's views of roles, and the other views as they read
//! the enabled roles: what a role brings a session is shown as its
//! privileges are current [ISO/IEC 9075-2:2023 §4.42.4].

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;
use crate::roles::{committed, owned};

/// Roles R, S granted to R, and P granted to PUBLIC; R granted to B WITH
/// ADMIN OPTION. A grants SELECT on T, and USAGE on its sequence Q, to R,
/// and EXECUTE on its function F to S; and B grants SELECT on T on, as R,
/// to C.
fn roled() -> Database {
    let db = owned();
    committed(
        &db,
        "OWNER",
        &["CREATE ROLE R", "CREATE ROLE S", "CREATE ROLE P", "GRANT S TO R", "GRANT P TO PUBLIC", "GRANT R TO B WITH ADMIN OPTION"],
    );
    committed(
        &db,
        "A",
        &[
            "CREATE SEQUENCE Q",
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X",
            "GRANT SELECT ON T TO R WITH GRANT OPTION",
            "GRANT USAGE ON SEQUENCE Q TO R",
            "GRANT EXECUTE ON FUNCTION F TO S",
        ],
    );
    let b = db.session("B");
    b.run("SET ROLE 'R'").unwrap();
    b.run("GRANT SELECT ON A.T TO C GRANTED BY CURRENT_ROLE").unwrap();
    b.commit().unwrap();
    db
}

/// The rows of `sql` in `s`, then commits, so SET ROLE may follow.
fn shown(s: &Session, sql: &str) -> Vec<String> {
    let out = rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.commit().unwrap();
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §5.30, §5.9, §5.8;
/// ISO/IEC 9075-2:2023 §4.42.4]: ENABLED_ROLES has no row until a role is
/// set, then the role, each role applicable for it through a chain, and
/// each granted to PUBLIC; APPLICABLE_ROLES has the role authorizations of
/// the current user, of PUBLIC and of the roles applicable through them,
/// set or not; ADMINISTRABLE_ROLE_AUTHORIZATIONS has every authorization of
/// a role the user holds WITH ADMIN OPTION, and none of a role it holds
/// without.
#[test]
fn enabled_applicable_and_administrable_roles_5_30() {
    let db = roled();
    let b = db.session("B");
    let enabled = "SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES ORDER BY 1";
    assert_eq!(shown(&b, enabled), Vec::<String>::new(), "no role is set, and a null one enables none");
    let applicable = "SELECT GRANTEE, ROLE_NAME, IS_GRANTABLE FROM INFORMATION_SCHEMA.APPLICABLE_ROLES ORDER BY 2";
    let of_b = ["PUBLIC,P,NO", "B,R,YES", "R,S,NO"];
    assert_eq!(shown(&b, applicable), of_b);
    b.run("SET ROLE 'R'").unwrap();
    assert_eq!(shown(&b, enabled), ["P", "R", "S"]);
    assert_eq!(shown(&b, applicable), of_b, "the same, set or not");
    let administrable = "SELECT GRANTEE, ROLE_NAME FROM INFORMATION_SCHEMA.ADMINISTRABLE_ROLE_AUTHORIZATIONS ORDER BY 1";
    assert_eq!(shown(&b, administrable), ["B,R", "OWNER,R"], "R's, not S's nor P's");
    let c = db.session("C");
    assert_eq!(shown(&c, applicable), ["PUBLIC,P,NO"]);
    assert_eq!(shown(&c, administrable), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §5.40, §5.38, §5.42,
/// §5.39, §5.43]: the ROLE_ grants views show the privileges granted to an
/// enabled role, through a chain too, and those it granted, and nothing
/// until a role is set.
#[test]
fn role_grants_show_what_enabled_roles_hold_and_grant_5_40() {
    let db = roled();
    let b = db.session("B");
    let tables = "SELECT GRANTOR, GRANTEE, TABLE_NAME, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.ROLE_TABLE_GRANTS ORDER BY 2";
    assert_eq!(shown(&b, tables), Vec::<String>::new(), "no role is set");
    b.run("SET ROLE 'R'").unwrap();
    assert_eq!(shown(&b, tables), ["R,C,T,SELECT,NO", "A,R,T,SELECT,YES"], "granted by R, and to it");
    let columns = "SELECT DISTINCT GRANTEE, COLUMN_NAME FROM INFORMATION_SCHEMA.ROLE_COLUMN_GRANTS ORDER BY 1";
    assert_eq!(shown(&b, columns), ["C,K", "R,K"]);
    let usage = "SELECT GRANTOR, GRANTEE, OBJECT_NAME, OBJECT_TYPE, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.ROLE_USAGE_GRANTS";
    assert_eq!(shown(&b, usage), ["A,R,Q,SEQUENCE,USAGE"]);
    let routines = "SELECT GRANTOR, GRANTEE, ROUTINE_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.ROLE_ROUTINE_GRANTS";
    assert_eq!(shown(&b, routines), ["A,S,F,EXECUTE"], "S is enabled through R");
    let types = "SELECT GRANTEE, UDT_NAME FROM INFORMATION_SCHEMA.ROLE_UDT_GRANTS";
    assert_eq!(shown(&b, types), Vec::<String>::new(), "no type");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §5.60, §5.61, §5.51,
/// §5.49, §5.50]: TABLES, TABLE_PRIVILEGES, SEQUENCES and ROUTINES show
/// what an enabled role holds a privilege on, and SCHEMATA the schema a
/// role owns, once the role is set, and not before.
#[test]
fn views_read_the_enabled_roles_5_61() {
    let db = roled();
    committed(&db, "OWNER", &["ALTER USER B CREATE ROLE"]);
    let b = db.session("B");
    b.run("CREATE ROLE O").unwrap();
    b.commit().unwrap();
    b.run("SET ROLE 'O'").unwrap();
    b.direct("CREATE SCHEMA AUTHORIZATION O").unwrap();
    b.commit().unwrap();
    let views = [
        "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A'",
        "SELECT GRANTEE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE TABLE_SCHEMA = 'A' ORDER BY 1",
        "SELECT SEQUENCE_NAME FROM INFORMATION_SCHEMA.SEQUENCES",
        "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'",
        "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY 1",
    ];
    let unset: [&[&str]; 5] = [&[], &[], &[], &[], &["B"]];
    let as_r: [&[&str]; 5] = [&["T"], &["C", "R"], &["Q"], &["F"], &["B"]];
    let as_o: [&[&str]; 5] = [&[], &[], &[], &[], &["B", "O"]];
    for (role, expected) in [("NONE", unset), ("'R'", as_r), ("'O'", as_o)] {
        b.run(&format!("SET ROLE {role}")).unwrap();
        for (sql, seen) in views.iter().zip(expected) {
            assert_eq!(shown(&b, sql), seen, "SET ROLE {role}: {sql}");
        }
    }
    assert_eq!(rows(&b, "SELECT * FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS"), Err(Code::SyntaxRule), "§21.1");
}
