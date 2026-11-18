//! SELECT on a property graph (§20): only the owner of a graph's schema
//! defines, alters and drops it, holding SELECT on what it reads; SELECT
//! on it is granted and revoked as on a view, its owner's found from its
//! tables; a revoke that leaves its owner without SELECT on one drops it
//! under CASCADE.

use turborel::db::Database;
use turborel::error::SqlState;

use super::{bank, refusal, refused_with, rows};

/// The bank, and schema B of user B, who holds SELECT on ACCOUNT with
/// grant option and on CITY without it.
fn shared() -> Database {
    let db = bank();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run("GRANT SELECT ON ACCOUNT TO B WITH GRANT OPTION").unwrap();
    a.run("GRANT SELECT ON CITY TO B").unwrap();
    a.commit().unwrap();
    db
}

/// The SELECT privileges on graphs shown to `user`, by grantor, grantee,
/// graph and whether grantable.
fn privileges(db: &Database, user: &str) -> Vec<String> {
    let query = "SELECT GRANTOR, GRANTEE, PROPERTY_GRAPH_NAME, PRIVILEGE_TYPE, IS_GRANTABLE \
                 FROM INFORMATION_SCHEMA.PROPERTY_GRAPH_PRIVILEGES ORDER BY 1, 2, 3";
    rows(&db.session(user), query)
}

/// Verifies: [EXTENSION] §20; [ISO/IEC 9075:1992 §11.19 GR4]: a graph is
/// defined, altered and dropped by its schema's owner alone, who holds
/// SELECT on each element table; its owner holds SELECT on it, grantably
/// as it holds SELECT on what it reads grantably.
#[test]
fn the_owner_defines_and_holds_a_graph_20() {
    let db = shared();
    let a = db.session("A");
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT)").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    let (code, _) = refusal(&b, "CREATE PROPERTY GRAPH A.H VERTEX TABLES (A.ACCOUNT)");
    assert_eq!(code, -104);
    let (code, _) = refusal(&b, "ALTER PROPERTY GRAPH A.G COMPILE");
    assert_eq!(code, -104);
    let (code, _) = refusal(&b, "DROP PROPERTY GRAPH A.G");
    assert_eq!(code, -104);
    let (code, msg) = refusal(&b, "CREATE PROPERTY GRAPH H VERTEX TABLES (A.PHONE)");
    assert_eq!(code, -104, "{msg}");
    b.run("CREATE PROPERTY GRAPH H VERTEX TABLES (A.ACCOUNT, A.CITY)").unwrap();
    b.run("CREATE PROPERTY GRAPH J VERTEX TABLES (A.ACCOUNT)").unwrap();
    b.commit().unwrap();
    assert_eq!(privileges(&db, "B"), ["_SYSTEM,B,H,SELECT,NO", "_SYSTEM,B,J,SELECT,YES"]);
    assert_eq!(privileges(&db, "A"), ["_SYSTEM,A,G,SELECT,YES"]);
    let (code, _) = refusal(&db.session("C"), "GRANT SELECT ON PROPERTY GRAPH A.G TO D");
    assert_eq!(code, -104);
    let graphs = "SELECT PROPERTY_GRAPH_NAME FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS ORDER BY 1";
    assert_eq!(rows(&db.session("C"), graphs), Vec::<String>::new());
}

