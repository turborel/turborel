//! The Syntax and General Rules of SQL/PSM assignments, RETURN and SIGNAL
//! [ISO/IEC 9075-4:2006 draft §14.5, §17.2; ISO/IEC 9075-2:2023 §6.25,
//! §16.2] (Features P002, T051): a row assigned to a list of targets, a
//! field of a row variable assigned, and each refused as its rules say.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::failure;

/// Schema A: T (K, N).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.5 GR1, GR4b,
/// SR1f; ISO/IEC 9075-2:2023 §6.25] a field of a row variable is assigned,
/// nested two deep, the rest of the row kept; a row subquery of no row
/// assigns its targets null; NULL assigns null.
#[test]
fn fields_and_rows_assigned_14_5() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "BEGIN DECLARE R ROW (X INTEGER, Y ROW (Z INTEGER)); DECLARE I, J INTEGER DEFAULT 7; \
         SET R = ROW (1, ROW (2)); SET R.Y.Z = 5; INSERT INTO T VALUES ((R).X, (R).Y.Z); \
         SET (I, J) = (SELECT K, N FROM T WHERE K = 99); INSERT INTO T VALUES (COALESCE(I, -1), COALESCE(J, -1)); \
         SET I = NULL; INSERT INTO T VALUES (2, I); END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["-1,-1", "1,5", "2,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.5 SR1b, GR4a,
/// SR1f; ISO/IEC 9075-2:2023 §9.2 SR2, §6.25 SR1, SR2] refused: a field of
/// a null row assigned (2202A); a field of a variable of no row type, or
/// one its row type has not, a row of more values than targets, and a
/// value its target cannot store (SQLCODE -103); a row subquery of two
/// rows (21000).
#[test]
fn assignments_refused_14_5() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    assert_eq!(failure(a.run("BEGIN DECLARE R ROW (X INTEGER); SET R.X = 1; END")).1, "2202A");
    assert_eq!(failure(a.run("BEGIN DECLARE I INTEGER; SET I.X = 1; END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN DECLARE R ROW (X INTEGER); SET R.Q = 1; END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN DECLARE I, J INTEGER; SET (I, J) = (1, 2, 3); END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN DECLARE I INTEGER; SET I = DATE '2024-01-01'; END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN DECLARE I, J INTEGER; SET (I, J) = (SELECT K, N FROM T); END")).1, "21000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.2 SR1, SR3; ISO/IEC
/// 9075-4:2006 draft §17.2 SR1a ii, SR3] refused (SQLCODE -103): RETURN
/// outside a function's body, RETURN of a value of a type not the
/// function's, SIGNAL of a condition not declared, and MESSAGE_TEXT set to
/// a value not a character string.
#[test]
fn return_and_signal_refused_16_2() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(failure(a.run("BEGIN RETURN 1; END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("CREATE FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN DATE '2024-01-01'")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN SIGNAL NOPE; END")).0, Code::SyntaxRule);
    assert_eq!(failure(a.run("BEGIN SIGNAL SQLSTATE '75000' SET MESSAGE_TEXT = 1; END")).0, Code::SyntaxRule);
}
