//! Values of structured types [ISO/IEC 9075-2:2023 §4.8, §6.17 to
//! §6.24, §8.19, §9.18, §14.15 SR7]: NEW and the constructor, observers
//! and mutators, methods of each kind and their dispatch by most specific
//! type, a static method, IS OF and TREAT, a CAST to a supertype, SET
//! clauses of attributes, references with DEREF and ->, TREAT of a
//! reference, and DROP TYPE of a type a reference type names.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: PT of X and Y DEFAULT 7, with NORM, the static ORIGIN and a
/// constructor of one integer; PT3 under it of Z, overriding NORM; table
/// T of a default PT, PT(4), a null and a PT3.
fn points() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PT AS (X INTEGER, Y INTEGER DEFAULT 7) NOT FINAL METHOD NORM () RETURNS INTEGER CONTAINS SQL, \
             STATIC METHOD ORIGIN () RETURNS PT CONTAINS SQL, CONSTRUCTOR METHOD PT (A INTEGER) RETURNS PT SELF AS RESULT CONTAINS SQL",
            "CREATE TYPE PT3 UNDER PT AS (Z INTEGER) NOT FINAL OVERRIDING METHOD NORM () RETURNS INTEGER",
            "CREATE METHOD NORM () RETURNS INTEGER FOR PT RETURN SELF.X + SELF.Y",
            "CREATE METHOD NORM () RETURNS INTEGER FOR PT3 RETURN SELF.X + SELF.Y + SELF.Z",
            "CREATE STATIC METHOD ORIGIN () RETURNS PT FOR PT RETURN NEW PT().X(0).Y(0)",
            "CREATE CONSTRUCTOR METHOD PT (A INTEGER) RETURNS PT FOR PT BEGIN SET SELF.X = A; SET SELF.Y = A; RETURN SELF; END",
            "CREATE TABLE T (K INTEGER, P PT)",
            "INSERT INTO T VALUES (1, NEW PT()), (2, NEW PT(4)), (3, NULL), (4, NEW PT3().Z(9).X(2))",
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

