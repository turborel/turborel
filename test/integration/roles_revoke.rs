//! [ISO/IEC 9075-2:2023 §12.6, §12.7] REVOKE of roles and DROP ROLE: the
//! role authorizations identified, those abandoned, and the privileges and
//! objects that stand on them no more, refused with RESTRICT and destroyed
//! with CASCADE; and roles kept as the database is opened again.

use turborel::db::{Completion, Database};
use turborel::error::Code;
use turborel::log::FileLogOptions;

use crate::common::{Scratch, exec, failure, rows};
use crate::logs::{self, BuiltIn};
use crate::roles::{committed, direct, owned};

/// `owned`, with role R2 holding SELECT on A.T and USAGE on sequence A.Q
/// grantably, R2 granted to role R1, R1 to B; and C, as R1 by B, granted
/// both GRANTED BY CURRENT_ROLE, the grantor R1 holding them only through
/// R2 [ISO/IEC 9075-2:2023 §12.8 GR, §12.7 GR2b i 2]; C's view on A.T.
fn chained() -> Database {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R1", "CREATE ROLE R2", "GRANT R2 TO R1", "GRANT R1 TO B"]);
    committed(
        &db,
        "A",
        &["CREATE SEQUENCE Q", "GRANT SELECT ON T TO R2 WITH GRANT OPTION", "GRANT USAGE ON SEQUENCE Q TO R2 WITH GRANT OPTION"],
    );
    let b = db.session("B");
    b.run("SET ROLE 'R1'").unwrap();
    b.run("GRANT SELECT ON A.T TO C GRANTED BY CURRENT_ROLE").unwrap();
    b.run("GRANT USAGE ON SEQUENCE A.Q TO C GRANTED BY CURRENT_ROLE").unwrap();
    b.commit().unwrap();
    committed(&db, "C", &["CREATE VIEW C.V AS SELECT K FROM A.T", "CREATE TABLE C.N (K INTEGER)"]);
    db
}

/// The database in `dir`, its log there.
fn open(dir: &std::path::Path) -> (Database, turborel::db::Opened) {
    logs::open_kept(&BuiltIn(FileLogOptions::default()), dir)
}

/// Whether C reads A.T, and draws from A.Q.
fn holds(db: &Database) -> (bool, bool) {
    let c = db.session("C");
    let held = (rows(&c, "SELECT K FROM A.T").is_ok(), c.run("INSERT INTO C.N VALUES (NEXT VALUE FOR A.Q)").is_ok());
    c.rollback().unwrap();
    held
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR8, GR9, GR12q, GR12s ii]: a
/// privilege a role granted stands while it holds it grantably through its
/// applicable roles; revoking the role that brought it abandons the
/// privilege, of each kind, and what needs it: RESTRICT refuses (2B000),
/// CASCADE destroys them and the view. Revoking a role from the user who
/// granted as it abandons nothing, the grantor being the role.
#[test]
fn a_privilege_a_role_granted_falls_with_its_role_12_7() {
    let db = chained();
    committed(&db, "OWNER", &["REVOKE R1 FROM B RESTRICT"]);
    assert_eq!(holds(&db), (true, true), "R1 granted them, not B");
    let o = db.session("OWNER");
    assert_eq!(failure(o.run("REVOKE R2 FROM R1 RESTRICT")), (Code::SyntaxRule, "2B000".to_owned()), "GR12q");
    o.rollback().unwrap();
    assert_eq!(exec(&o, "REVOKE R2 FROM R1 CASCADE"), Ok(Completion::Success));
    o.commit().unwrap();
    assert_eq!(holds(&db), (false, false));
    let c = db.session("C");
    assert_eq!(rows(&c, "SELECT K FROM C.V"), Err(Code::SyntaxRule), "the view was dropped");
}

/// `owned`, with role R granted to B WITH ADMIN OPTION, and by B to C.
fn administered() -> Database {
    let db = owned();
    committed(&db, "OWNER", &["CREATE ROLE R", "GRANT R TO B WITH ADMIN OPTION"]);
    committed(&db, "B", &["GRANT R TO C"]);
    db
}

/// Whether B, and C, may set role R.
fn set(db: &Database) -> (Result<(), Code>, Result<(), Code>) {
    let set = |auth: &str| db.session(auth).run("SET ROLE 'R'").map(drop).map_err(|e| e.code);
    (set("B"), set("C"))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 AR1b, GR1b, GR6, GR7, GR12s ii]:
