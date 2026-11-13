//! UNNEST, the `<collection derived table>` [ISO/IEC 9075-2:2023 §7.6
//! SR8, §7.10]: the rows of its arrays' elements, with their ordinals,
//! of each row of the FROM items before it, and as the second operand of a
//! join.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, rows, sorted};

/// Schema A: table T of an INTEGER K and an INTEGER ARRAY[3] A, holding
/// (1, [1, 2, 3]), (2, [4]), (3, null), (4, [null, 5]) and (5, []).
fn arrays() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, A INTEGER ARRAY[3])").unwrap();
    let a = db.session("A");
    let rows =
        "(1, ARRAY[1, 2, 3]), (2, ARRAY[4]), (3, NULL), (4, ARRAY[CAST(NULL AS INTEGER), 5]), (5, CAST(ARRAY[] AS INTEGER ARRAY[1]))";
    a.run(&format!("INSERT INTO T VALUES {rows}")).unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `sql`, in order, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR8f to SR8l] UNNEST is a row for
/// each ordinal from 1 to the greatest cardinality of its arrays, a null
/// array's 0 [SR8f]: each array's element of the ordinal, null past its
/// cardinality [SR8k], then WITH ORDINALITY the ordinal, of an exact
/// numeric type of scale 0, INTEGER (IV158). An array of no elements has
/// no rows.
#[test]
fn elements_and_ordinals_7_6() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT * FROM UNNEST(ARRAY[1, 2, 3]) AS U"), ["1", "2", "3"]);
    let padded = "SELECT * FROM UNNEST(ARRAY[1, 2, 3], ARRAY['a']) WITH ORDINALITY AS U (V, W, N) ORDER BY N";
    assert_eq!(query(&a, padded), ["1,a,1", "2,NULL,2", "3,NULL,3"], "SR8k");
    assert_eq!(query(&a, "SELECT * FROM UNNEST(CAST(NULL AS INTEGER ARRAY[2]), ARRAY[7]) AS U (V, W)"), ["NULL,7"], "SR8f");
    assert_eq!(query(&a, "SELECT * FROM UNNEST(CAST(ARRAY[] AS INTEGER ARRAY[2])) AS U"), Vec::<String>::new());
    a.run("CREATE VIEW V AS SELECT * FROM UNNEST(ARRAY[1]) WITH ORDINALITY AS U (E, N)").unwrap();
    let typed = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V'";
    assert_eq!(sorted(&a, typed), ["E,NUMERIC", "N,INTEGER"], "the literal 1 is NUMERIC(1,0)");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR8c, SR8m, SR8n] an array of a
/// row type is spread into the row's fields, named as the fields; a
/// derived column list names every column, as many as there are; and
/// each collection is an array.
#[test]
fn columns_named_7_6() {
    let db = arrays();
    let a = db.session("A");
    let spread = "SELECT * FROM UNNEST(ARRAY[ROW(1, 'x'), ROW(2, 'y')]) AS U";
    assert_eq!(query(&a, spread), ["1,x", "2,y"], "SR8n");
    let named = "SELECT B FROM UNNEST(CAST(ARRAY[ROW(1, 'x')] AS ROW(A INTEGER, B CHARACTER(1)) ARRAY[1])) AS U";
    assert_eq!(query(&a, named), ["x"], "the fields' names");
    assert_eq!(code(&a, "SELECT * FROM UNNEST(ARRAY[1]) AS U (A, B)"), Code::SyntaxRule, "SR8m i 2");
    assert_eq!(code(&a, "SELECT * FROM UNNEST(ARRAY[1], ARRAY[2]) AS U (A, A)"), Code::SyntaxRule, "one name twice");
    assert_eq!(code(&a, "SELECT * FROM UNNEST(5) AS U"), Code::SyntaxRule, "SR8c");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR20a, §7.10 GR] UNNEST reads
