//! Privileges [X3.135-1986 §4.15, §6.6]: what a `<privilege definition>`
//! grants, who may grant what, and the statements that need them
//! [§5.24 SR1, §5.25 SR1, §8.4 SR1, §8.5 SR1, §8.7 SR1, §8.11 SR1, §8.12
//! SR1].

use turborel::db::{Completion, Database};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{code, exec, rows, setup};

/// `setup`, and schema B, whose table P grants APP SELECT; INSERT and
/// UPDATE of V, grantable; and PUBLIC DELETE, grantable, which makes it
/// grantable by anyone [ISO/IEC 9075:1992 §10.3 GR2, §11.37 SR7a ii].
/// G holds SELECT, and
/// INSERT and UPDATE of V with the grant option. Z has a table of its own.
fn granted() -> Database {
    let db = setup();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
           CREATE TABLE P (K INTEGER NOT NULL UNIQUE, V INTEGER, W INTEGER)
           GRANT SELECT ON P TO APP, G
           GRANT INSERT, UPDATE (V) ON P TO APP, G WITH GRANT OPTION
           GRANT DELETE ON P TO PUBLIC WITH GRANT OPTION",
    )
    .unwrap();
    db.session("B").run("INSERT INTO P VALUES (1, 10, 100)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION Z CREATE TABLE ZT (X INTEGER)").unwrap();
    db.session("Z").run("INSERT INTO ZT VALUES (1)").unwrap();
    db
}

/// Verifies: [X3.135-1986 §6.6 SR3b] the owner of a table holds every
/// privilege on it; anyone else those granted to it or to PUBLIC; each
/// statement needs the privilege its SR1 names: SELECT on each table read,
/// in a subquery too [§5.24 SR1, §5.25 SR1], INSERT [§8.7 SR1], DELETE
/// [§8.5 SR1, §8.4 SR1], UPDATE on each object column [§8.12 SR1, §8.11
/// SR1].
#[test]
fn statements_need_their_privileges_6_6() {
    let db = granted();
    let app = db.session("APP");
    assert_eq!(rows(&app, "SELECT K, V FROM B.P"), Ok(vec!["1,10".into()]));
    assert_eq!(exec(&app, "INSERT INTO B.P (K, V) VALUES (2, 20)"), Ok(Completion::Success));
    assert_eq!(exec(&app, "UPDATE B.P SET V = V + 1 WHERE K = 2"), Ok(Completion::Success));
    assert_eq!(exec(&app, "UPDATE B.P SET W = 1 WHERE K = 2"), Err(Code::Privilege), "UPDATE of V only");
    assert_eq!(exec(&app, "UPDATE B.P SET V = 1, W = 1"), Err(Code::Privilege));
    assert_eq!(exec(&app, "DELETE FROM B.P WHERE K = 2"), Ok(Completion::Success), "PUBLIC");
    assert_eq!(exec(&app, "INSERT INTO T (ID) SELECT K + 10 FROM B.P"), Ok(Completion::Success));

    let z = db.session("Z");
    assert_eq!(code(&z, "SELECT K FROM B.P"), Code::Privilege);
    assert_eq!(code(&z, "SELECT X FROM ZT WHERE EXISTS (SELECT * FROM B.P)"), Code::Privilege, "§5.24 SR1");
    assert_eq!(exec(&z, "INSERT INTO B.P VALUES (3, 3, 3)"), Err(Code::Privilege));
    assert_eq!(exec(&z, "INSERT INTO ZT SELECT K FROM B.P"), Err(Code::Privilege));
    assert_eq!(exec(&z, "UPDATE B.P SET V = 0"), Err(Code::Privilege));
    assert_eq!(exec(&z, "DELETE FROM ZT WHERE X IN (SELECT K FROM B.P)"), Err(Code::Privilege));
    assert_eq!(exec(&z, "DELETE FROM B.P WHERE K = 99"), Ok(Completion::NoData), "PUBLIC");
    assert_eq!(code(&db.session("APP"), "SELECT X FROM Z.ZT"), Code::Privilege, "nothing granted");

    let owner = db.session("B");
    assert_eq!(exec(&owner, "UPDATE P SET W = 0"), Ok(Completion::Success));
    assert_eq!(rows(&owner, "SELECT K, V, W FROM P"), Ok(vec!["1,10,0".into()]));
}

