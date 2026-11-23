//! Unit tests: which changes of a compiled program may run straight on
//! their table (RULES.md D841, R-TEST-8).

use std::collections::HashSet;

use super::*;
use crate::parser::Parser;

/// `text`, parsed as a statement of a procedure declaring N and X.
fn parsed(text: &str) -> SqlStatement {
    let declared: HashSet<String> = ["N", "X"].map(str::to_owned).into_iter().collect();
    Parser::for_procedure(text, false, declared).unwrap().sql_statement().unwrap()
}

/// What `shape_of` makes of `text`, as words.
fn shaped(text: &str) -> &'static str {
    match shape_of(&parsed(text)) {
        None => "general",
        Some(Shape::Insert) => "insert",
        Some(Shape::Searched { delete: false, delta: None }) => "update",
        Some(Shape::Searched { delete: true, delta: None }) => "delete",
        Some(Shape::Searched { delete: false, delta: Some(_) }) => "update into",
        Some(Shape::Searched { delete: true, delta: Some(_) }) => "delete into",
    }
}

/// Verifies: [DESIGN] RULES.md D841, a searched UPDATE and DELETE, an
/// INSERT of a query reading no delta table, and a SELECT ... INTO of an
/// UPDATE's FINAL or NEW TABLE or of a DELETE's OLD TABLE alone may run
/// straight; each other change runs the general path. MC/DC pairs of the
/// SELECT ... INTO: its delta table's result against its statement (NEW
/// of a DELETE, OLD of an UPDATE, an INSERT's), and a WHERE, a GROUP BY or
/// a second table reference beside it.
#[test]
fn the_shapes_that_run_straight() {
    for (text, shape) in [
        ("UPDATE S SET Q = Q - N WHERE W = N", "update"),
        ("DELETE FROM S WHERE W = N", "delete"),
        ("INSERT INTO S VALUES (N, N, N)", "insert"),
        ("INSERT INTO S SELECT W, I, Q FROM T WHERE W = N", "insert"),
        ("SELECT Q INTO X FROM FINAL TABLE (UPDATE S SET Q = Q - N WHERE W = N)", "update into"),
        ("SELECT Q INTO X FROM NEW TABLE (UPDATE S SET Q = Q - N WHERE W = N)", "update into"),
        ("SELECT Q INTO X FROM OLD TABLE (DELETE FROM S WHERE W = N)", "delete into"),
    ] {
        assert_eq!(shaped(text), shape, "{text}");
    }
    for text in [
        "SELECT Q INTO X FROM NEW TABLE (DELETE FROM S WHERE W = N)",
        "SELECT Q INTO X FROM OLD TABLE (UPDATE S SET Q = N WHERE W = N)",
        "SELECT Q INTO X FROM FINAL TABLE (INSERT INTO S VALUES (N, N, N))",
        "SELECT Q INTO X FROM FINAL TABLE (UPDATE S SET Q = N WHERE W = N) WHERE Q > 0",
        "SELECT COUNT(*) INTO X FROM OLD TABLE (DELETE FROM S WHERE W = N) GROUP BY Q",
        "SELECT Q INTO X FROM FINAL TABLE (UPDATE S SET Q = N), T",
        "SELECT Q INTO X FROM S WHERE W = N",
        "INSERT INTO S DEFAULT VALUES",
        "INSERT INTO S SELECT Q, Q, Q FROM FINAL TABLE (UPDATE T SET Q = N)",
        "MERGE INTO S USING T ON S.W = T.W WHEN MATCHED THEN UPDATE SET Q = N",
        "SET X = N",
    ] {
        assert_eq!(shaped(text), "general", "{text}");
    }
}
