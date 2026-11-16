//! SQL/PSM function bodies: a compound statement run where a query
//! evaluates the function [ISO/IEC 9075-4:2006 draft §11.2, Clause 14;
//! ISO/IEC 9075-2:2023 §9.18 GR8b] (Feature P002).

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N), rows (1, 10), (2, 20), (3, 30).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §11.2, §14.1, §14.10, §14.13]
/// A function whose body is a compound statement is evaluated for each
/// row: its loop and its FOR over a table give its RETURN's value.
#[test]
fn a_compound_body_is_evaluated_per_row_11_2() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE FUNCTION FACT (N INTEGER) RETURNS INTEGER CONTAINS SQL \
         BEGIN DECLARE R INTEGER DEFAULT 1; L: LOOP IF N <= 1 THEN LEAVE L; END IF; SET R = R * N; SET N = N - 1; END LOOP L; RETURN R; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION ABOVE (X INTEGER) RETURNS INTEGER READS SQL DATA \
         BEGIN DECLARE C INTEGER DEFAULT 0; FOR R AS SELECT N FROM T DO IF R.N > X THEN SET C = C + 1; END IF; END FOR; RETURN C; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, FACT(K + 1), ABOVE(N) FROM T ORDER BY K"), ["1,2,2", "2,6,1", "3,24,0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §12.1 to §12.4, §14.2, §17.1 P007; ISO/IEC 9075-2:2023 §16.1]
