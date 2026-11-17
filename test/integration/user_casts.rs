//! User-defined casts, transform groups and the routines a schema defines
//! for its types [ISO/IEC 9075-2:2023 §4.13, §6.4, §6.13 SR12, AR2b, GR3,
//! §9.1, §9.2 SR3, GR2, §11.1, §11.62 SR4] (Features S211, S241): CAST
//! by a cast function, a value assigned by one AS ASSIGNMENT where its
//! type is not assignable, CURRENT_TRANSFORM_GROUP_FOR_TYPE of a type or
//! its supertype, and DROP FUNCTION of a cast, ordering or transform
//! function.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::rows;

/// Schema A, made by one CREATE SCHEMA: P of X, with the method M; Q
/// under P; an ordering of P by SORD, a cast of P to CHARACTER(5) by
/// SCAST, a cast of P to INTEGER by PTOI AS ASSIGNMENT, of ten times X;
/// TP of one P of X 2, TI of an integer.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE P AS (X INTEGER) NOT FINAL METHOD M () RETURNS INTEGER CONTAINS SQL
           CREATE METHOD M () RETURNS INTEGER FOR P RETURN SELF.X + 1
           CREATE TYPE Q UNDER P AS (Y INTEGER) NOT FINAL
           CREATE TYPE R AS (Z INTEGER) NOT FINAL
           CREATE FUNCTION SORD (V P) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN V.X
           CREATE ORDERING FOR P ORDER FULL BY MAP WITH FUNCTION SORD (P)
           CREATE FUNCTION SCAST (V P) RETURNS CHARACTER(5) DETERMINISTIC CONTAINS SQL RETURN 'P'
           CREATE CAST (P AS CHARACTER(5)) WITH FUNCTION SCAST (P)
           CREATE FUNCTION PTOI (V P) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN V.X * 10
           CREATE CAST (P AS INTEGER) WITH FUNCTION PTOI (P) AS ASSIGNMENT
           CREATE TABLE TP (V P)
           CREATE TABLE TI (N INTEGER)",
    )
    .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    ok(&a, &["INSERT INTO TP VALUES (NEW P().X(2))", "GRANT SELECT ON TP TO B", "GRANT USAGE ON TYPE P TO B"]);
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

