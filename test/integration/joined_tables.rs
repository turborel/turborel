//! \[EXTENSION\] Joined tables [ISO/IEC 9075:1992 §7.5]: CROSS, INNER and
//! LEFT, RIGHT, FULL outer joins on a condition, NATURAL and USING joins,
//! UNION joins, nested and in parentheses; and derived tables and AS, with
//! derived column lists [§6.3].

use turborel::error::Code;
use turborel::params::Params;

use crate::common::{code, exec, expected, rows, setup, sorted};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 SR2, SR3, GR1, GR5]: a
/// CROSS join gives every pair; an INNER join, written or implicit, the
/// pairs its condition is true for; LEFT, RIGHT and FULL joins those, and
/// the rows of their preserved operands that match none, the other's
/// columns null; a UNION join only those; a filter over an outer join is
/// applied to its result, not below it; CROSS and UNION take no condition,
/// the others need one.
#[test]
fn joins_on_conditions_7_5() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T CROSS JOIN U"), expected(&["6"]));
    assert_eq!(sorted(&s, "SELECT T.ID, CODE FROM T JOIN U ON T.ID = U.ID"), vec!["1,A  ", "1,B  "]);
    assert_eq!(sorted(&s, "SELECT T.ID, CODE FROM T INNER JOIN U ON T.ID = U.ID AND CODE = 'B'"), vec!["1,B  "]);
    assert_eq!(sorted(&s, "SELECT T.ID, CODE FROM T LEFT JOIN U ON T.ID = U.ID"), vec!["1,A  ", "1,B  ", "2,NULL", "3,NULL"]);
    assert_eq!(sorted(&s, "SELECT T.ID, CODE FROM U RIGHT OUTER JOIN T ON T.ID = U.ID"), vec!["1,A  ", "1,B  ", "2,NULL", "3,NULL"]);
    let full = "SELECT T.ID, U.CODE FROM T FULL JOIN U ON T.ID = U.ID AND U.CODE = 'B'";
    assert_eq!(sorted(&s, full), vec!["1,B  ", "2,NULL", "3,NULL", "NULL,A  "]);
    assert_eq!(rows(&s, "SELECT COUNT(*), COUNT(T.ID), COUNT(U.ID) FROM T UNION JOIN U"), expected(&["5,3,2"]));
    assert_eq!(sorted(&s, "SELECT T.ID FROM T LEFT JOIN U ON T.ID = U.ID WHERE U.ID IS NULL"), vec!["2", "3"], "filtered after");
    assert_eq!(sorted(&s, "SELECT T.ID FROM T LEFT JOIN U ON T.ID = U.ID AND U.ID IS NULL"), vec!["1", "2", "3"], "matched none");
    let on_subquery = "SELECT T.ID, CODE FROM T LEFT JOIN U ON U.CODE = (SELECT MAX(CODE) FROM U) AND T.ID = U.ID";
    assert_eq!(sorted(&s, on_subquery), vec!["1,B  ", "2,NULL", "3,NULL"]);
    assert_eq!(code(&s, "SELECT * FROM T CROSS JOIN U ON T.ID = U.ID"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT * FROM T UNION JOIN U ON T.ID = U.ID"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT * FROM T JOIN U"), Code::SyntaxRule, "SR2c");
    assert_eq!(code(&s, "SELECT * FROM T LEFT JOIN U"), Code::SyntaxRule);
    let plan = s.explain("SELECT T.ID FROM T LEFT JOIN U ON T.ID = U.ID", &Params::new()).unwrap();
    assert!(plan.contains("LeftOuterJoin"), "{plan}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 SR4a, §6.3 SR2]: a join
/// condition references the columns of its two operands, not another table
/// of the FROM clause; every table of a joined table is exposed to the
/// query, nested joins in parentheses among them.
#[test]
fn join_scopes_7_5() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT * FROM T, U JOIN U X ON T.ID = X.ID"), Code::SyntaxRule, "T is outside the join");
    let nested = "SELECT T.ID, X.CODE, Y.CODE FROM T JOIN (U X JOIN U Y ON X.CODE = Y.CODE) ON T.ID = X.ID";
    assert_eq!(sorted(&s, nested), vec!["1,A  ,A  ", "1,B  ,B  "]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T A JOIN T B ON A.ID < B.ID JOIN T C ON B.ID < C.ID"), expected(&["1"]));
    assert_eq!(code(&s, "SELECT * FROM T JOIN T ON T.ID = T.ID"), Code::SyntaxRule, "T exposed twice");
    assert_eq!(code(&s, "SELECT * FROM (T) JOIN U ON 1 = 1"), Code::Syntax, "only a joined table in parentheses");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 SR6, SR9, GR1d, GR6]: a
/// NATURAL join equates the columns of the same name, a USING join those it
/// names; its result has each such column once, first, as COALESCE gives
/// it, which a column reference unqualified names and * gives, and which
/// an operand's qualifier does not name [§6.4 SR2b]; a NATURAL join with no
/// column in common is a product; a USING column not of each operand once
/// is refused.
#[test]
fn natural_and_using_joins_7_5() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(sorted(&s, "SELECT * FROM T NATURAL JOIN U"), vec!["1,ANN   ,10.50,1.5E0,A  ", "1,ANN   ,10.50,1.5E0,B  "]);
    assert_eq!(sorted(&s, "SELECT ID, T.NAME, U.CODE FROM T NATURAL JOIN U"), vec!["1,ANN   ,A  ", "1,ANN   ,B  "]);
    assert_eq!(code(&s, "SELECT T.ID FROM T NATURAL JOIN U"), Code::SyntaxRule, "§6.4 SR2b");
    assert_eq!(code(&s, "SELECT ID FROM T JOIN U USING (ID) WHERE U.ID = 1"), Code::SyntaxRule, "§6.4 SR2b");
    assert_eq!(code(&s, "SELECT ID FROM T, U"), Code::SyntaxRule, "ambiguous without the join");
    assert_eq!(sorted(&s, "SELECT ID, CODE FROM T LEFT JOIN U USING (ID)"), vec!["1,A  ", "1,B  ", "2,NULL", "3,NULL"]);
    let coalesced = "SELECT NAME FROM T FULL JOIN (SELECT CODE FROM U) AS X (NAME) USING (NAME)";
    assert_eq!(sorted(&s, coalesced), vec!["A     ", "ANN   ", "B     ", "BOB   ", "NULL"], "CHARACTER(6)");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T NATURAL JOIN (SELECT CODE FROM U) X"), expected(&["6"]), "no column in common");
    assert_eq!(code(&s, "SELECT * FROM T JOIN U USING (CODE)"), Code::SyntaxRule, "not of T");
    assert_eq!(code(&s, "SELECT * FROM T NATURAL JOIN U ON T.ID = U.ID"), Code::SyntaxRule, "SR2a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.3 SR1, SR6, SR7; §7.11]: a
/// derived table, with AS or not, its columns named by its query or by a
/// derived column list, as many as it has, none twice; a correlation name
/// with AS, and with a derived column list, on a table; a derived table in
/// a subquery referencing the enclosing query; a derived table with no
/// correlation name refused.
#[test]
fn derived_tables_6_3() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT X.N FROM (SELECT ID FROM T WHERE ID > 1) AS X (N) ORDER BY N"), expected(&["2", "3"]));
    assert_eq!(rows(&s, "SELECT ID FROM (SELECT ID FROM T) X WHERE ID = 1"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT K FROM T AS A (K, NM, AM, FL) WHERE NM = 'BOB'"), expected(&["2"]));
    assert_eq!(rows(&s, "SELECT MAX(C) FROM (SELECT COUNT(*) FROM U GROUP BY ID) AS G (C)"), expected(&["2"]));
    let correlated = "SELECT ID FROM T WHERE EXISTS (SELECT * FROM (SELECT CODE FROM U WHERE U.ID = T.ID) AS X)";
    assert_eq!(rows(&s, correlated), expected(&["1"]), "an outer reference of the enclosing query");
    assert_eq!(code(&s, "SELECT * FROM (SELECT ID FROM T) AS X (A, B)"), Code::SyntaxRule, "SR7");
    assert_eq!(code(&s, "SELECT * FROM T AS A (K, K, M, N)"), Code::SyntaxRule, "SR6");
    assert_eq!(code(&s, "SELECT * FROM (SELECT ID FROM T)"), Code::Syntax);
    assert_eq!(code(&s, "SELECT ID FROM T AS A WHERE T.ID = 1"), Code::SyntaxRule, "A hides T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 SR10, §11.19]: a view of
/// a joined table is read as its rows, and is read-only.
#[test]
fn views_of_joined_tables_7_5() {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION V CREATE TABLE A (K INTEGER) CREATE TABLE B (K INTEGER, C CHAR(1)) CREATE VIEW AB AS SELECT A.K, C FROM A LEFT JOIN B ON A.K = B.K").unwrap();
    let v = db.session("V");
    for sql in ["INSERT INTO A VALUES (1)", "INSERT INTO A VALUES (2)", "INSERT INTO B VALUES (1, 'X')"] {
        v.run(sql).unwrap();
    }
    assert_eq!(sorted(&v, "SELECT K, C FROM AB"), vec!["1,X", "2,NULL"]);
    assert_eq!(exec(&v, "UPDATE AB SET C = 'Y'"), Err(Code::SyntaxRule), "read-only");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 F]: a join's second
/// operand is a `<table reference>`, so a joined table: in "A JOIN B LEFT
/// JOIN C ON c1 ON c2" the LEFT join of B and C is the second operand, the
/// first ON its, the second that of the join of A and it; without the inner
/// ON, the joins are taken left to right.
#[test]
fn a_joined_table_is_a_second_operand_7_5() {
    let db = setup();
    let s = db.session("APP");
    let nested = "SELECT T.ID, U.CODE, X.CODE FROM T JOIN U LEFT JOIN U AS X ON X.CODE = 'B' AND U.CODE = 'A' ON T.ID = U.ID";
    assert_eq!(sorted(&s, nested), vec!["1,A  ,B  ", "1,B  ,NULL"]);
    let chained = "SELECT T.ID, U.CODE FROM T JOIN U ON T.ID = U.ID LEFT JOIN U AS X ON X.CODE = 'Z'";
    assert_eq!(sorted(&s, chained), vec!["1,A  ", "1,B  "]);
    assert_eq!(code(&s, "SELECT * FROM T JOIN U JOIN U AS X ON 1 = 1"), Code::SyntaxRule, "the outer join's ON is missing");
}