/// A function's local cursor fetches, a NOT FOUND handler ends its loop,
/// GET STACKED DIAGNOSTICS in a handler reads the condition it handles,
/// and a CALL of a procedure that reads SQL-data runs within it, of a
/// query that reads the table through the function alone too.
#[test]
fn cursors_handlers_and_calls_in_a_function_12_1() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE TOP (OUT M INTEGER) READS SQL DATA SELECT MAX(N) INTO M FROM T").unwrap();
    a.run(
        "CREATE FUNCTION SUMMED () RETURNS VARCHAR(40) READS SQL DATA \
         BEGIN DECLARE S, V, M INTEGER DEFAULT 0; DECLARE DONE BOOLEAN DEFAULT FALSE; DECLARE STATE CHAR(5); \
         DECLARE C CURSOR FOR SELECT N FROM T ORDER BY N; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND BEGIN GET STACKED DIAGNOSTICS CONDITION 1 STATE = RETURNED_SQLSTATE; SET DONE = TRUE; END; \
         OPEN C; FETCH C INTO V; WHILE NOT DONE DO SET S = S + V; FETCH C INTO V; END WHILE; CLOSE C; \
         CALL TOP(M); RETURN CAST(S AS VARCHAR(10)) || ' ' || CAST(M AS VARCHAR(10)) || ' ' || STATE; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT SUMMED() FROM T WHERE K = 1"), ["60 30 02000"]);
    // A query that reads no table but through the function, in a new session.
    a.commit().unwrap();
    let b = db.session("A");
    assert_eq!(rows(&b, "SELECT SUMMED() FROM (VALUES (0)) AS ONE (X)"), ["60 30 02000"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR8b ix 1; ISO/IEC 9075-4:2006 draft §17.2]
/// A function whose body ends with no RETURN executed raises SQLSTATE
/// 2F005; SIGNAL in it raises its condition from the query.
#[test]
fn no_return_and_signal_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION NONE (X INTEGER) RETURNS INTEGER CONTAINS SQL BEGIN IF X > 5 THEN RETURN X; END IF; END").unwrap();
    a.run("CREATE FUNCTION REFUSE (X INTEGER) RETURNS INTEGER CONTAINS SQL BEGIN SIGNAL SQLSTATE '75003' SET MESSAGE_TEXT = 'no'; END")
        .unwrap();
    assert_eq!(rows(&a, "SELECT NONE(10) FROM T WHERE K = 1"), ["10"]);
    let e = a.open_cursor("SELECT NONE(1) FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2F005");
    let e = a.open_cursor("SELECT REFUSE(1) FROM T").and_then(|mut c| c.fetch()).unwrap_err();
    assert_eq!((e.sqlstate().as_str(), e.msg.as_str()), ("75003", "no"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §7.17 SR25, §6.12 SR1a; ISO/IEC 9075-4:2006 draft §11.2]
/// A function that possibly modifies SQL-data is invoked in an UPDATE's
/// SET, an INSERT's VALUES, a CALL's argument and a program's value, its
/// changes the statement's; it is refused in a query, a WHERE, a CASE
/// abbreviation and a simple CASE statement's operand [ISO/IEC
/// 9075-4:2006 draft §14.6 SR1a]; the rows an UPDATE updates are found before its SET
/// values invoke it, so a row it inserts is not updated.
#[test]
fn a_function_that_modifies_sql_data_4_35_2() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (V INTEGER)").unwrap();
    a.run("CREATE FUNCTION LOGGED (D INTEGER) RETURNS INTEGER MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (D); RETURN D + 1; END")
        .unwrap();
    a.run("CREATE FUNCTION GROWN (D INTEGER) RETURNS INTEGER MODIFIES SQL DATA BEGIN INSERT INTO T VALUES (D, 0); RETURN D; END").unwrap();
    a.run("CREATE PROCEDURE KEEP (IN X INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES (X * 100)").unwrap();
    a.run("UPDATE T SET N = LOGGED(N) WHERE K < 3").unwrap();
    a.run("INSERT INTO T VALUES (4, LOGGED(40))").unwrap();
    a.run("CALL KEEP(LOGGED(7))").unwrap();
    a.run("BEGIN DECLARE X INTEGER; SET X = LOGGED(1); IF LOGGED(X) > 0 THEN INSERT INTO L VALUES (X); END IF; END").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,11", "2,21", "3,30", "4,41"]);
    assert_eq!(rows(&a, "SELECT V FROM L ORDER BY V"), ["1", "2", "2", "7", "10", "20", "40", "800"]);
    assert_eq!(a.open_cursor("SELECT LOGGED(K) FROM T").err().map(|e| e.code), Some(Code::SyntaxRule));
    for refused in [
        "UPDATE T SET N = 0 WHERE LOGGED(K) > 0",
        "UPDATE T SET N = COALESCE(LOGGED(N), 0)",
        "INSERT INTO L SELECT LOGGED(K) FROM T",
        "BEGIN CASE LOGGED(1) WHEN 2 THEN INSERT INTO L VALUES (0); ELSE INSERT INTO L VALUES (1); END CASE; END",
    ] {
        assert_eq!(a.run(refused).unwrap_err().code, Code::SyntaxRule, "{refused}");
    }
    a.run("UPDATE T SET N = GROWN(K + 10)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,11", "2,12", "3,13", "4,14", "11,0", "12,0", "13,0", "14,0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.16 SR17c, SR25c, CR6 (T285)]
/// A select list item that is a single SQL parameter or SQL variable
/// reference names its column for it, so a FOR's row names it, in a
/// procedure and in a function; such a column has no counterpart in the
/// table, so a cursor of it is read-only and a positioned UPDATE through
/// it is refused, where before it failed as an updatable cursor
/// (regression).
#[test]
fn a_parameters_column_is_named_for_it_7_16() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (V INTEGER)").unwrap();
    a.run("CREATE PROCEDURE P1 (IN P INTEGER) MODIFIES SQL DATA BEGIN FOR R AS SELECT P, K FROM T WHERE K = 1 DO INSERT INTO L VALUES (R.P); END FOR; END")
        .unwrap();
    a.run("CALL P1(7)").unwrap();
    a.run("BEGIN DECLARE V INTEGER DEFAULT 3; FOR R AS SELECT V, K FROM T WHERE K = 1 DO INSERT INTO L VALUES (R.V); END FOR; END")
        .unwrap();
    assert_eq!(rows(&a, "SELECT V FROM L ORDER BY V"), ["3", "7"]);
    a.run(
        "CREATE FUNCTION F1 (P INTEGER) RETURNS INTEGER READS SQL DATA \
         BEGIN DECLARE S INTEGER DEFAULT 0; FOR R AS SELECT P, K FROM T DO SET S = S + R.P; END FOR; RETURN S; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT F1(10) FROM T WHERE K = 1"), ["30"]);
    let e = a
        .run("BEGIN DECLARE V INTEGER DEFAULT 3; FOR R AS C CURSOR FOR SELECT V, K FROM T DO UPDATE T SET K = 2 WHERE CURRENT OF C; END FOR; END")
        .unwrap_err();
    assert_eq!((e.code, e.sqlcode()), (Code::SyntaxRule, -103), "{e:?}");
}