/// one revokes only a role it holds WITH ADMIN OPTION; ADMIN OPTION FOR
/// leaves the role, not grantable; a role authorization granted by that
/// admin option is abandoned, destroyed with CASCADE and kept with
/// RESTRICT, an independent node from then on, which a later CASCADE
/// leaves.
#[test]
fn admin_option_for_abandons_what_it_granted_12_7() {
    let db = administered();
    let c = db.session("C");
    assert_eq!(exec(&c, "REVOKE R FROM B RESTRICT"), Err(Code::Privilege), "AR1b: C holds no admin option");
    c.rollback().unwrap();
    committed(&db, "OWNER", &["REVOKE ADMIN OPTION FOR R FROM B CASCADE"]);
    assert_eq!(set(&db), (Ok(()), Err(Code::Privilege)), "CASCADE destroys C's");
    let b = db.session("B");
    assert_eq!(failure(b.run("GRANT R TO OWNER")), (Code::Privilege, "0P000".to_owned()), "B's is not grantable");
    b.rollback().unwrap();
    let db = administered();
    committed(&db, "OWNER", &["REVOKE ADMIN OPTION FOR R FROM B RESTRICT"]);
    assert_eq!(set(&db), (Ok(()), Ok(())), "RESTRICT keeps C's, abandoned");
    committed(&db, "OWNER", &["REVOKE ADMIN OPTION FOR R FROM B CASCADE"]);
    assert_eq!(set(&db), (Ok(()), Ok(())), "GR6: C's depends on no grantable one now");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.6 AR1, GR1, GR2, NOTE 652]; [DESIGN]
/// RULES.md D75: DROP ROLE needs its admin option; it is refused with
/// RESTRICT while the role granted a privilege, and for a role that owns a
/// schema; with CASCADE every grant to it and by it goes, and its name is
/// free again.
#[test]
fn drop_role_takes_every_grant_naming_it_12_6() {
    let db = chained();
    let c = db.session("C");
    assert_eq!(exec(&c, "DROP ROLE R1 CASCADE"), Err(Code::Privilege), "AR1");
    c.rollback().unwrap();
    let o = db.session("OWNER");
    assert_eq!(failure(o.run("DROP ROLE R1 RESTRICT")), (Code::SyntaxRule, "2B000".to_owned()), "R1 granted SELECT to C");
    o.rollback().unwrap();
    assert_eq!(exec(&o, "DROP ROLE R1 CASCADE"), Ok(Completion::Success));
    o.commit().unwrap();
    assert_eq!(holds(&db), (false, false));
    assert_eq!(failure(db.session("B").run("SET ROLE 'R1'")).1, "0P000", "no role R1");
    committed(&db, "OWNER", &["CREATE ROLE R1", "DROP ROLE R2 RESTRICT"]);
    committed(&db, "OWNER", &["GRANT R1 TO B"]);
    let b = db.session("B");
    b.run("SET ROLE 'R1'").unwrap();
    direct(&b, "CREATE SCHEMA AUTHORIZATION R1").unwrap();
    b.commit().unwrap();
    assert_eq!(failure(o.run("DROP ROLE R1 CASCADE")), (Code::SyntaxRule, "2B000".to_owned()), "R1 owns a schema");
}

/// Verifies: [DESIGN] RULES.md D25, D75: roles, their authorizations, the
/// right to create them, a schema a role owns, defined there as the role,
/// and a privilege GRANTED BY CURRENT_ROLE, the log keeping the role
/// beside each definition, are kept: replayed from the log after stopping without a clean
/// close, and read from a checkpoint after one.
#[test]
fn roles_are_kept_as_the_database_opens_again() {
    let scratch = Scratch::new("roles-kept");
    let dir = scratch.path("db");
    let state = |db: &Database| {
        let b = db.session("B");
        b.run("SET ROLE 'R'").unwrap();
        let shown = (
            rows(&b, "SELECT K FROM R.L").unwrap(),
            rows(&db.session("OWNER"), "SELECT USER_NAME, CREATE_ROLE FROM INFORMATION_SCHEMA.USERS WHERE USER_NAME = 'B'").unwrap(),
            rows(&db.session("OWNER"), "SELECT K FROM R.L").unwrap(),
        );
        b.rollback().unwrap();
        shown
    };
    let expected = (vec!["1".to_owned()], vec!["B,YES".to_owned()], vec!["1".to_owned()]);
    let (db, _) = open(&dir);
    for user in ["OWNER", "B"] {
        db.create_user(user, b"x").unwrap();
    }
    db.checkpoint().unwrap();
    // Each after the checkpoint, so replayed.
    db.allow_role_creator("B", true).unwrap();
    committed(&db, "B", &["CREATE ROLE R"]);
    let b = db.session("B");
    b.run("SET ROLE 'R'").unwrap();
    direct(&b, "CREATE SCHEMA AUTHORIZATION R").unwrap();
    for sql in ["CREATE TABLE R.L (K INTEGER)", "INSERT INTO R.L VALUES (1)", "GRANT SELECT ON R.L TO OWNER GRANTED BY CURRENT_ROLE"] {
        b.run(sql).unwrap();
    }
    b.commit().unwrap();
    assert_eq!(state(&db), expected);
    let (recovered, _) = open(&dir);
    assert_eq!(state(&recovered), expected, "replayed from the log");
    drop(db);
    recovered.checkpoint().unwrap();
    recovered.close().unwrap();
    drop(recovered);
    let (reopened, _) = open(&dir);
    assert_eq!(state(&reopened), expected, "read from the checkpoint");
}