/// Verifies: [X3.135-1986 §8.11 SR1, §8.4 SR1] a positioned UPDATE needs
/// UPDATE on each object column, and a positioned DELETE needs DELETE,
/// whatever the cursor could read.
#[test]
fn positioned_changes_need_their_privileges_8_11_8_4() {
    let db = granted();
    let s = db.session("APP");
    let mut p = Params::new().declare("K_OUT", DataType::Exact { precision: 9, scale: 0 }, Value::Null);
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM B.P").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    assert_eq!(s.execute("UPDATE B.P SET W = 5 WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Privilege);
    s.execute("UPDATE B.P SET V = 5 WHERE CURRENT OF C", &mut p).unwrap();
    s.execute("DELETE FROM B.P WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT K FROM B.P"), Ok(vec![]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.36 SR3, AR1, GR14, GR15,
/// §10.3 SR5, GR2]; [X3.135-1986 §4.15] a grant gives only privileges its
/// grantor holds WITH GRANT OPTION, by name or as PUBLIC's, where SQL-86
/// took only those by name [§6.6 SR3b ii]; it completes with a warning,
/// "privilege not granted", for each it names but cannot give, or for ALL
/// PRIVILEGES when it gives none; a grantor that holds nothing on the
/// table is refused. In a `<schema>` too, which SQL-86 refused instead
/// [X3.135-1986 §6.6 SR5].
#[test]
fn only_held_grantable_privileges_are_granted_11_36() {
    let db = granted();
    let warned = Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED));
    let schema = |sql: &str| db.schema(sql).map_err(|e| e.code);
    assert_eq!(schema("CREATE SCHEMA AUTHORIZATION G GRANT SELECT, INSERT ON B.P TO H"), warned, "SELECT is not grantable");
    let g = db.session("G");
    assert_eq!(exec(&g, "GRANT DELETE ON B.P TO H"), Ok(Completion::Success), "PUBLIC's option is everyone's");
    assert_eq!(exec(&g, "GRANT UPDATE ON B.P TO H"), warned, "UPDATE (V) only");
    assert_eq!(exec(&g, "GRANT SELECT ON Z.ZT TO H"), Err(Code::Privilege), "AR1: nothing held");
    assert_eq!(exec(&g, "GRANT ALL PRIVILEGES ON B.P TO H2"), Ok(Completion::Success), "§10.3 SR5: all it can give");
    g.commit().unwrap();
    let z = db.session("Z");
    assert_eq!(exec(&z, "GRANT DELETE, INSERT ON B.P TO H"), warned, "PUBLIC's DELETE only");
    z.rollback().unwrap();
    let h = db.session("H");
    assert_eq!(exec(&h, "INSERT INTO B.P VALUES (7, 7, 7)"), Ok(Completion::Success));
    // [§8.12 SR1] UPDATE of the object columns is all it needs: the WHERE
    // clause's columns need no SELECT.
    assert_eq!(exec(&h, "UPDATE B.P SET V = 8 WHERE K = 7"), Ok(Completion::Success));
    assert_eq!(code(&h, "SELECT K FROM B.P"), Code::Privilege);
    assert_eq!(exec(&h, "DELETE FROM B.P WHERE K = 99"), Ok(Completion::NoData), "PUBLIC's, and G's");
    h.commit().unwrap();
    let h2 = db.session("H2");
    assert_eq!(exec(&h2, "UPDATE B.P SET V = 9 WHERE K = 1"), Ok(Completion::Success));
    assert_eq!(exec(&h2, "UPDATE B.P SET W = 9 WHERE K = 1"), Err(Code::Privilege));
}

