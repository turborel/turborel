//! [ISO/IEC 9075-2:2023 §19.2, §19.3, §6.4, §4.42.1.2] A session's role:
//! SET ROLE, what CURRENT_USER, CURRENT_ROLE and SESSION_USER give in a
//! statement, a trigger and a plan kept, and a CURRENT_ROLE default.

use turborel::db::{Completion, Database};
use turborel::error::Code;

use crate::common::{exec, failure, rows};
use crate::roles::{committed, direct, owned};

/// `owned`, with roles R, granted to B, and P, granted to PUBLIC; R holding
/// SELECT on A.T.
fn granted() -> Database {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R", "CREATE ROLE P", "GRANT R TO B", "GRANT P TO PUBLIC"]);
    committed(&db, "A", &["GRANT SELECT ON T TO R"]);
    db
}

/// What the user value functions give, as `CURRENT_USER,CURRENT_ROLE,SESSION_USER`.
fn who(s: &turborel::db::Session) -> String {
    let shown = rows(s, "VALUES (CURRENT_USER, CURRENT_ROLE, SESSION_USER)").unwrap().concat();
    s.commit().unwrap();
    shown
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §19.3 GR1-GR4, §4.42.4, §6.4 GR6-GR8]:
/// SET ROLE enables a role granted to the user or to PUBLIC, its privileges
/// then the statement's, and NONE removes it; it is refused in a
/// transaction (25001) and for a role granted neither (0P000); its value is
/// trimmed.
#[test]
fn set_role_enables_a_role_granted_19_3() {
    let db = granted();
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT K FROM A.T"), Err(Code::Privilege), "R is not enabled");
    b.rollback().unwrap();
    b.run("SET ROLE ' R '").unwrap();
    assert_eq!(who(&b), "B,R,B");
    assert_eq!(rows(&b, "SELECT K FROM A.T").unwrap(), ["1"]);
    assert_eq!(failure(b.run("SET ROLE NONE")), (Code::TransactionState, "25001".to_owned()), "GR1");
    b.commit().unwrap();
    b.run("SET ROLE NONE").unwrap();
    assert_eq!(who(&b), "B,NULL,B");
    let c = db.session("C");
    assert_eq!(failure(c.run("SET ROLE 'R'")), (Code::Privilege, "0P000".to_owned()), "GR4: not C's");
    assert_eq!(failure(c.run("SET ROLE 'NONE_SUCH'")), (Code::Privilege, "0P000".to_owned()), "GR3: no role");
    assert_eq!(c.run("SET ROLE 'P'").map_err(|e| e.code), Ok(Completion::Success), "GR4: PUBLIC's");
    assert_eq!(who(&c), "C,P,C");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.42.4]: a role granted to the role a
/// session sets is applicable, and its privileges current, through a chain.
#[test]
fn a_role_granted_to_a_role_is_applicable_4_42_4() {
    let db = granted();
    committed(&db, "OWNER", &["CREATE ROLE Q", "GRANT R TO Q", "GRANT Q TO C"]);
    let c = db.session("C");
    c.run("SET ROLE 'Q'").unwrap();
    assert_eq!(rows(&c, "SELECT K FROM A.T").unwrap(), ["1"], "R through Q");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §19.2 GR5, GR8]: SET SESSION