/// Verifies: [EXTENSION] §20; [ISO/IEC 9075:1992 §11.36 GR14, §11.37 SR9,
/// SR18]: GRANT SELECT ON PROPERTY GRAPH by one who holds it grantably,
/// a warning by one who does not; of SELECT alone; REVOKE of it, RESTRICT
/// refusing to abandon a grant that rests on it, CASCADE taking it too;
/// and the views show each to whom it is granted or by whom.
#[test]
fn select_on_a_graph_is_granted_and_revoked_20() {
    let db = shared();
    let b = db.session("B");
    b.run("CREATE PROPERTY GRAPH H VERTEX TABLES (A.ACCOUNT, A.CITY)").unwrap();
    b.run("CREATE PROPERTY GRAPH J VERTEX TABLES (A.ACCOUNT)").unwrap();
    b.commit().unwrap();
    let warned = b.run("GRANT SELECT ON PROPERTY GRAPH H TO C").unwrap();
    assert_eq!(format!("{warned:?}"), format!("{:?}", turborel::db::Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED)));
    b.run("GRANT SELECT ON PROPERTY GRAPH J TO C WITH GRANT OPTION").unwrap();
    b.run("GRANT ALL PRIVILEGES ON PROPERTY GRAPH J TO PUBLIC").unwrap();
    b.commit().unwrap();
    refused_with(&b, "GRANT INSERT ON PROPERTY GRAPH J TO C", "SELECT alone");
    refused_with(&b, "GRANT SELECT ON PROPERTY GRAPH NOPE TO C", "GRF-0001");
    b.commit().unwrap();
    let c = db.session("C");
    c.run("GRANT SELECT ON PROPERTY GRAPH B.J TO D").unwrap();
    c.commit().unwrap();
    let d = db.session("D");
    let warned = d.run("GRANT SELECT ON PROPERTY GRAPH B.J TO E").unwrap();
    assert_eq!(format!("{warned:?}"), format!("{:?}", turborel::db::Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED)));
    d.commit().unwrap();
    assert_eq!(privileges(&db, "C"), ["B,C,J,SELECT,YES", "B,PUBLIC,J,SELECT,NO", "C,D,J,SELECT,NO"]);
    let e = b.run("REVOKE SELECT ON PROPERTY GRAPH J FROM C RESTRICT").unwrap_err();
    assert_eq!((e.sqlcode(), e.sqlstate().as_str()), (-103, "2B000"), "{e:?}");
    b.rollback().unwrap();
    b.run("REVOKE GRANT OPTION FOR SELECT ON PROPERTY GRAPH J FROM C CASCADE").unwrap();
    b.commit().unwrap();
    assert_eq!(privileges(&db, "C"), ["B,C,J,SELECT,NO", "B,PUBLIC,J,SELECT,NO"]);
    b.run("REVOKE SELECT ON PROPERTY GRAPH J FROM C, PUBLIC CASCADE").unwrap();
    let warned = b.run("REVOKE SELECT ON PROPERTY GRAPH J FROM C RESTRICT").unwrap();
    assert_eq!(format!("{warned:?}"), format!("{:?}", turborel::db::Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED)));
    b.commit().unwrap();
    assert_eq!(privileges(&db, "C"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] §20; [ISO/IEC 9075:1992 §11.37 SR13, SR18, GR3]:
/// REVOKE of SELECT on an element table from a graph's owner is refused
/// under RESTRICT, and drops the graph under CASCADE; revoking its grant
/// option abandons the grants on the graph that rest on it.
#[test]
fn a_revoke_from_the_owner_abandons_the_graph_20() {
    let db = shared();
    let b = db.session("B");
    b.run("CREATE PROPERTY GRAPH J VERTEX TABLES (A.ACCOUNT)").unwrap();
    b.run("GRANT SELECT ON PROPERTY GRAPH J TO C").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    let e = a.run("REVOKE GRANT OPTION FOR SELECT ON ACCOUNT FROM B RESTRICT").unwrap_err();
    assert!(e.msg.contains("property graph B.J"), "{e:?}");
    a.rollback().unwrap();
    a.run("REVOKE GRANT OPTION FOR SELECT ON ACCOUNT FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(privileges(&db, "B"), ["_SYSTEM,B,J,SELECT,NO"]);
    let e = a.run("REVOKE SELECT ON ACCOUNT FROM B RESTRICT").unwrap_err();
    assert!(e.msg.contains("property graph B.J"), "{e:?}");
    a.rollback().unwrap();
    a.run("REVOKE SELECT ON ACCOUNT FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&db.session("B"), "SELECT * FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS"), Vec::<String>::new());
}
