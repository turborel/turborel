//! SQL-invoked routines as the Information Schema shows them
//! [ISO/IEC 9075-11 ROUTINES, PARAMETERS], their
//! definitions seen by their owners alone, and routines of array
//! parameters and results (Features S203, S204).

use turborel::db::{Database, Session};

/// Schema A: T (K) of rows 1 and 2; function F of a parameter and a
/// result, and procedure P of each mode.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1), (2)").unwrap();
    a.run("CREATE FUNCTION F (X SMALLINT) RETURNS DECIMAL(5, 1) SPECIFIC F_ONE DETERMINISTIC CONTAINS SQL RETURNS NULL ON NULL INPUT RETURN X / 2.0").unwrap();
    a.run("CREATE PROCEDURE P (IN X INTEGER, OUT Y INTEGER, INOUT Z CHARACTER(3)) MODIFIES SQL DATA INSERT INTO T VALUES (X)").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 GR3, §4.35.5] ROUTINES shows each
/// routine of an SQL routine body: its kind, specific name, result type,
/// body kind, determinism, data access and null-call, a procedure of no
/// result and no null-call; and its specific name implicit if none was
/// written [SR9s].
#[test]
fn routines_are_shown() {
    let db = defined();
    let a = db.session("A");
    let shown = rows(
        &a,
        "SELECT ROUTINE_NAME, ROUTINE_TYPE, COALESCE(DATA_TYPE, '-'), ROUTINE_BODY, IS_DETERMINISTIC, SQL_DATA_ACCESS, \
         COALESCE(IS_NULL_CALL, '-') FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME",
    );
    assert_eq!(shown, ["F,FUNCTION,DECIMAL,SQL,YES,CONTAINS SQL,YES", "P,PROCEDURE,-,SQL,NO,MODIFIES SQL DATA,-"]);
    let specific = rows(&a, "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME");
    assert_eq!(specific.first().map(String::as_str), Some("F_ONE"));
    assert!(specific.get(1).is_some_and(|p| p.starts_with("P_")), "implicit: {specific:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.5, §11.60 GR3] PARAMETERS shows each
/// parameter of each routine, in order, with its mode and type.
#[test]
fn parameters_are_shown() {
    let db = defined();
    let a = db.session("A");
    let shown = rows(
        &a,
        "SELECT R.ROUTINE_NAME, P.ORDINAL_POSITION, P.PARAMETER_MODE, P.PARAMETER_NAME, P.DATA_TYPE \
         FROM INFORMATION_SCHEMA.PARAMETERS AS P JOIN INFORMATION_SCHEMA.ROUTINES AS R \
         ON P.SPECIFIC_SCHEMA = R.SPECIFIC_SCHEMA AND P.SPECIFIC_NAME = R.SPECIFIC_NAME \
         WHERE R.ROUTINE_SCHEMA = 'A' ORDER BY R.ROUTINE_NAME, P.ORDINAL_POSITION",
    );
    assert_eq!(shown, ["F,1,IN,X,SMALLINT", "P,1,IN,X,INTEGER", "P,2,OUT,Y,INTEGER", "P,3,INOUT,Z,CHARACTER"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11 ROUTINES view] a routine's
/// definition is shown to the owner of its schema, and null to another
/// who holds EXECUTE on it. MC/DC of the owner condition.
#[test]
fn a_definition_is_shown_to_its_owner() {
    let db = defined();
    let a = db.session("A");
    a.run("GRANT EXECUTE ON FUNCTION F TO B").unwrap();
    a.commit().unwrap();
    let defined = "SELECT COALESCE(ROUTINE_DEFINITION, '-') FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'F'";
    assert_eq!(rows(&a, defined), ["RETURN X / 2.0"]);
    assert_eq!(rows(&db.session("B"), defined), ["-"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60, §9.18, Features S203, S204] a
/// function of an array parameter and an array result, invoked with an
/// array, its element read and its cardinality; and a procedure of an
/// array OUT parameter.
#[test]
fn routines_of_arrays() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION AT_TWO (V INTEGER ARRAY[4]) RETURNS INTEGER CONTAINS SQL RETURN V[2]").unwrap();
    a.run("CREATE FUNCTION PAIR (X INTEGER) RETURNS INTEGER ARRAY[2] CONTAINS SQL RETURN ARRAY[X, X * 10]").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT AT_TWO(ARRAY[K, K + 1, K + 2]) FROM T ORDER BY K"), ["2", "3"]);
    assert_eq!(rows(&a, "SELECT CARDINALITY(PAIR(K)), PAIR(K)[2] FROM T ORDER BY K"), ["2,10", "2,20"]);
    assert_eq!(rows(&a, "SELECT AT_TWO(PAIR(K)) FROM T ORDER BY K"), ["10", "20"]);
    a.run("CREATE PROCEDURE PAIRED (IN X INTEGER, OUT V INTEGER ARRAY[2]) READS SQL DATA SELECT PAIR(X) INTO V FROM (VALUES (0)) AS D (Z)")
        .unwrap();
    a.run("CREATE TABLE H (V INTEGER ARRAY[2])").unwrap();
    a.run("CREATE TRIGGER W BEFORE INSERT ON H REFERENCING NEW AS NR FOR EACH ROW CALL PAIRED(3, NR.V)").unwrap();
    a.run("INSERT INTO H VALUES (NULL)").unwrap();
    assert_eq!(rows(&a, "SELECT CARDINALITY(V), V[2] FROM H"), ["2,30"]);
}