/// AUTHORIZATION removes the current role, and takes no role's name.
#[test]
fn set_session_authorization_removes_the_role_19_2() {
    let db = granted();
    committed(&db, "OWNER", &["GRANT R TO OWNER"]);
    let o = db.session("OWNER");
    o.run("SET ROLE 'R'").unwrap();
    o.run("SET SESSION AUTHORIZATION 'C'").unwrap();
    assert_eq!(who(&o), "C,NULL,C");
    let o = db.session("OWNER");
    assert_eq!(failure(o.run("SET SESSION AUTHORIZATION 'R'")), (Code::Privilege, "28000".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.21 GR6b, §4.42.1.2, §6.4 GR6-GR8]:
/// a triggered action runs as its schema's owner alone, a role that owns
/// it there: CURRENT_USER null, CURRENT_ROLE the role; SESSION_USER stays
/// the session's. [§12.8 GR] A grant names the role its grantor by
/// GRANTED BY CURRENT_ROLE, the current user being it otherwise.
///
/// Regression test (R-TEST-7): SESSION_USER in a triggered action gave the
/// trigger's owner [§6.4 GR7].
#[test]
fn a_trigger_of_a_role_runs_as_the_role_alone_15_21() {
    let db = granted();
    let b = db.session("B");
    b.run("SET ROLE 'R'").unwrap();
    direct(&b, "CREATE SCHEMA AUTHORIZATION R").unwrap();
    b.run("CREATE TABLE R.T (K INTEGER)").unwrap();
    b.run("CREATE TABLE R.L (U VARCHAR(128), R VARCHAR(128), S VARCHAR(128))").unwrap();
    let action = "INSERT INTO R.L VALUES (CURRENT_USER, CURRENT_ROLE, SESSION_USER)";
    b.run(&format!("CREATE TRIGGER R.W AFTER INSERT ON R.T FOR EACH STATEMENT {action}")).unwrap();
    b.run("GRANT INSERT ON R.T TO C GRANTED BY CURRENT_ROLE").unwrap();
    b.commit().unwrap();
    committed(&db, "C", &["INSERT INTO R.T VALUES (1)"]);
    assert_eq!(rows(&b, "SELECT U, R, S FROM R.L").unwrap(), ["NULL,R,C"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 SR4b, GR, §11.9 SR5, §6.4 GR8]: a
/// CURRENT_ROLE default gives the role the session set, null with none, of
/// a character column of 128 characters or more; a CHECK may not read it;
/// a column of that name, qualified, is a column.
#[test]
fn current_role_as_a_default_11_5() {
    let db = granted();
    let b = db.session("B");
    b.run("CREATE TABLE B.D (K INTEGER, W VARCHAR(128) DEFAULT CURRENT_ROLE)").unwrap();
    assert_eq!(exec(&b, "CREATE TABLE B.E (W VARCHAR(10) DEFAULT CURRENT_ROLE)"), Err(Code::SyntaxRule), "SR4b");
    assert_eq!(exec(&b, "CREATE TABLE B.F (W VARCHAR(128) CHECK (W = CURRENT_ROLE))"), Err(Code::SyntaxRule), "§11.9 SR5");
    b.run("CREATE TABLE B.N (CURRENT_ROLE INTEGER)").unwrap();
    b.run("INSERT INTO B.N VALUES (7)").unwrap();
    b.run("INSERT INTO B.D (K) VALUES (1)").unwrap();
    b.commit().unwrap();
    b.run("SET ROLE 'R'").unwrap();
    b.run("INSERT INTO B.D (K) VALUES (2)").unwrap();
    assert_eq!(rows(&b, "SELECT K, W FROM B.D ORDER BY K").unwrap(), ["1,NULL", "2,R"]);
    assert_eq!(rows(&b, "SELECT N.CURRENT_ROLE FROM B.N N").unwrap(), ["7"]);
}

/// Verifies: [DESIGN] RULES.md D45, D75: a prepared statement's plan
/// holding a user value is kept for one role only: made again once the
/// session sets another.
#[test]
fn a_plan_is_kept_for_the_role_it_was_made_for_19_3() {
    let db = granted();
    let b = db.session("B");
    let p = b.prepare("VALUES (CURRENT_ROLE)").unwrap();
    let run = |b: &turborel::db::Session| {
        let mut c = b.open_prepared(&p, &[]).unwrap();
        let shown = c.fetch().unwrap().unwrap().iter().map(ToString::to_string).collect::<String>();
        drop(c);
        b.commit().unwrap();
        shown
    };
    assert_eq!(run(&b), "NULL");
    b.run("SET ROLE 'R'").unwrap();
    assert_eq!(run(&b), "R");
}