/// Verifies: [ISO/IEC 9075-2:2023 §11.1 SR, §11.60 SR9q, §6.18, §6.13]
/// CREATE SCHEMA defines a method, an ordering and casts of a type of it,
/// each usable after; a method of a type of another schema is refused, as
/// is a cast of a type of a schema its owner does not own.
#[test]
fn a_schema_defines_the_routines_of_its_types_11_1() {
    let db = database();
    let a = db.session("A");
    let one = "FROM (VALUES 1) AS T(Z)";
    assert_eq!(query(&a, &format!("SELECT NEW P().X(1).M() {one}")), ["2"]);
    assert_eq!(query(&a, &format!("SELECT CAST(NEW P().X(1) AS CHARACTER(5)), NEW P().X(1) < NEW P().X(2) {one}")), ["P    ,TRUE"]);
    let e = db.schema("CREATE SCHEMA AUTHORIZATION C CREATE METHOD M () RETURNS INTEGER FOR A.P RETURN 1").unwrap_err();
    assert!(e.to_string().contains("§11.60 SR9q"), "{e}");
    let e = db
        .schema("CREATE SCHEMA AUTHORIZATION D CREATE FUNCTION F (V A.P) RETURNS DATE CONTAINS SQL RETURN CURRENT_DATE CREATE CAST (A.P AS DATE) WITH FUNCTION F (A.P)")
        .unwrap_err();
    assert_eq!(e.code, Code::Privilege, "{e}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 SR12, AR2b, GR3] CAST of a
/// structured type invokes the cast function whose source is first in
/// its type precedence list, of a subtype too; to a supertype it is the
/// value; with no such cast it is refused, as it is without EXECUTE on the
/// function.
#[test]
fn a_user_defined_cast_invokes_its_function_6_13() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    let one = "FROM (VALUES 1) AS T(Z)";
    assert_eq!(query(&a, &format!("SELECT CAST(NEW P().X(5) AS INTEGER), CAST(NEW Q().X(6) AS INTEGER) {one}")), ["50,60"]);
    assert_eq!(query(&a, &format!("SELECT CAST(NEW Q().X(7) AS P).X {one}")), ["7"]);
    refused(&a, &format!("SELECT CAST(NEW P().X(5) AS DECIMAL(5,1)) {one}"), (Code::SyntaxRule, "§6.13 SR12"));
    refused(&a, &format!("SELECT CAST(NEW P().X(5) AS DATE) {one}"), (Code::SyntaxRule, "§6.13 SR12"));
    refused(&a, &format!("SELECT CAST(NEW P().X(5) AS R) {one}"), (Code::SyntaxRule, "§6.13 SR12"));
    refused(&b, "SELECT CAST(V AS INTEGER) FROM A.TP", (Code::Privilege, "§6.13 AR2b"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.13, §9.2 SR3, GR2] A value whose
/// type is not assignable to its site is stored by the cast AS ASSIGNMENT
/// to the site's type: by INSERT of VALUES and of a query, UPDATE, MERGE, a
/// function's result, and a procedure's SET, FETCH and SELECT INTO; an
/// argument is not, and a cast not AS ASSIGNMENT stores nothing.
#[test]
fn a_value_is_stored_by_a_cast_as_assignment_9_2() {
    let db = database();
    let a = db.session("A");
    ok(
        &a,
        &[
            "INSERT INTO TI VALUES (NEW P().X(1))",
            "INSERT INTO TI SELECT V FROM TP",
            "UPDATE TI SET N = NEW Q().X(3) WHERE N = 10",
            "CREATE FUNCTION TWICE (N INTEGER) RETURNS INTEGER CONTAINS SQL RETURN N * 2",
            "CREATE FUNCTION BACK (N INTEGER) RETURNS INTEGER CONTAINS SQL RETURN NEW P().X(N)",
        ],
    );
    assert_eq!(query(&a, "SELECT N FROM TI ORDER BY N"), ["20", "30"]);
    assert_eq!(query(&a, "SELECT TWICE(N), BACK(4) FROM TI ORDER BY N"), ["40,40", "60,40"]);
    // [§9.18 SR10d iv] a parameter's type is in its argument's precedence list.
    refused(&a, "SELECT TWICE(V) FROM TP", (Code::SyntaxRule, "§9.18 SR10d"));
    ok(
        &a,
        &["CREATE PROCEDURE PR (OUT S INTEGER) READS SQL DATA \
           BEGIN DECLARE X1, Y1 INTEGER; DECLARE K CURSOR FOR SELECT V FROM TP; \
           SET X1 = NEW P().X(6); OPEN K; FETCH K INTO Y1; CLOSE K; \
           SELECT V INTO S FROM TP; SET S = S + X1 + Y1; END"],
    );
    let mut p = Params::new().declare("S", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, Value::Null);
    a.execute("CALL PR(S)", &mut p).unwrap();
    assert_eq!(p.get("S").map(|p| p.value.to_string()), Some("100".to_owned()));
    ok(&a, &["MERGE INTO TI USING TP ON 1 = 0 WHEN NOT MATCHED THEN INSERT VALUES (V)"]);
    assert_eq!(query(&a, "SELECT N FROM TI ORDER BY N"), ["20", "20", "30"]);
    let char5 = "CREATE TABLE TC (C CHARACTER(5))";
    ok(&a, &[char5]);
    refused(&a, "INSERT INTO TC VALUES (NEW P().X(1))", (Code::SyntaxRule, "§9.2"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.4 GR21, §19.9 GR1] The current
/// transform group of a type is the one SET for it, else for a supertype
/// of it, else the default.
#[test]
fn the_transform_group_of_a_type_is_its_supertypes_else_the_default_6_4() {
    let db = database();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE FUNCTION TOSQL (V P) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN V.X",
            "CREATE FUNCTION FROMSQL (V INTEGER) RETURNS P DETERMINISTIC CONTAINS SQL RETURN NEW P().X(V)",
            "CREATE TRANSFORM FOR P G (TO SQL WITH FUNCTION FROMSQL (INTEGER), FROM SQL WITH FUNCTION TOSQL (P))",
            "CREATE TRANSFORM FOR P G2 (TO SQL WITH FUNCTION FROMSQL (INTEGER), FROM SQL WITH FUNCTION TOSQL (P))",
        ],
    );
    a.run("SET DEFAULT TRANSFORM GROUP G").unwrap();
    a.run("SET TRANSFORM GROUP FOR TYPE P G2").unwrap();
    let groups = "SELECT CURRENT_TRANSFORM_GROUP_FOR_TYPE A.Q, CURRENT_TRANSFORM_GROUP_FOR_TYPE A.P, \
                  CURRENT_TRANSFORM_GROUP_FOR_TYPE A.R, CURRENT_DEFAULT_TRANSFORM_GROUP FROM (VALUES 1) AS T(Z)";
    assert_eq!(query(&a, groups), ["G2,G2,G,G"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.62 SR4a, SR4d, GR2 to GR4] DROP
/// FUNCTION RESTRICT of an ordering, transform or cast function is
/// refused; CASCADE drops the ordering, transform or cast with it. DROP
/// TYPE RESTRICT of a distinct type a cast names is refused [§11.59
/// SR4f iv].
#[test]
fn a_cast_ordering_or_transform_function_is_dropped_with_cascade_11_62() {
    let db = database();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE FUNCTION TOSQL (V P) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN V.X",
            "CREATE FUNCTION FROMSQL (V INTEGER) RETURNS P DETERMINISTIC CONTAINS SQL RETURN NEW P().X(V)",
            "CREATE TRANSFORM FOR P G (TO SQL WITH FUNCTION FROMSQL (INTEGER), FROM SQL WITH FUNCTION TOSQL (P))",
        ],
    );
    refused(&a, "DROP FUNCTION SORD RESTRICT", (Code::SyntaxRule, "§11.62 SR4a"));
    refused(&a, "DROP FUNCTION TOSQL RESTRICT", (Code::SyntaxRule, "§11.62 SR4d i"));
    refused(&a, "DROP FUNCTION SCAST RESTRICT", (Code::SyntaxRule, "§11.62 SR4d ii"));
    ok(&a, &["DROP FUNCTION SORD CASCADE", "DROP FUNCTION TOSQL CASCADE", "DROP FUNCTION SCAST CASCADE"]);
    let one = "FROM (VALUES 1) AS T(Z)";
    refused(&a, &format!("SELECT NEW P().X(1) < NEW P().X(2) {one}"), (Code::SyntaxRule, "P"));
    refused(&a, &format!("SELECT CAST(NEW P().X(1) AS CHARACTER(5)) {one}"), (Code::SyntaxRule, "§6.13 SR12"));
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRANSFORMS WHERE GROUP_NAME = 'G'"), ["0"]);
    // [§11.59 SR4f iv] a distinct type a cast names is dropped with CASCADE.
    ok(
        &a,
        &[
            "CREATE TYPE MONEY AS DECIMAL(9,2) FINAL",
            "CREATE FUNCTION MTOC (M MONEY) RETURNS CHARACTER(9) DETERMINISTIC CONTAINS SQL RETURN 'M'",
            "CREATE CAST (MONEY AS CHARACTER(9)) WITH FUNCTION MTOC (MONEY)",
        ],
    );
    refused(&a, "DROP TYPE MONEY RESTRICT", (Code::SyntaxRule, "§11.59 SR4f iv"));
    ok(&a, &["DROP TYPE MONEY CASCADE"]);
}
