//! \[EXTENSION\] [ISO/IEC 9075:1992 §11.37] REVOKE, in a transaction
//! (RULES.md D47): the privileges it identifies destroyed, or their grant
//! option; what then stands no more abandoned, which RESTRICT refuses and
//! CASCADE destroys: privileges, views and constraints.

use turborel::db::{Completion, Database};
use turborel::error::{Code, SqlState};

use crate::common::{code, exec, rows};

/// Schema A with table T, holding 1; schemas B and C, empty.
fn owned() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY)").unwrap();
    for auth in ["B", "C", "D"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {auth}")).unwrap();
    }
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.commit().unwrap();
    db
}

/// Runs `statements` as `auth`, and commits.
fn committed(db: &Database, auth: &str, statements: &[&str]) {
    let s = db.session(auth);
    for sql in statements {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
}

/// Whether `auth` may read A.T.
fn reads(db: &Database, auth: &str) -> bool {
    rows(&db.session(auth), "SELECT K FROM A.T").is_ok()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR6, SR11, SR18, GR1]
/// a revoke destroys what its revoker granted; a privilege granted by one
/// who held it only through the one revoked is abandoned: RESTRICT
/// refuses, CASCADE destroys it too.
#[test]
fn what_stands_on_a_privilege_revoked_goes_with_it_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT ON T TO B WITH GRANT OPTION"]);
    committed(&db, "B", &["GRANT SELECT ON A.T TO C"]);
    let a = db.session("A");
    assert_eq!(exec(&a, "REVOKE SELECT ON T FROM B RESTRICT"), Err(Code::SyntaxRule), "SR18: B's grant to C");
    assert_eq!(exec(&a, "REVOKE SELECT ON T FROM B CASCADE"), Ok(Completion::Success));
    assert!(reads(&db, "B") && reads(&db, "C"), "until it commits");
    a.commit().unwrap();
    assert!(!reads(&db, "B") && !reads(&db, "C"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR9-SR11] a privilege
/// granted by one who holds it another way stands; privileges that grant
/// each other in a cycle, once nothing else holds them up, fall together.
#[test]
fn what_stands_another_way_stays_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT ON T TO B WITH GRANT OPTION", "GRANT SELECT ON T TO D WITH GRANT OPTION"]);
    committed(&db, "B", &["GRANT SELECT ON A.T TO C WITH GRANT OPTION"]);
    committed(&db, "D", &["GRANT SELECT ON A.T TO C"]);
    committed(&db, "C", &["GRANT SELECT ON A.T TO B WITH GRANT OPTION"]);
    committed(&db, "A", &["REVOKE SELECT ON T FROM B CASCADE"]);
    assert!(reads(&db, "C"), "D's grant");
    assert!(!reads(&db, "B"), "C's grant to B stood on B's own");
    committed(&db, "A", &["REVOKE SELECT ON T FROM D CASCADE"]);
    assert!(!reads(&db, "C") && !reads(&db, "D"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 GR2] GRANT OPTION FOR
/// takes the grant option, not the privilege: RESTRICT refuses while a
/// privilege granted by it stands (SQLSTATE 2B000); CASCADE destroys it.
#[test]
fn grant_option_for_takes_the_option_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT ON T TO B WITH GRANT OPTION"]);
    committed(&db, "B", &["GRANT SELECT ON A.T TO C"]);
    let a = db.session("A");
    let refused = a.execute("REVOKE GRANT OPTION FOR SELECT ON T FROM B RESTRICT", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!(refused.sqlstate(), SqlState::DEPENDENT_PRIVILEGES);
    assert_eq!(exec(&a, "REVOKE GRANT OPTION FOR SELECT ON T FROM B CASCADE"), Ok(Completion::Success));
    a.commit().unwrap();
    assert!(reads(&db, "B") && !reads(&db, "C"));
    assert_eq!(exec(&db.session("B"), "GRANT SELECT ON A.T TO C"), Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR7a ii, SR18, GR2b] a
/// grant to PUBLIC WITH GRANT OPTION lets anyone grant the privilege, and
/// what they grant stands on it: revoked, RESTRICT refuses, GRANT OPTION
/// FOR with RESTRICT too (SQLSTATE 2B000), CASCADE takes it.
#[test]
fn a_grant_option_given_to_public_is_everyones_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT ON T TO PUBLIC WITH GRANT OPTION"]);
    committed(&db, "B", &["GRANT SELECT ON A.T TO C"]);
    let a = db.session("A");
    assert_eq!(exec(&a, "REVOKE SELECT ON T FROM PUBLIC RESTRICT"), Err(Code::SyntaxRule), "SR18: B's grant to C");
    let refused = a.execute("REVOKE GRANT OPTION FOR SELECT ON T FROM PUBLIC RESTRICT", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!(refused.sqlstate(), SqlState::DEPENDENT_PRIVILEGES);
    assert_eq!(exec(&a, "REVOKE GRANT OPTION FOR SELECT ON T FROM PUBLIC CASCADE"), Ok(Completion::Success));
    a.commit().unwrap();
    assert!(reads(&db, "B") && reads(&db, "C"), "PUBLIC's SELECT stays");
    let warned = Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED));
    assert_eq!(exec(&db.session("B"), "GRANT SELECT ON A.T TO C"), warned, "the option is gone");
    committed(&db, "A", &["REVOKE SELECT ON T FROM PUBLIC CASCADE"]);
    assert!(!reads(&db, "B") && !reads(&db, "C"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR13, SR14, SR18, GR3,
/// GR4] a view whose owner would no longer read a table it names, and a
/// constraint whose owner would no longer hold the REFERENCES it needs,
/// are abandoned: RESTRICT refuses, CASCADE drops them; a view over a view
/// dropped goes with it.
#[test]
fn views_and_constraints_abandoned_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT, REFERENCES ON T TO B"]);
    committed(
        &db,
        "B",
        &["CREATE VIEW V AS SELECT K FROM A.T", "CREATE VIEW W AS SELECT K FROM V", "CREATE TABLE U (K INTEGER REFERENCES A.T)"],
    );
    let a = db.session("A");
    assert_eq!(exec(&a, "REVOKE SELECT ON T FROM B RESTRICT"), Err(Code::SyntaxRule), "view V");
    assert_eq!(exec(&a, "REVOKE REFERENCES ON T FROM B RESTRICT"), Err(Code::SyntaxRule), "U's foreign key");
    assert_eq!(exec(&a, "REVOKE ALL PRIVILEGES ON T FROM B CASCADE"), Ok(Completion::Success));
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(code(&b, "SELECT K FROM W"), Code::SyntaxRule, "dropped with V");
    assert_eq!(exec(&b, "INSERT INTO U VALUES (9)"), Ok(Completion::Success), "no foreign key");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 AR1, GR8, GR9] a revoker
/// holds a privilege on the object; what it names and did not grant is not
/// revoked, with a warning, as is ALL PRIVILEGES that finds none.
#[test]
fn what_is_not_revoked_is_warned_of_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT ON T TO B"]);
    let a = db.session("A");
    let warned = Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED));
    assert_eq!(exec(&a, "REVOKE SELECT, DELETE ON T FROM B CASCADE"), warned, "no DELETE");
    assert_eq!(exec(&a, "REVOKE ALL PRIVILEGES ON T FROM C CASCADE"), warned, "none of C's");
    a.commit().unwrap();
    assert_eq!(exec(&db.session("C"), "REVOKE SELECT ON A.T FROM B CASCADE"), Err(Code::Privilege), "AR1");
    assert!(!reads(&db, "B"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.37 SR14-SR16, SR18,
/// GR4-GR6] REVOKE REFERENCES abandons the assertions, domain constraints
/// and table constraints, through a view too, that need it: RESTRICT
/// refuses, CASCADE drops each, a domain's constraint with its copies.
#[test]
fn revoke_abandons_assertions_and_domain_constraints_11_37() {
    let db = owned();
    committed(&db, "A", &["GRANT SELECT, REFERENCES (K) ON T TO B"]);
    committed(
        &db,
        "B",
        &[
            "CREATE ASSERTION KNOWN CHECK (EXISTS (SELECT * FROM A.T WHERE K = 1))",
            "CREATE DOMAIN D AS INTEGER CONSTRAINT LISTED CHECK (VALUE IN (SELECT K FROM A.T))",
            "CREATE TABLE BT (J D)",
            "CREATE VIEW W AS SELECT K FROM A.T",
            "CREATE TABLE BU (J INTEGER CONSTRAINT VIA CHECK (J IN (SELECT K FROM W)))",
        ],
    );
    let (a, b) = (db.session("A"), db.session("B"));
    assert_eq!(exec(&b, "INSERT INTO BT VALUES (2)").err(), Some(Code::Constraint), "LISTED");
    b.rollback().unwrap();
    assert_eq!(exec(&a, "REVOKE REFERENCES (K) ON T FROM B RESTRICT").err(), Some(Code::SyntaxRule), "SR18");
    a.run("REVOKE REFERENCES (K) ON T FROM B CASCADE").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO BT VALUES (2)").unwrap();
    b.run("INSERT INTO BU VALUES (2)").unwrap();
    assert_eq!(exec(&b, "DROP ASSERTION KNOWN").err(), Some(Code::SyntaxRule), "KNOWN went");
    assert_eq!(exec(&b, "ALTER DOMAIN D DROP CONSTRAINT LISTED").err(), Some(Code::SyntaxRule), "LISTED went");
    assert_eq!(rows(&b, "SELECT K FROM W"), Ok(vec!["1".into()]), "W stays: B holds SELECT");
    b.commit().unwrap();
}
