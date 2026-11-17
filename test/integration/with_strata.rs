//! The Syntax Rules of a recursive `<with list>` [ISO/IEC 9075-2:2023
//! §7.17 SR3f, SR3j] over the forms of query its elements may take: a
//! collection derived table, a `<with clause>` of the element's query, a
//! table value constructor, a full join, a window of row pattern
//! recognition, and an anchor expression whose non-recursive operand is
//! its second.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: table T of K and V, rows (1, 10), (2, 20) and (3, 30).
fn tables() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, V INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `sql`, each its values joined by commas, or the SQLCODE
/// class of its refusal, as it opens or as a row is fetched.
fn outcome(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut cursor = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// The recursive query N counting from 1 by `rest`, its recursive operand,
/// its rows in order.
fn step(rest: &str) -> String {
    format!("WITH RECURSIVE N (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL {rest}) SELECT * FROM N ORDER BY I")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j ix 5 B] a
/// recursive reference is allowed in a `<from clause>` of the body beside a
/// collection derived table, of an array of the reference's columns or of
/// a subquery not referencing it; a subquery of a collection derived table
/// referencing it is refused (SQLCODE -103), as a subquery anywhere else
/// is, and so is one in a table value constructor of the body.
#[test]
fn collection_derived_tables_and_constructors_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 10").unwrap();
    assert_eq!(outcome(&a, &step("SELECT X + 1 FROM N, UNNEST(ARRAY[I]) AS U (X) WHERE I < 3")).unwrap(), ["1", "2", "3"]);
    let unrelated = step("SELECT I + 1 FROM N, UNNEST(ARRAY(SELECT K FROM T WHERE K = 1)) AS U (X) WHERE I < 3");
    assert_eq!(outcome(&a, &unrelated).unwrap(), ["1", "2", "3"]);
    let referencing = step("SELECT X + 1 FROM T, UNNEST(ARRAY(SELECT I FROM N)) AS U (X) WHERE X < 3");
    assert_eq!(outcome(&a, &referencing), Err(Code::SyntaxRule), "B, a collection's subquery");
    assert_eq!(outcome(&a, &step("VALUES ((SELECT MAX(I) + 1 FROM N))")), Err(Code::SyntaxRule), "B, a constructor's subquery");
    let nulls = "WITH RECURSIVE N (I, J) AS (VALUES (1, 2) UNION ALL VALUES ((SELECT MAX(I) FROM N), NULL)) SELECT * FROM N";
    assert_eq!(outcome(&a, nulls), Err(Code::SyntaxRule), "B, beside a null");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3h, SR3i, SR3j vii 1,
/// ix 5 F] a `<with clause>` of a derived table of the recursive operand
/// names its own queries beside the reference, unless it is recursive
/// (SQLCODE -103); the non-recursive operand of the anchor expression may
/// be its second; a reference in an operand of a FULL join is refused
/// (SQLCODE -103), as it may be given nulls.
#[test]
fn nested_with_clauses_anchors_and_full_joins_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 10").unwrap();
    let nested = step("SELECT I + X FROM N, (WITH B (X) AS (VALUES (1)) SELECT X FROM B) AS B WHERE I < 3");
    assert_eq!(outcome(&a, &nested).unwrap(), ["1", "2", "3"]);
    let recursive = step(
        "SELECT I + X FROM N, (WITH RECURSIVE B (X) AS (VALUES (1) UNION ALL SELECT X + 1 FROM B WHERE X < 1) SELECT X FROM B) AS B WHERE I < 3",
    );
    assert_eq!(outcome(&a, &recursive), Err(Code::SyntaxRule), "SR3h");
    let second =
        "WITH RECURSIVE N (I) AS (SELECT I + 1 FROM N WHERE I < 3 UNION ALL VALUES (CAST(1 AS INTEGER))) SELECT * FROM N ORDER BY I";
    assert_eq!(outcome(&a, second).unwrap(), ["1", "2", "3"]);
    assert_eq!(outcome(&a, &step("SELECT I + 1 FROM N FULL JOIN T ON N.I = T.K WHERE I < 3")), Err(Code::SyntaxRule), "F");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3j ix 5 C] a scalar
/// subquery of the recursive operand's select list holding no set function
/// allows the reference beside it, whether a query specification with a
/// WHERE clause, a table value constructor or a UNION; one holding a set
/// function in its select list is refused (SQLCODE -103).
#[test]
fn set_functions_of_subqueries_beside_the_reference_7_17() {
    let db = tables();
    let a = db.session("A");
    a.run("SET EXECUTOR.MAX_ITERATIONS = 10").unwrap();
    for subquery in ["(SELECT K FROM T WHERE K = 1)", "(VALUES (1))", "(SELECT K FROM T WHERE K = 1 UNION SELECT K FROM T WHERE K = 1)"] {
        let sql = step(&format!("SELECT {subquery} + I FROM N WHERE I < 3"));
        assert_eq!(outcome(&a, &sql).unwrap(), ["1", "2", "3"], "{subquery}");
    }
    assert_eq!(outcome(&a, &step("SELECT (SELECT MIN(K) FROM T WHERE K > 0) + I FROM N WHERE I < 3")), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.17 SR3f] a recursive
/// query contains no row pattern recognition: of a window of its window
/// clause or of a window function of its select list, each refused
/// (SQLCODE -103).
#[test]
fn row_pattern_windows_are_refused_in_recursive_queries_7_17() {
    let db = tables();
    let a = db.session("A");
    let window = "(ORDER BY I MEASURES COUNT(*) AS C ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING PATTERN (P) DEFINE P AS TRUE)";
    let clause = step(&format!("SELECT I + 1 FROM N WHERE I < 3 WINDOW W AS {window}"));
    assert_eq!(outcome(&a, &clause), Err(Code::SyntaxRule), "a window clause");
    let inline = step(&format!("SELECT COUNT(*) OVER {window} FROM N WHERE I < 3"));
    assert_eq!(outcome(&a, &inline), Err(Code::SyntaxRule), "a window function");
}
