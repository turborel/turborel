//! Typed tables and subtables [ISO/IEC 9075-2:2023 §4.17.9, §11.3 SR11,
//! SR14, AR4, GR4; §11.31 SR6, GR1; §14.11 SR5, SR13a, GR7d, GR7e]: CREATE
//! TABLE OF a structured type with each reference generation, its column
//! options, a subtable UNDER a supertable, the REF values an INSERT gives
//! the self-referencing column, the override clause's rules, DROP TABLE
//! of a supertable, and each rule's refusal with its SQLCODE and SQLSTATE.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: PERSON of NAME and AGE, REF IS SYSTEM GENERATED; EMPLOYEE
/// under it of PAY; BOSS under EMPLOYEE of BONUS; U of REF USING INTEGER;
/// D of REF FROM (X, Y); SHAPE NOT INSTANTIABLE and CIRCLE under it.
/// Schema S, also A's.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20) DEFAULT 'NOBODY', AGE INTEGER) NOT FINAL REF IS SYSTEM GENERATED
           CREATE TYPE EMPLOYEE UNDER PERSON AS (PAY INTEGER DEFAULT 10) NOT FINAL
           CREATE TYPE BOSS UNDER EMPLOYEE AS (BONUS INTEGER) NOT FINAL
           CREATE TYPE U AS (X INTEGER) NOT FINAL REF USING INTEGER
           CREATE TYPE D AS (X INTEGER, Y INTEGER) NOT FINAL REF FROM (X, Y)
           CREATE TYPE SHAPE AS (X INTEGER) NOT INSTANTIABLE NOT FINAL REF IS SYSTEM GENERATED
           CREATE TYPE CIRCLE UNDER SHAPE AS (R INTEGER) NOT FINAL",
    )
    .unwrap();
    db.schema("CREATE SCHEMA S AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
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

/// Asserts `sql` in `s` is refused with SQLCODE class `code`, SQLSTATE
/// `state` and a message citing `cited`; rolls back.
fn refused(s: &Session, sql: &str, (code, state, cited): (Code, &str, &str)) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// A Syntax Rule's refusal citing `cited`.
fn syntax(cited: &str) -> (Code, &str, &str) {
    (Code::SyntaxRule, "2A000", cited)
}

/// PEOPLE of PERSON, its AGE NOT NULL; STAFF of EMPLOYEE under it.
fn family(a: &Session) {
    ok(
        a,
        &[
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED, AGE WITH OPTIONS NOT NULL, NAME WITH OPTIONS DEFAULT 'ANON')",
            "CREATE TABLE STAFF OF EMPLOYEE UNDER PEOPLE",
        ],
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11c vii, SR11f, SR11i, GR4f]
/// CREATE TABLE OF a structured type makes its self-referencing column
/// first, REF(PERSON) SCOPE PEOPLE, not nullable, then a column of each
/// attribute with its default; an INSERT that names no self-referencing
/// column gives each row its own REF value.
#[test]
fn a_typed_table_has_a_column_of_each_attribute_11_3() {
    let (_db, a) = database();
    ok(&a, &["CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)"]);
    let columns = "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, SCOPE_NAME FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_NAME = 'PEOPLE' ORDER BY ORDINAL_POSITION";
    assert_eq!(query(&a, columns), ["ID,REF,NO,NULL,PEOPLE", "NAME,CHARACTER VARYING,YES,'NOBODY',NULL", "AGE,INTEGER,YES,NULL,NULL"]);
    ok(&a, &["INSERT INTO PEOPLE (NAME, AGE) VALUES ('ANN', 30), ('BOB', 40)", "INSERT INTO PEOPLE (AGE) VALUES (5)"]);
    assert_eq!(query(&a, "SELECT NAME, AGE FROM PEOPLE ORDER BY AGE"), ["NOBODY,5", "ANN,30", "BOB,40"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT ID), COUNT(ID) FROM PEOPLE"), ["3,3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11a, AR4, SR11c iii, iv, v]
/// CREATE TABLE OF is refused of a name of no structured type (SQLCODE
/// -103), of a type its owner holds no USAGE on (-104), and of a
/// generation its type's reference representation is not: SYSTEM
/// GENERATED of a user-defined one, USER GENERATED of a system-defined
/// one, DERIVED of a system-defined one.
#[test]
fn a_typed_tables_type_and_generation_are_checked_11_3() {
    let (db, a) = database();
    refused(&a, "CREATE TABLE T OF NOTHING (REF IS ID)", syntax("§11.3 SR11a"));
    a.run("CREATE TYPE DIST AS INTEGER FINAL").unwrap();
    refused(&a, "CREATE TABLE T OF DIST (REF IS ID)", syntax("§11.3 SR11a"));
    refused(&db.session("B"), "CREATE TABLE T OF A.PERSON (REF IS ID)", (Code::Privilege, "2A000", "§11.3 AR4"));
    refused(&a, "CREATE TABLE T OF U (REF IS ID SYSTEM GENERATED)", syntax("§11.3 SR11c iii"));
    refused(&a, "CREATE TABLE T OF U (REF IS ID)", syntax("§11.3 SR11c iii"));
    refused(&a, "CREATE TABLE T OF PERSON (REF IS ID USER GENERATED)", syntax("§11.3 SR11c iv"));
    refused(&a, "CREATE TABLE T OF PERSON (REF IS ID DERIVED)", syntax("§11.3 SR11c v"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11c vi] A table of DERIVED
/// references has a UNIQUE constraint on the derivational attributes in
/// order, each NOT NULL by its column options, or a PRIMARY KEY on them.
/// MC/DC: no such constraint; the columns in another order; UNIQUE
/// without NOT NULL on one; UNIQUE with both; PRIMARY KEY alone.
#[test]
fn derived_references_need_their_unique_constraint_11_3() {
    let (_db, a) = database();
    refused(&a, "CREATE TABLE T OF D (REF IS R DERIVED)", syntax("§11.3 SR11c vi 1"));
    refused(&a, "CREATE TABLE T OF D (REF IS R DERIVED, UNIQUE (Y, X))", syntax("§11.3 SR11c vi 1"));
    refused(&a, "CREATE TABLE T OF D (REF IS R DERIVED, X WITH OPTIONS NOT NULL, UNIQUE (X, Y))", syntax("§11.3 SR11c vi 2"));
    ok(
        &a,
        &[
            "CREATE TABLE T1 OF D (REF IS R DERIVED, X WITH OPTIONS NOT NULL, Y WITH OPTIONS NOT NULL, UNIQUE (X, Y))",
            "CREATE TABLE T2 OF D (REF IS R DERIVED, PRIMARY KEY (X, Y))",
        ],
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.11 GR7d ii] A derived
/// self-referencing column's value is derived from the derivational
/// attributes: equal of two rows of equal values, different otherwise;
/// SR13a: inserting into it needs an override clause, and OVERRIDING USER
/// VALUE derives it in place of the value given.
#[test]
fn derived_references_follow_their_attributes_14_11() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TABLE T1 OF D (REF IS R DERIVED, PRIMARY KEY (X, Y))",
            "CREATE TABLE T2 OF D (REF IS R DERIVED, PRIMARY KEY (X, Y))",
            "INSERT INTO T1 (X, Y) VALUES (1, 2), (2, 1)",
            "INSERT INTO T2 (X, Y) VALUES (1, 2)",
        ],
    );
    let joined = "SELECT P.X, P.Y FROM T1 AS P JOIN T2 AS Q ON P.R = Q.R";
    assert_eq!(query(&a, joined), ["1,2"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT R) FROM T1"), ["2"]);
    refused(&a, "INSERT INTO T2 SELECT R, 2, 1 FROM T1 WHERE X = 1", syntax("§14.11 SR13a"));
    ok(&a, &["INSERT INTO T2 OVERRIDING USER VALUE SELECT R, 2, 1 FROM T1 WHERE X = 1"]);
    let derived = "SELECT COUNT(*) FROM T1 AS P JOIN T2 AS Q ON P.R = Q.R";
    assert_eq!(query(&a, derived), ["2"], "the (2, 1) row of T2 has the reference of T1's");
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.11 SR13a, GR7d i, GR7e ii, SR13]
/// Inserting into a system-generated self-referencing column needs an
/// override clause: OVERRIDING USER VALUE replaces the value given by a
/// new REF value, OVERRIDING SYSTEM VALUE keeps it, here one a row has
/// already, so the column's UNIQUE constraint refuses it. An override
/// clause of a table of neither identity nor such a column is refused.
#[test]
fn the_override_clause_governs_system_references_14_11() {
    let (_db, a) = database();
    ok(&a, &["CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)", "INSERT INTO PEOPLE (NAME) VALUES ('ANN')"]);
    refused(&a, "INSERT INTO PEOPLE VALUES (NULL, 'BOB', 1)", syntax("§14.11 SR13a"));
    refused(&a, "INSERT INTO PEOPLE DEFAULT VALUES", syntax("§14.11 SR13a"));
    ok(&a, &["INSERT INTO PEOPLE OVERRIDING USER VALUE SELECT ID, 'BOB', 2 FROM PEOPLE"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT ID) FROM PEOPLE"), ["2"]);
    let kept = a.run("INSERT INTO PEOPLE OVERRIDING SYSTEM VALUE SELECT ID, 'CY', 3 FROM PEOPLE WHERE NAME = 'ANN'").unwrap_err();
    let _ = a.rollback();
    assert_eq!(kept.sqlstate().as_str(), "23000", "{kept}");
    ok(&a, &["CREATE TABLE PLAIN (K INTEGER)"]);
    refused(&a, "INSERT INTO PLAIN OVERRIDING USER VALUE VALUES (1)", syntax("§14.11 SR13"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.11 GR7e i, SR13] A user-generated
/// self-referencing column is given its values as any column is, with no
/// override clause, which is refused; NOT NULL, it is given one.
#[test]
fn user_generated_references_are_given_14_11() {
    let (_db, a) = database();
    ok(&a, &["CREATE TABLE UT OF U (REF IS R USER GENERATED)"]);
    let null = a.run("INSERT INTO UT (X) VALUES (1)").unwrap_err();
    let _ = a.rollback();
    assert_eq!(null.sqlstate().as_str(), "23000", "{null}");
    refused(&a, "INSERT INTO UT OVERRIDING USER VALUE VALUES (NULL, 1)", syntax("§14.11 SR13"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11g iv to vi, SR14] Column
/// options give an originally-defined column a scope, a default and
/// constraints; SR11g i, iv, SR14: they are refused of no column, of the
/// self-referencing column, of a scope of a column of no reference type,
/// and of a scope that is a view.
#[test]
fn column_options_change_their_columns_11_3() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TYPE PET AS (NAME CHARACTER VARYING(9), OWNER REF(PERSON)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED, NAME WITH OPTIONS DEFAULT 'X' CHECK (NAME <> 'Z'))",
            "CREATE TABLE PETS OF PET (REF IS ID SYSTEM GENERATED, OWNER WITH OPTIONS SCOPE PEOPLE)",
            "CREATE VIEW SEEN AS SELECT * FROM PEOPLE",
        ],
    );
    let scoped = "SELECT COLUMN_NAME, SCOPE_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'PETS' AND COLUMN_NAME = 'OWNER'";
    assert_eq!(query(&a, scoped), ["OWNER,PEOPLE"]);
    ok(&a, &["INSERT INTO PEOPLE (AGE) VALUES (1)"]);
    assert_eq!(query(&a, "SELECT NAME FROM PEOPLE"), ["X"]);
    let checked = a.run("INSERT INTO PEOPLE (NAME) VALUES ('Z')").unwrap_err();
    let _ = a.rollback();
    assert_eq!(checked.sqlstate().as_str(), "23000", "{checked}");
    refused(&a, "CREATE TABLE T OF PERSON (REF IS ID, HEIGHT WITH OPTIONS NOT NULL)", syntax("§11.3 SR11g i"));
    refused(&a, "CREATE TABLE T OF PERSON (REF IS ID, ID WITH OPTIONS NOT NULL)", syntax("§11.3 SR11g i"));
    refused(&a, "CREATE TABLE T OF PERSON (REF IS ID, NAME WITH OPTIONS SCOPE PEOPLE)", syntax("§11.3 SR11g iv"));
    refused(&a, "CREATE TABLE T OF PET (REF IS ID, OWNER WITH OPTIONS SCOPE SEEN)", syntax("§11.3 SR14"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11d vi, vii, SR11f iii, GR4f,
/// GR4i] A subtable has the self-referencing column of its supertable,
/// REF(EMPLOYEE) SCOPE STAFF, its inherited columns with their direct
/// supercolumns' defaults and nullability, then its originally-defined
/// ones; it is a direct subtable of its supertable.
#[test]
fn a_subtable_inherits_its_supertables_columns_11_3() {
    let (_db, a) = database();
    family(&a);
    let columns = "SELECT COLUMN_NAME, UDT_NAME, IS_NULLABLE, COLUMN_DEFAULT, SCOPE_NAME FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_NAME = 'STAFF' ORDER BY ORDINAL_POSITION";
    assert_eq!(
        query(&a, columns),
        ["ID,EMPLOYEE,NO,NULL,STAFF", "NAME,NULL,YES,'ANON',NULL", "AGE,NULL,NO,NULL,NULL", "PAY,NULL,YES,10,NULL"]
    );
    let supertables = "SELECT TABLE_NAME, SUPERTABLE_NAME FROM INFORMATION_SCHEMA.DIRECT_SUPERTABLES";
    assert_eq!(query(&a, supertables), ["STAFF,PEOPLE"]);
    ok(&a, &["CREATE TABLE BOSSES OF BOSS UNDER STAFF (BONUS WITH OPTIONS DEFAULT 1)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11d i to v, SR11g i] A subtable
/// is refused under a table not defined, a view, a table of no type and a
/// table of a type its type is no direct subtype of; in another schema;
/// of a type a table of its family has; with a PRIMARY KEY; and with
/// column options of an inherited column.
#[test]
fn a_subtables_supertable_is_checked_11_3() {
    let (_db, a) = database();
    family(&a);
    ok(&a, &["CREATE TABLE PLAIN (K INTEGER)", "CREATE VIEW SEEN AS SELECT * FROM PEOPLE"]);
    refused(&a, "CREATE TABLE T OF EMPLOYEE UNDER NOWHERE", syntax("§11.3 SR11d i"));
    refused(&a, "CREATE TABLE T OF EMPLOYEE UNDER SEEN", syntax("§11.3 SR11d i"));
    refused(&a, "CREATE TABLE T OF EMPLOYEE UNDER PLAIN", syntax("§11.3 SR11d ii"));
    refused(&a, "CREATE TABLE T OF BOSS UNDER PEOPLE", syntax("§11.3 SR11d ii"));
    refused(&a, "CREATE TABLE S.T OF EMPLOYEE UNDER A.PEOPLE", syntax("§11.3 SR11d iii"));
    refused(&a, "CREATE TABLE T OF EMPLOYEE UNDER PEOPLE", syntax("§11.3 SR11d iv"));
    refused(&a, "CREATE TABLE T OF BOSS UNDER STAFF (BONUS WITH OPTIONS PRIMARY KEY)", syntax("§11.3 SR11d v"));
    refused(&a, "CREATE TABLE T OF BOSS UNDER STAFF (PAY WITH OPTIONS NOT NULL)", syntax("§11.3 SR11g i"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 GR4f iii, §14.11 GR7d i] The rows
/// of a supertable and of its subtables take REF values of one sequence
/// generator, the family's: no two are alike.
#[test]
fn a_family_shares_its_references_14_11() {
    let (_db, a) = database();
    family(&a);
    ok(
        &a,
        &[
            "CREATE TABLE BOSSES OF BOSS UNDER STAFF",
            "INSERT INTO PEOPLE (NAME, AGE) VALUES ('ANN', 1), ('BOB', 2)",
            "INSERT INTO STAFF (NAME, AGE) VALUES ('CY', 3), ('DI', 4)",
            "INSERT INTO BOSSES (NAME, AGE) VALUES ('ED', 5)",
            "INSERT INTO PEOPLE (NAME, AGE) VALUES ('FAY', 6)",
        ],
    );
    let mut all = Vec::new();
    for table in ["PEOPLE", "STAFF", "BOSSES"] {
        all.extend(query(&a, &format!("SELECT ID FROM ONLY ({table})")));
    }
    assert_eq!(all.len(), 6);
    let mut distinct = all.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 6, "{all:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.11 SR5, §11.3 GR4n] A typed table
/// of a type NOT INSTANTIABLE is not insertable-into: an INSERT into it is
/// refused, one into its subtable of an instantiable type is not.
#[test]
fn a_table_of_a_type_not_instantiable_takes_no_insert_14_11() {
    let (_db, a) = database();
    ok(&a, &["CREATE TABLE SHAPES OF SHAPE (REF IS ID SYSTEM GENERATED)", "CREATE TABLE CIRCLES OF CIRCLE UNDER SHAPES"]);
    refused(&a, "INSERT INTO SHAPES (X) VALUES (1)", syntax("§14.11 SR5"));
    ok(&a, &["INSERT INTO CIRCLES (X, R) VALUES (1, 2)"]);
    assert_eq!(query(&a, "SELECT X, R FROM ONLY (CIRCLES)"), ["1,2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.31 SR6, GR1] DROP TABLE RESTRICT of
/// a table of a proper subtable is refused; CASCADE drops its subtables,
/// the deepest first, then it; DROP SCHEMA CASCADE drops a family.
#[test]
fn a_supertable_is_dropped_with_its_subtables_11_31() {
    let (_db, a) = database();
    family(&a);
    ok(&a, &["CREATE TABLE BOSSES OF BOSS UNDER STAFF"]);
    refused(&a, "DROP TABLE PEOPLE RESTRICT", syntax("§11.31 SR6"));
    refused(&a, "DROP TABLE STAFF RESTRICT", syntax("§11.31 SR6"));
    ok(&a, &["DROP TABLE BOSSES RESTRICT", "CREATE TABLE BOSSES OF BOSS UNDER STAFF", "DROP TABLE PEOPLE CASCADE"]);
    let left = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A'";
    assert_eq!(query(&a, left), Vec::<String>::new());
    family(&a);
    ok(&a, &["DROP SCHEMA A CASCADE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.59 SR4d, GR7] DROP TYPE RESTRICT of
/// the type of a typed table is refused; CASCADE drops its tables, a
/// supertable's subtables with it.
#[test]
fn a_type_is_dropped_with_its_typed_tables_11_59() {
    let (_db, a) = database();
    family(&a);
    refused(&a, "DROP TYPE EMPLOYEE RESTRICT", syntax("§11.59 SR4"));
    ok(&a, &["DROP TYPE PERSON CASCADE"]);
    let left = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A'";
    assert_eq!(query(&a, left), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11i] A typed table and its
/// subtable defined in one `<schema definition>`, the subtable after its
/// supertable, take REF values of one generator.
#[test]
fn a_family_is_defined_in_a_schema_definition_11_3() {
    let (db, _a) = database();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A2
           CREATE TYPE P AS (N INTEGER) NOT FINAL REF IS SYSTEM GENERATED
           CREATE TYPE Q UNDER P AS (M INTEGER) NOT FINAL
           CREATE TABLE TP OF P (REF IS ID SYSTEM GENERATED)
           CREATE TABLE TQ OF Q UNDER TP",
    )
    .unwrap();
    let s = db.session("A2");
    ok(&s, &["INSERT INTO TP (N) VALUES (1)", "INSERT INTO TQ (N, M) VALUES (2, 3)"]);
    let (p, q) = (query(&s, "SELECT ID FROM ONLY (TP)"), query(&s, "SELECT ID FROM ONLY (TQ)"));
    assert_eq!((p.len(), q.len()), (1, 1));
    assert_ne!(p, q);
}
