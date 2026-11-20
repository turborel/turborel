//! The Syntax Rules of a recursive `<with list>` [ISO/IEC 9075-2:2023
//! §7.17 SR3j ix] over the table references of a `<from clause>` other
//! than a table or a derived table: a JSON table, a graph table and a
//! graph algorithm's table function.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, V), rows (1, 10), (2, 20) and (3, 30); E (S, D) of
/// edges 1 to 2 and 2 to 3; and the graph G over T and E.
fn tables() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER PRIMARY KEY, V INTEGER) CREATE TABLE E (S INTEGER, D INTEGER)")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.run("INSERT INTO E VALUES (1, 2), (2, 3)").unwrap();
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (T KEY (K) PROPERTIES (K, V))").unwrap();
    a.run("SET EXECUTOR.MAX_ITERATIONS = 10").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `sql`, each its values joined by commas, or the SQLCODE
/// class of its refusal.
fn outcome(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut cursor = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// The recursive query N counting from 1 by `rest`, its recursive
/// operand, its rows in order.
fn step(rest: &str) -> String {
    format!("WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL {rest}) SELECT * FROM N ORDER BY I")
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 SR3j ix 2, ix 5 B] a JSON table
/// beside the recursive reference is allowed when its operands do not
/// reference the stratum; one whose operand is a subquery of the
/// reference is refused (SQLCODE -103), as such a subquery is not in a
/// `<from clause>`.
#[test]
fn json_tables_beside_a_recursive_reference_7_17() {
    let db = tables();
    let a = db.session("A");
    let plain = step("SELECT I + J FROM N, JSON_TABLE('[1]', 'lax $[*]' COLUMNS (J INTEGER PATH 'lax $')) AS JT WHERE I < 3");
    assert_eq!(outcome(&a, &plain).unwrap(), ["1", "2", "3"]);
    let referencing = step(
        "SELECT I + 1 FROM T, JSON_TABLE((SELECT CAST(MAX(I) AS VARCHAR(10)) FROM N), 'lax $' COLUMNS (I INTEGER PATH 'lax $')) AS JT WHERE I < 3",
    );
    assert_eq!(outcome(&a, &referencing), Err(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 SR3j ix 2, ix 5 B] a graph table
/// beside the recursive reference is allowed when it does not reference
/// the stratum; one whose WHERE clause has a subquery of the reference is
/// refused (SQLCODE -103), beside the reference or alone.
#[test]
fn graph_tables_beside_a_recursive_reference_7_17() {
    let db = tables();
    let a = db.session("A");
    let plain = step("SELECT I + 1 FROM N, GRAPH_TABLE (G MATCH (x) WHERE x.K = 1 COLUMNS (x.K AS X)) AS GT WHERE I < 3");
    assert_eq!(outcome(&a, &plain).unwrap(), ["1", "2", "3"]);
    let twice = step("SELECT I + 1 FROM N, GRAPH_TABLE (G MATCH (x) WHERE x.K IN (SELECT I FROM N) COLUMNS (x.K AS X)) AS GT WHERE I < 3");
    assert_eq!(outcome(&a, &twice), Err(Code::SyntaxRule), "twice in one query specification");
    let alone = step("SELECT X + 1 FROM GRAPH_TABLE (G MATCH (x) WHERE x.K IN (SELECT I FROM N) COLUMNS (x.K AS X)) AS GT WHERE X < 3");
    assert_eq!(outcome(&a, &alone), Err(Code::SyntaxRule), "a subquery of a graph table");
}

/// Verifies: [EXTENSION] RULES.md D145 [ISO/IEC 9075-2:2023 §7.17 SR3j ix
/// 2] a graph algorithm's table function whose table argument names the
/// recursive query is a reference to it, so one beside another reference
/// is refused (SQLCODE -103) as twice in one `<query specification>`.
#[test]
fn graph_algorithm_tables_of_a_recursive_reference_7_17() {
    let db = tables();
    let a = db.session("A");
    let twice = "WITH RECURSIVE P (S, D) AS (SELECT S, D FROM E UNION ALL \
                 SELECT P.S, C.VERTEX FROM P, GRAPH_ALGO.SCC(TABLE (P)) AS C WHERE P.S < 0) SELECT * FROM P";
    assert_eq!(outcome(&a, twice), Err(Code::SyntaxRule));
    let unrelated = "WITH RECURSIVE P (S, D) AS (SELECT S, D FROM E UNION ALL \
                     SELECT P.S, C.VERTEX FROM P, GRAPH_ALGO.SCC(TABLE (E)) AS C WHERE P.D = C.VERTEX AND P.S < 0) SELECT COUNT(*) FROM P";
    assert_eq!(outcome(&a, unrelated).unwrap(), ["2"]);
}