/// Verifies: [X3.135-1986 §6.6 SR1, SR2, SR4] the table of a grant exists,
/// in the schema or before it; a `<grant column list>` names its columns;
/// UPDATE without one is every column, and ALL PRIVILEGES every privilege
/// the grantor holds; the `<privilege definition>` follows its Format.
/// [ISO/IEC 9075:1992 §11.36 AR1] One who holds nothing on the table is
/// refused.
#[test]
fn privilege_definitions_6_6() {
    let db = setup();
    let fails = |sql: &str| db.schema(sql).map(drop).map_err(|e| e.code);
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E GRANT SELECT ON NOPE TO APP"), Err(Code::SyntaxRule), "§5.4 SR5");
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E CREATE TABLE Q (X INT) GRANT UPDATE (Y) ON Q TO APP"), Err(Code::SyntaxRule));
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E CREATE TABLE Q (X INT) GRANT SELECT ON Q TO APP WITH OPTION"), Err(Code::Syntax));
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E CREATE TABLE Q (X INT) GRANT ON Q TO APP"), Err(Code::Syntax));
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E CREATE TABLE Q (X INT) GRANT ALL ON Q TO APP"), Err(Code::Syntax));
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION E CREATE TABLE Q (X INT) GRANT SELECT ON Q TO PUBLIC, "), Err(Code::Syntax));
    db.schema("CREATE SCHEMA AUTHORIZATION E GRANT ALL PRIVILEGES ON Q TO APP CREATE TABLE Q (X INT) GRANT UPDATE ON Q TO F").unwrap();
    let app = db.session("APP");
    assert_eq!(exec(&app, "INSERT INTO E.Q VALUES (1)"), Ok(Completion::Success));
    assert_eq!(exec(&app, "UPDATE E.Q SET X = 2"), Ok(Completion::Success));
    assert_eq!(exec(&app, "DELETE FROM E.Q"), Ok(Completion::Success));
    assert_eq!(rows(&app, "SELECT X FROM E.Q"), Ok(vec![]));
    assert_eq!(exec(&db.session("F"), "UPDATE E.Q SET X = 3"), Ok(Completion::NoData));
    assert_eq!(fails("CREATE SCHEMA AUTHORIZATION APP2 GRANT SELECT ON E.Q TO E"), Err(Code::Privilege), "APP2 holds nothing");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.3 GR4, §13.8 AR1, §11.11
/// GR3] INSERT may be granted on columns: an INSERT then names those alone,
/// listed or implicit, else is refused (SQLCODE -104); the Information
/// Schema gives it per column; INSERT granted on the table extends to a
/// column added later, INSERT on columns does not; it is revoked as
/// granted.
#[test]
fn insert_on_columns_10_3() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE T (A INTEGER, B INTEGER)
           GRANT INSERT (A) ON T TO P
           GRANT INSERT ON T TO Q",
    )
    .unwrap();
    let p = db.session("P");
    p.run("INSERT INTO O.T (A) VALUES (1)").unwrap();
    assert_eq!(exec(&p, "INSERT INTO O.T (A, B) VALUES (1, 2)").err(), Some(Code::Privilege), "B not granted");
    assert_eq!(exec(&p, "INSERT INTO O.T VALUES (1, 2)").err(), Some(Code::Privilege), "every column, implicit");
    p.commit().unwrap();
    let privileges = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMN_PRIVILEGES WHERE GRANTEE = 'P' AND PRIVILEGE_TYPE = 'INSERT'";
    assert_eq!(rows(&p, privileges).map(|r| r.iter().map(|c| c.trim_end().to_owned()).collect::<Vec<_>>()), Ok(vec!["A".to_owned()]));
    let o = db.session("O");
    o.run("ALTER TABLE T ADD COLUMN C INTEGER").unwrap();
    o.commit().unwrap();
    db.session("Q").run("INSERT INTO O.T (C) VALUES (3)").unwrap();
    assert_eq!(exec(&p, "INSERT INTO O.T (C) VALUES (3)").err(), Some(Code::Privilege), "not P's");
    o.run("REVOKE INSERT (A) ON T FROM P RESTRICT").unwrap();
    o.commit().unwrap();
    assert_eq!(exec(&p, "INSERT INTO O.T (A) VALUES (1)").err(), Some(Code::Privilege), "revoked");
    assert_eq!(code(&p, "SELECT A FROM O.T"), Code::Privilege);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.19 GR6b; §11.9 AR1;
/// §11.34 AR2] a view's owner holds REFERENCES on each column of it drawn
/// only from columns it holds REFERENCES on, so a CHECK or an assertion
/// reads the view; grantable only if those are; none on a view drawn from
/// a derived table, what it is drawn from not being known. In a `<schema
/// definition>`, a CHECK reads a view it defines [§11.1 GR3].
#[test]
fn references_on_a_view_11_19() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, N INTEGER)
           CREATE VIEW V AS SELECT K, N + 1 AS M FROM T
           CREATE VIEW X AS SELECT K FROM (SELECT K FROM T) AS D
           CREATE TABLE U (K INTEGER CHECK (K IN (SELECT K FROM V)))
           GRANT SELECT, REFERENCES (K) ON T TO B",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 1)").unwrap();
    a.run("INSERT INTO U VALUES (1)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO U VALUES (2)").err(), Some(Code::Constraint), "the CHECK reads V");
    a.run("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM V WHERE M > 9))").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (9, 9)").err(), Some(Code::Constraint), "SMALL reads V");
    assert_eq!(exec(&a, "CREATE ASSERTION OF_X CHECK (NOT EXISTS (SELECT * FROM X))").err(), Some(Code::Privilege), "X");
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE VIEW W AS SELECT K, N FROM A.T").unwrap();
    let b = db.session("B");
    b.run("CREATE TABLE BT (J INTEGER CHECK (J IN (SELECT K FROM W)))").unwrap();
    assert_eq!(exec(&b, "CREATE TABLE BU (J INTEGER CHECK (J IN (SELECT N FROM W)))").err(), Some(Code::Privilege), "not of N");
    let granted = exec(&b, "GRANT REFERENCES (K) ON W TO C");
    assert_eq!(granted, Ok(Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED)), "not grantable");
    b.commit().unwrap();
}