/// Verifies: [ISO/IEC 9075-2:2023 §6.17 GR, §6.20, §9.18 GR8] NEW of no
/// arguments gives each attribute its default; NEW of arguments invokes
/// the constructor method; an observer reads an attribute and a mutator
/// gives a new value with it changed; a mutator of a null value is
/// SQLSTATE 2202D.
#[test]
fn constructors_observers_and_mutators_6_17() {
    let (_db, a) = points();
    assert_eq!(query(&a, "SELECT K, P FROM T ORDER BY K"), ["1,A.PT(NULL, 7)", "2,A.PT(4, 4)", "3,NULL", "4,A.PT3(2, 7, 9)"]);
    assert_eq!(query(&a, "SELECT P.Y, P.X(55) FROM T WHERE K = 1"), ["7,A.PT(55, 7)"]);
    refused(&a, "UPDATE T SET P.X = 1 WHERE K = 3", (Code::Data, "2202D", "§9.18 GR8a i 1"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.18, §6.19, §9.18 GR3] An instance
/// method is chosen by the most specific type of its subject, and of the
/// subject treated as a supertype, by that type; a static method is
/// invoked of its type; a method of a null subject is null.
#[test]
fn methods_dispatch_by_most_specific_type_6_18() {
    let (_db, a) = points();
    assert_eq!(query(&a, "SELECT K, P.NORM(), (P AS PT).NORM() FROM T ORDER BY K"), ["1,NULL,NULL", "2,8,8", "3,NULL,NULL", "4,18,9"]);
    assert_eq!(query(&a, "SELECT PT::ORIGIN() FROM T WHERE K = 1"), ["A.PT(0, 0)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 GR, §6.21, §8.19] IS OF tells a
/// value's most specific type, of ONLY that type or of its subtypes too;
/// TREAT gives a value its subtype, and SQLSTATE 0D000 of a value not of
/// it; a CAST to a supertype keeps the value.
#[test]
fn type_predicates_and_treatment_8_19() {
    let (_db, a) = points();
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IS OF (ONLY PT3)"), ["4"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IS OF (PT) ORDER BY K"), ["1", "2", "4"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IS NOT OF (PT3) ORDER BY K"), ["1", "2"]);
    assert_eq!(query(&a, "SELECT CAST(P AS PT) FROM T WHERE K = 4"), ["A.PT3(2, 7, 9)"]);
    assert_eq!(query(&a, "SELECT TREAT(P AS PT3).Z FROM T WHERE K = 4"), ["9"]);
    let e = a.open_cursor("SELECT TREAT(P AS PT3).Z FROM T WHERE K = 2").and_then(|mut c| c.fetch().map(|_| ())).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "0D000", "{e}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.15 SR4, SR7c, GR] SET clauses of
/// attributes change an attribute each, a later one seeing the value
/// before the statement; DEFAULT gives an attribute its default; a column
/// set whole and by an attribute is set twice (SQLCODE -103).
#[test]
fn attributes_are_set_by_set_clauses_14_15() {
    let (_db, a) = points();
    ok(
        &a,
        &[
            "UPDATE T SET P.X = 10, P.Y = P.X WHERE K = 1",
            "UPDATE T SET P.Y = DEFAULT, P.X = NULL WHERE K = 4",
            "UPDATE T SET P.X = 100 WHERE K = 2",
        ],
    );
    assert_eq!(query(&a, "SELECT K, P FROM T ORDER BY K"), ["1,A.PT(10, 10)", "2,A.PT(100, 4)", "3,NULL", "4,A.PT3(NULL, 7, 9)"]);
    refused(&a, "UPDATE T SET P = NEW PT(), P.X = 1 WHERE K = 1", (Code::SyntaxRule, "2A000", "§8.12 SR4"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.22 to §6.24, §8.2 GR] A reference
/// scoped to a typed table reaches its row: `->` reads an attribute,
/// DEREF gives the row as an instance of its most specific type, `->` a
/// method invokes it; references compare for equality; of a row that is
/// gone, the value is null.
#[test]
fn references_reach_their_rows_6_22() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED METHOD HELLO () RETURNS CHARACTER VARYING(30) CONTAINS SQL",
            "CREATE METHOD HELLO () RETURNS CHARACTER VARYING(30) FOR PERSON RETURN 'HI ' || SELF.NAME",
            "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE STAFF OF EMP UNDER PEOPLE",
            "CREATE TABLE PETS (OWNER REF(PERSON) SCOPE PEOPLE, N INTEGER)",
            "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
            "INSERT INTO STAFF (NAME, PAY) VALUES ('CAT', 5)",
            "INSERT INTO PETS SELECT ID, 1 FROM ONLY (PEOPLE)",
            "INSERT INTO PETS SELECT ID, 2 FROM STAFF",
        ],
    );
    assert_eq!(query(&a, "SELECT OWNER->NAME FROM PETS ORDER BY N"), ["ANN", "CAT"]);
    assert_eq!(query(&a, "SELECT DEREF(OWNER) FROM PETS ORDER BY N"), ["A.PERSON('ANN')", "A.EMP('CAT', 5)"]);
    assert_eq!(query(&a, "SELECT OWNER->HELLO() FROM PETS ORDER BY N"), ["HI ANN", "HI CAT"]);
    assert_eq!(query(&a, "SELECT P.NAME FROM PEOPLE P, PETS WHERE P.ID = OWNER AND N = 2"), ["CAT"]);
    ok(&a, &["DELETE FROM PEOPLE WHERE NAME = 'ANN'"]);
    assert_eq!(query(&a, "SELECT OWNER->NAME IS NULL FROM PETS ORDER BY N"), ["TRUE", "FALSE"]);
}

/// Schema A: PERSON, EMP under it and BOSS under EMP, a table of each in
/// one family, holding ANN, CAT and BIG; LOOSE of references with no
/// scope to each, numbered 1 to 3.
fn bosses() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TYPE EMP UNDER PERSON AS (PAY INTEGER) NOT FINAL",
            "CREATE TYPE BOSS UNDER EMP AS (B INTEGER) NOT FINAL",
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE STAFF OF EMP UNDER PEOPLE",
            "CREATE TABLE BOSSES OF BOSS UNDER STAFF",
            "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
            "INSERT INTO STAFF (NAME, PAY) VALUES ('CAT', 5)",
            "INSERT INTO BOSSES (NAME, PAY, B) VALUES ('BIG', 9, 1)",
            "CREATE TABLE LOOSE (OWNER REF(PERSON), N INTEGER)",
            "INSERT INTO LOOSE SELECT ID, CASE NAME WHEN 'ANN' THEN 1 WHEN 'CAT' THEN 2 ELSE 3 END FROM PEOPLE",
        ],
    );
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.17 SR1, SR3, GR, Feature S162]
/// [DESIGN] RULES.md D81 TREAT of a reference to a reference type of a
/// subtype gives the reference if the row it identifies is of a table of
/// that type or a subtype, SQLSTATE 0D000 if of another, null of null; a
/// reference that identifies no row keeps its declared type; the target
/// may have a scope, through which it is dereferenced; a target that is no
/// reference type, or no subtype, is SQLCODE -103.
#[test]
fn references_are_treated_as_subtypes_6_17() {
    let (_db, a) = bosses();
    let treated = |target: &str, n: u8| {
        let sql = format!("SELECT R IS NULL FROM (SELECT TREAT(OWNER AS {target}) FROM LOOSE WHERE N = {n}) AS D (R)");
        let fetched = a.open_cursor(&sql).and_then(|mut c| c.fetch());
        let _ = a.rollback();
        fetched.map(|r| r.and_then(|r| r.first().map(ToString::to_string))).map_err(|e| e.sqlstate().as_str().to_owned())
    };
    let not_null: Result<Option<String>, String> = Ok(Some("FALSE".to_owned()));
    assert_eq!(treated("REF(EMP)", 2), not_null.clone());
    assert_eq!(treated("REF(EMP)", 3), not_null.clone());
    assert_eq!(treated("REF(BOSS)", 3), not_null.clone());
    assert_eq!(treated("REF(EMP)", 1), Err("0D000".to_owned()));
    assert_eq!(treated("REF(BOSS)", 2), Err("0D000".to_owned()));
    assert_eq!(query(&a, "SELECT TREAT(CAST(NULL AS REF(PERSON)) AS REF(BOSS)) IS NULL FROM LOOSE WHERE N = 1"), ["TRUE"]);
    assert_eq!(query(&a, "SELECT TREAT(OWNER AS REF(EMP) SCOPE STAFF)->PAY FROM LOOSE WHERE N = 2"), ["5"]);
    ok(&a, &["DELETE FROM PEOPLE WHERE NAME = 'CAT'"]);
    assert_eq!(treated("REF(PERSON)", 2), not_null.clone());
    assert_eq!(treated("REF(EMP)", 2), Err("0D000".to_owned()));
    let syntax = |cited| (Code::SyntaxRule, "2A000", cited);
    refused(&a, "SELECT TREAT(N AS REF(EMP)) FROM LOOSE", syntax("§6.17 SR1"));
    refused(&a, "SELECT TREAT(OWNER AS EMP) FROM LOOSE", syntax("§6.17 SR1"));
    refused(&a, "SELECT TREAT(CAST(OWNER AS REF(EMP)) AS REF(PERSON)) FROM LOOSE", syntax("§6.17 SR3"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.59 SR4a, SR4g, GR1] DROP TYPE
/// RESTRICT is refused while a column, or a routine's parameter, is of a
/// reference type of the type; CASCADE drops the column and the routine.
#[test]
fn a_type_named_by_a_reference_type_is_dropped_11_59() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
            "CREATE TABLE PETS (OWNER REF(PERSON), N INTEGER)",
            "CREATE FUNCTION F (R REF(PERSON)) RETURNS INTEGER CONTAINS SQL RETURN 1",
        ],
    );
    let syntax = |cited| (Code::SyntaxRule, "2A000", cited);
    refused(&a, "DROP TYPE PERSON RESTRICT", syntax("§11.59 SR4a"));
    ok(&a, &["ALTER TABLE PETS DROP COLUMN OWNER RESTRICT"]);
    refused(&a, "DROP TYPE PERSON RESTRICT", syntax("§11.59 SR4g"));
    ok(&a, &["ALTER TABLE PETS ADD COLUMN OWNER REF(PERSON)", "DROP TYPE PERSON CASCADE"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'PETS'"), ["1"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.12.3, §6.39, §7.6 SR8e, Feature
/// S274] A multiset of references is a column's type, made by COLLECT
/// and by MULTISET of a query, counted, tested for MEMBER, and unnested
/// to references that are dereferenced.
#[test]
fn multisets_of_references_are_unnested_and_dereferenced_4_12_3() {
    let (_db, a) = bosses();
    ok(
        &a,
        &[
            "CREATE TABLE TEAMS (T INTEGER, MEMBERS REF(PERSON) SCOPE PEOPLE MULTISET)",
            "INSERT INTO TEAMS SELECT 1, COLLECT(ID) FROM PEOPLE",
            "INSERT INTO TEAMS VALUES (2, MULTISET(SELECT ID FROM STAFF))",
        ],
    );
    assert_eq!(query(&a, "SELECT T, CARDINALITY(MEMBERS) FROM TEAMS ORDER BY T"), ["1,3", "2,2"]);
    let names = "SELECT U.R -> NAME FROM TEAMS, UNNEST(TEAMS.MEMBERS) AS U(R) WHERE T = 2 ORDER BY 1";
    assert_eq!(query(&a, names), ["BIG", "CAT"]);
    let member = "SELECT T FROM TEAMS WHERE (SELECT ID FROM PEOPLE WHERE NAME = 'ANN') MEMBER OF MEMBERS";
    assert_eq!(query(&a, member), ["1"]);
}