/// the FROM items before it, each of their rows giving its rows: after a
/// comma, as the second operand of CROSS and INNER JOIN, of a NATURAL
/// join's columns, in a correlated subquery, and grouped; not those after
/// it.
#[test]
fn lateral_7_6() {
    let db = arrays();
    let a = db.session("A");
    let expected = ["1,1", "1,2", "1,3", "2,4", "4,5", "4,NULL"];
    assert_eq!(sorted(&a, "SELECT T.K, U.E FROM T, UNNEST(T.A) AS U (E)"), expected);
    assert_eq!(sorted(&a, "SELECT T.K, U.E FROM T CROSS JOIN UNNEST(A) AS U (E)"), expected);
    assert_eq!(sorted(&a, "SELECT T.K, U.E FROM T JOIN UNNEST(T.A) AS U (E) ON U.E > 2"), ["1,3", "2,4", "4,5"]);
    assert_eq!(sorted(&a, "SELECT K, E FROM (T NATURAL JOIN T AS S) CROSS JOIN UNNEST(A) AS U (E) WHERE K = 2"), ["2,4"]);
    let counted = "SELECT K, (SELECT COUNT(*) FROM UNNEST(T.A) AS U) FROM T";
    assert_eq!(sorted(&a, counted), ["1,3", "2,1", "3,0", "4,2", "5,0"]);
    let grouped = "SELECT CARDINALITY(A), COUNT(*) FROM T, UNNEST(A) AS U (E) WHERE E IS NOT NULL GROUP BY A";
    assert_eq!(code(&a, grouped), Code::SyntaxRule, "§9.12 SR3: A is not grouped");
    assert_eq!(sorted(&a, "SELECT E, COUNT(*) FROM T, UNNEST(A) AS U (E) WHERE K < 3 GROUP BY E"), ["1,1", "2,1", "3,1", "4,1"]);
    assert_eq!(code(&a, "SELECT * FROM UNNEST(A) AS U, T"), Code::SyntaxRule, "no item after it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 GR1d] UNNEST as the second
/// operand of a LEFT join keeps each row of the first none of whose rows
/// its condition makes true, its own columns null. MC/DC of a row kept:
/// K = 1, some element over 1, against K = 2, whose only element 4 is,
/// and K = 4 whose 5 is; K = 3 and K = 5, of no elements, kept padded.
#[test]
fn left_joined_7_10() {
    let db = arrays();
    let a = db.session("A");
    let left = "SELECT T.K, U.E, U.N FROM T LEFT JOIN UNNEST(T.A) WITH ORDINALITY AS U (E, N) ON U.E > 3";
    assert_eq!(sorted(&a, left), ["1,NULL,NULL", "2,4,1", "3,NULL,NULL", "4,5,2", "5,NULL,NULL"]);
    let all = "SELECT T.K, U.E FROM T LEFT JOIN UNNEST(T.A) AS U (E) ON TRUE WHERE T.K IN (3, 4)";
    assert_eq!(sorted(&a, all), ["3,NULL", "4,5", "4,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.10 SR4; RULES.md D68] the second
/// operand of a RIGHT or FULL join references none of its first in a
/// collection derived table (SQLCODE -103); one in such a join that
/// references a FROM item outside it is valid but not taken (SQLCODE
/// -900).
#[test]
fn not_lateral_across_an_outer_join_7_10() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(code(&a, "SELECT T.K, U.E FROM T RIGHT JOIN UNNEST(T.A) AS U (E) ON TRUE"), Code::SyntaxRule, "SR4");
    assert_eq!(code(&a, "SELECT T.K, U.E FROM T FULL JOIN UNNEST(T.A) AS U (E) ON TRUE"), Code::SyntaxRule, "SR4");
    let outside = "SELECT * FROM T AS X, T AS Y FULL JOIN UNNEST(X.A) AS U (E) ON TRUE";
    assert_eq!(code(&a, outside), Code::Unsupported);
}
