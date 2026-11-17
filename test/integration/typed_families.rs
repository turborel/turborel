//! Subtable families [ISO/IEC 9075-2:2023 §4.17.9, §7.6 GR3b, AR1b ii 2;
//! §11.3 GR8; §11.11 SR2; §11.17; §11.18; §11.23 SR3; §12.2 SR3, GR8;
//! §12.7; §14.8 SR9; §14.9 AR1b ii; §14.10 GR5; §14.12; §14.13 SR9;
//! §14.15 SR3]: a typed table read with its subtables' rows, its
//! constraints holding of them, its cursors and MERGE over them, TRUNCATE
//! of the family or ONLY the table, SELECT WITH HIERARCHY OPTION, a
//! reference column's scope added and dropped, and the statements a
//! referenceable table refuses.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{Scratch, after_crash, rows};

/// Schema A: PERSON of NAME and AGE; EMP under it of PAY; PEOPLE of
/// PERSON, UNIQUE (NAME) and CHECK (AGE > 0); STAFF of EMP under PEOPLE;
/// ANN and BOB in PEOPLE, CAT and DAN in STAFF. Schema B.
fn family() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20), AGE INTEGER) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED, UNIQUE (NAME), CHECK (AGE > 0))",
            "CREATE TABLE STAFF OF EMP UNDER PEOPLE",
            "INSERT INTO PEOPLE (NAME, AGE) VALUES ('ANN', 30), ('BOB', 40)",
            "INSERT INTO STAFF (NAME, AGE, PAY) VALUES ('CAT', 50, 100), ('DAN', 60, 200)",
        ],
    );
    (db, a)
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
/// SQLCODE class `code`, SQLSTATE `state` and a message citing `cited`;
/// rolls back.
fn refused(s: &Session, sql: &str, (code, state, cited): (Code, &str, &str)) {
    let e = if sql.starts_with("SELECT") { s.open_cursor(sql).err() } else { s.run(sql).err() };
    let e = e.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// A Syntax Rule's refusal citing `cited`.
fn syntax(cited: &str) -> (Code, &str, &str) {
    (Code::SyntaxRule, "2A000", cited)
}

/// A privilege's refusal citing `cited`.
fn privilege(cited: &str) -> (Code, &str, &str) {
    (Code::Privilege, "2A000", cited)
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.17.9, §7.6 GR3b, §14.9 GR3, §14.14
/// GR6] A typed table read without ONLY has its subtables' rows, with ONLY
/// its own alone; a searched UPDATE or DELETE of it changes its
/// subtables' rows too, and with ONLY its own alone.
#[test]
fn a_table_is_read_with_its_subtables_7_6() {
    let (_db, a) = family();
    assert_eq!(query(&a, "SELECT NAME FROM PEOPLE ORDER BY NAME"), ["ANN", "BOB", "CAT", "DAN"]);
    assert_eq!(query(&a, "SELECT NAME FROM ONLY (PEOPLE) ORDER BY NAME"), ["ANN", "BOB"]);
    assert_eq!(query(&a, "SELECT NAME, PAY FROM STAFF ORDER BY NAME"), ["CAT,100", "DAN,200"]);
    ok(&a, &["UPDATE PEOPLE SET AGE = AGE + 1", "UPDATE ONLY (PEOPLE) SET AGE = AGE + 1", "DELETE FROM ONLY (PEOPLE) WHERE NAME = 'BOB'"]);
    assert_eq!(query(&a, "SELECT NAME, AGE FROM PEOPLE ORDER BY NAME"), ["ANN,32", "CAT,51", "DAN,61"]);
    ok(&a, &["DELETE FROM PEOPLE WHERE NAME = 'CAT'"]);
    assert_eq!(query(&a, "SELECT NAME FROM STAFF"), ["DAN"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.17.9, §11.7, §11.8 GR] The
/// constraints of a supertable hold of its subtables' rows: UNIQUE over
/// the family, the CHECK, and a foreign key that references the
/// supertable, matched by and cascaded from a subtable's row.
#[test]
fn supertable_constraints_hold_of_subtable_rows_4_17_9() {
    let (_db, a) = family();
    ok(&a, &["CREATE TABLE KIDS (NAME CHARACTER VARYING(20) REFERENCES PEOPLE (NAME) ON DELETE CASCADE)"]);
    let violated = |cited| (Code::Constraint, "23000", cited);
    refused(&a, "INSERT INTO STAFF (NAME, AGE, PAY) VALUES ('ANN', 1, 1)", violated("(NAME)"));
    refused(&a, "INSERT INTO PEOPLE (NAME, AGE) VALUES ('CAT', 1)", violated("(NAME)"));
    refused(&a, "INSERT INTO STAFF (NAME, AGE, PAY) VALUES ('EVE', -1, 1)", violated("§11.6 GR3"));
    refused(&a, "INSERT INTO KIDS VALUES ('ZED')", violated("§11.8 GR2"));
    ok(&a, &["INSERT INTO KIDS VALUES ('ANN'), ('CAT')", "DELETE FROM STAFF WHERE NAME = 'CAT'"]);
    assert_eq!(query(&a, "SELECT NAME FROM KIDS"), ["ANN"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.8 SR9, §14.13 SR9] A cursor over a
/// family reads its rows in order; a positioned UPDATE or DELETE changes
/// the row of whichever table it is of; the target table is ONLY just
/// when the cursor's table reference is (SQLCODE -103).
#[test]
fn a_cursor_over_a_family_changes_each_row_where_it_is_14_13() {
    let (_db, a) = family();
    let mut p = Params::new().declare("N", DataType::character(20), Value::Null).declare(
        "G",
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        Value::Null,
    );
    a.declare_cursor("DECLARE C CURSOR FOR SELECT NAME, AGE FROM PEOPLE ORDER BY NAME DESC FOR UPDATE OF AGE").unwrap();
    a.execute("OPEN C", &mut p).unwrap();
    a.execute("FETCH C INTO N, G", &mut p).unwrap();
    assert_eq!(p.value("N").to_string().trim(), "DAN");
    a.execute("UPDATE PEOPLE SET AGE = 99 WHERE CURRENT OF C", &mut p).unwrap();
    let only = a.execute("UPDATE ONLY (PEOPLE) SET AGE = 98 WHERE CURRENT OF C", &mut p).unwrap_err();
    assert!(only.to_string().contains("§14.13 SR9"), "{only}");
    a.execute("FETCH C INTO N, G", &mut p).unwrap();
    a.execute("DELETE FROM PEOPLE WHERE CURRENT OF C", &mut p).unwrap();
    a.execute("FETCH C INTO N, G", &mut p).unwrap();
    assert_eq!(p.value("N").to_string().trim(), "BOB");
    a.execute("UPDATE PEOPLE SET AGE = 97 WHERE CURRENT OF C", &mut p).unwrap();
    a.execute("CLOSE C", &mut p).unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT NAME, AGE FROM PEOPLE ORDER BY NAME"), ["ANN,30", "BOB,97", "DAN,99"]);
    assert_eq!(query(&a, "SELECT NAME, AGE FROM STAFF"), ["DAN,99"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.12 GR, §15.17] MERGE into a typed
/// table updates and deletes its subtables' rows where they are, each
/// through the supertable's triggers, and inserts into the table; into
/// ONLY the table, its own rows alone; a target row matched twice is
/// SQLSTATE 21000.
#[test]
fn merge_reaches_subtable_rows_14_12() {
    let (_db, a) = family();
    ok(
        &a,
        &[
            "CREATE TABLE SRC (NAME CHARACTER VARYING(20), AGE INTEGER)",
            "CREATE TABLE LOG (WHAT CHARACTER VARYING(40))",
            "CREATE TRIGGER TU BEFORE UPDATE ON PEOPLE REFERENCING NEW ROW AS N FOR EACH ROW SET N.AGE = N.AGE + 1000",
            "CREATE TRIGGER TD AFTER DELETE ON PEOPLE REFERENCING OLD ROW AS O FOR EACH ROW INSERT INTO LOG VALUES ('DEL ' || O.NAME)",
            "INSERT INTO SRC VALUES ('ANN', 1), ('CAT', 2), ('DAN', 3), ('EVE', 4)",
            "MERGE INTO PEOPLE P USING SRC S ON P.NAME = S.NAME WHEN MATCHED AND S.NAME = 'DAN' THEN DELETE \
             WHEN MATCHED THEN UPDATE SET AGE = S.AGE WHEN NOT MATCHED THEN INSERT (NAME, AGE) VALUES (S.NAME, S.AGE)",
            "MERGE INTO ONLY (PEOPLE) P USING SRC S ON P.NAME = S.NAME WHEN MATCHED THEN UPDATE SET AGE = 7",
        ],
    );
    assert_eq!(query(&a, "SELECT NAME, AGE FROM PEOPLE ORDER BY NAME"), ["ANN,1007", "BOB,40", "CAT,1002", "EVE,1007"]);
    assert_eq!(query(&a, "SELECT NAME, AGE, PAY FROM STAFF"), ["CAT,1002,100"]);
    assert_eq!(query(&a, "SELECT WHAT FROM LOG"), ["DEL DAN"]);
    let twice = "MERGE INTO PEOPLE P USING (VALUES ('CAT'), ('CAT')) AS S (N) ON P.NAME = S.N WHEN MATCHED THEN UPDATE SET AGE = 9";
    refused(&a, twice, (Code::Cardinality, "21000", "§14.12 GR6a"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.10 GR5] TRUNCATE TABLE ONLY (T)
/// deletes T's own rows; without ONLY, its subtables' too.
#[test]
fn truncate_of_the_family_or_only_the_table_14_10() {
    let (_db, a) = family();
    ok(&a, &["TRUNCATE TABLE ONLY (PEOPLE)"]);
    assert_eq!(query(&a, "SELECT NAME FROM PEOPLE ORDER BY NAME"), ["CAT", "DAN"]);
    ok(&a, &["INSERT INTO PEOPLE (NAME, AGE) VALUES ('EVE', 3)", "TRUNCATE TABLE PEOPLE"]);
    assert_eq!(query(&a, "SELECT NAME FROM PEOPLE"), Vec::<String>::new());
    assert_eq!(query(&a, "SELECT NAME FROM STAFF"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.2 SR3b, GR8, §12.7, §7.6 AR1b ii 2,
/// §14.9 AR1b ii, §21 TABLE_PRIVILEGES] SELECT WITH HIERARCHY OPTION on a
/// supertable is SELECT on its subtables, and lets its grantee name it
/// with ONLY; HIERARCHY OPTION FOR revokes it and leaves SELECT; it is
/// granted on a table of a structured type alone.
#[test]
fn select_with_hierarchy_option_12_2() {
    let (db, a) = family();
    ok(&a, &["CREATE TABLE PLAIN (X INTEGER)", "GRANT SELECT ON PEOPLE TO B WITH HIERARCHY OPTION"]);
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT NAME FROM A.STAFF ORDER BY NAME"), ["CAT", "DAN"]);
    assert_eq!(query(&b, "SELECT NAME FROM ONLY (A.PEOPLE) ORDER BY NAME"), ["ANN", "BOB"]);
    let granted = "SELECT TABLE_NAME, PRIVILEGE_TYPE, WITH_HIERARCHY FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES \
                   WHERE GRANTEE = 'B' AND PRIVILEGE_TYPE = 'SELECT'";
    assert_eq!(query(&b, granted), ["PEOPLE,SELECT,YES"]);
    refused(&a, "GRANT SELECT ON PLAIN TO B WITH HIERARCHY OPTION", syntax("§12.2 SR3b"));
    ok(&a, &["REVOKE HIERARCHY OPTION FOR SELECT ON PEOPLE FROM B CASCADE", "GRANT DELETE, UPDATE ON PEOPLE TO B"]);
    assert_eq!(query(&b, granted), ["PEOPLE,SELECT,NO"]);
    assert_eq!(query(&b, "SELECT COUNT(*) FROM A.PEOPLE"), ["4"]);
    refused(&b, "SELECT NAME FROM A.STAFF", privilege("§6.6 SR3"));
    refused(&b, "SELECT NAME FROM ONLY (A.PEOPLE)", privilege("§7.6 AR1b ii 2"));
    refused(&b, "DELETE FROM ONLY (A.PEOPLE)", privilege("§14.9 AR1b ii"));
    refused(&b, "UPDATE ONLY (A.PEOPLE) SET AGE = 1", privilege("§14.14 AR1b ii"));
    ok(&a, &["REVOKE SELECT ON PEOPLE FROM B CASCADE"]);
    refused(&b, "SELECT NAME FROM A.PEOPLE", privilege("§6.6 SR3"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.11 SR2, §11.23 SR3, §14.15 SR3] A
/// referenceable table is given no column and loses none, and no SET
/// clause sets its self-referencing column.
#[test]
fn a_referenceable_table_keeps_its_columns_11_11() {
    let (_db, a) = family();
    refused(&a, "ALTER TABLE PEOPLE ADD COLUMN Z INTEGER", syntax("§11.11 SR2"));
    refused(&a, "ALTER TABLE PEOPLE DROP COLUMN AGE CASCADE", syntax("§11.23 SR3"));
    refused(&a, "UPDATE PEOPLE SET ID = NULL", syntax("§14.15 SR3"));
    refused(&a, "MERGE INTO PEOPLE P USING (VALUES (1)) AS S (X) ON TRUE WHEN MATCHED THEN UPDATE SET ID = NULL", syntax("§14.15 SR3"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.17 SR1, SR3, SR5, GR1, GR2; §11.18
/// SR1, SR2, SR5, GR1 to GR6] ADD SCOPE gives a reference column a scope,
/// a referenceable table of its referenced type, and DEREF then reads it;
/// DROP SCOPE RESTRICT is refused while a routine dereferences it, and
/// CASCADE drops the routines and views that do and keeps the others; a
/// column inherited from a supertable is altered at the supertable.
#[test]
fn scopes_are_added_and_dropped_11_17() {
    let (_db, a) = family();
    ok(
        &a,
        &[
            "CREATE TABLE PETS (OWNER REF(PERSON), N INTEGER)",
            "INSERT INTO PETS VALUES ((SELECT ID FROM ONLY (PEOPLE) WHERE NAME = 'ANN'), 1)",
        ],
    );
    refused(&a, "SELECT OWNER->NAME FROM PETS", syntax("§6.22 SR1"));
    refused(&a, "ALTER TABLE PETS ALTER COLUMN N ADD SCOPE PEOPLE", syntax("§11.17 SR1"));
    refused(&a, "ALTER TABLE PETS ALTER COLUMN OWNER ADD SCOPE PETS", syntax("§11.17 SR5"));
    refused(&a, "ALTER TABLE PETS ALTER COLUMN OWNER DROP SCOPE RESTRICT", syntax("§11.18 SR1"));
    ok(&a, &["ALTER TABLE PETS ALTER COLUMN OWNER ADD SCOPE PEOPLE"]);
    refused(&a, "ALTER TABLE PETS ALTER COLUMN OWNER ADD SCOPE PEOPLE", syntax("§11.17 SR3"));
    assert_eq!(query(&a, "SELECT OWNER->NAME FROM PETS"), ["ANN"]);
    ok(
        &a,
        &[
            "CREATE TYPE HOLDER AS (R REF(PERSON)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TYPE SUBHOLDER UNDER HOLDER AS (Z INTEGER) NOT FINAL",
            "CREATE TABLE HS OF HOLDER (REF IS HID SYSTEM GENERATED)",
            "CREATE TABLE SUBHS OF SUBHOLDER UNDER HS",
            "INSERT INTO SUBHS (R, Z) VALUES ((SELECT ID FROM ONLY (PEOPLE) WHERE NAME = 'BOB'), 1)",
        ],
    );
    refused(&a, "ALTER TABLE SUBHS ALTER COLUMN R ADD SCOPE PEOPLE", syntax("§11.17 SR2"));
    ok(&a, &["ALTER TABLE HS ALTER COLUMN R ADD SCOPE PEOPLE"]);
    assert_eq!(query(&a, "SELECT R->NAME FROM SUBHS"), ["BOB"]);
    refused(&a, "ALTER TABLE SUBHS ALTER COLUMN R DROP SCOPE CASCADE", syntax("§11.18 SR2"));
    ok(
        &a,
        &[
            "CREATE VIEW V AS SELECT OWNER->NAME AS NAME FROM PETS",
            "CREATE VIEW W AS SELECT N FROM PETS",
            "CREATE FUNCTION F () RETURNS VARCHAR(20) READS SQL DATA RETURN (SELECT DEREF(OWNER).NAME FROM PETS)",
        ],
    );
    refused(&a, "ALTER TABLE PETS ALTER COLUMN OWNER DROP SCOPE RESTRICT", syntax("§11.18 SR5a"));
    ok(&a, &["DROP FUNCTION F RESTRICT"]);
    refused(&a, "ALTER TABLE PETS ALTER COLUMN OWNER DROP SCOPE RESTRICT", syntax("§11.18 SR5b"));
    ok(&a, &["ALTER TABLE PETS ALTER COLUMN OWNER DROP SCOPE CASCADE"]);
    assert_eq!(query(&a, "SELECT N FROM W"), ["1"]);
    refused(&a, "SELECT NAME FROM V", syntax("§5.4 SR5"));
}

/// The database of directory `dir` of `fs`, opened.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.11.1, §11.3 SR11c] [DESIGN] RULES.md
/// D81 a family's system generated references come from one counter,
/// which a database reopened after a crash goes on from: a REF given once
/// is never given again.
#[test]
fn system_generated_references_survive_a_reopen_4_11_1() {
    let scratch = Scratch::new("typed-family-refs");
    let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
    let before = opened(&fs, &dir);
    before.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = before.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE STAFF OF EMP UNDER PEOPLE",
            "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
            "INSERT INTO STAFF (NAME, PAY) VALUES ('CAT', 1)",
        ],
    );
    let db = opened(&after_crash(), &dir);
    let a = db.session("A");
    ok(&a, &["INSERT INTO PEOPLE (NAME) VALUES ('BOB')", "INSERT INTO STAFF (NAME, PAY) VALUES ('DAN', 2)"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT ID) FROM PEOPLE"), ["4"]);
}
