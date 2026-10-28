//! \[EXTENSION\] The value expressions and predicates of ISO/IEC 9075:1992
//! (RULES.md D32): CASE, NULLIF, COALESCE [§6.9], CAST [§6.10], the string
//! functions and concatenation [§6.6, §6.7, §6.13], scalar and row
//! subqueries [§7.11], comparisons of rows [§8.2], BETWEEN and IN of rows
//! [§8.3, §8.4], quantified comparisons of rows [§8.7], UNIQUE [§8.9],
//! MATCH [§8.10], and IS TRUE, FALSE, UNKNOWN [§8.12].

use turborel::error::Code;

use crate::common::{code, expected, rows, setup};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.9 SR1-SR7, GR1, GR2]: a
/// searched CASE gives the result of its first WHEN true, else its ELSE,
/// NULL if none; a simple CASE compares its operand; NULLIF and COALESCE as
/// the CASEs they are; the result's type is the results' [§9.3], each cast
/// to it; a WHEN after one true is not evaluated; all results NULL, and a
/// string beside a number, are refused.
#[test]
fn case_expressions_6_9() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID, CASE WHEN AMT > 0 THEN 'PLUS' WHEN AMT < 0 THEN 'MINUS' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,PLUS ", "2,MINUS", "3,NULL"]), "padded to CHAR(5)");
    let q = "SELECT ID, CASE ID WHEN 1 THEN 'ONE' WHEN 2 THEN NULL ELSE 'MANY' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,ONE ", "2,NULL", "3,MANY"]));
    assert_eq!(
        rows(&s, "SELECT ID, NULLIF(ID, 2), COALESCE(NAME, 'NONE') FROM T ORDER BY ID"),
        expected(&["1,1,ANN   ", "2,NULL,BOB   ", "3,3,NONE  "])
    );
    assert_eq!(rows(&s, "SELECT ID, COALESCE(AMT, F, ID) FROM T ORDER BY ID"), expected(&["1,1.05E1", "2,-3E0", "3,3E0"]), "approximate");
    assert_eq!(rows(&s, "SELECT CASE WHEN ID = 0 THEN 1 / 0 ELSE ID END FROM T WHERE ID = 1"), expected(&["1"]), "not evaluated");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE CASE WHEN NAME IS NULL THEN 0 ELSE 1 END = 0"), expected(&["3"]));
    assert_eq!(code(&s, "SELECT CASE WHEN ID = 1 THEN NULL END FROM T"), Code::SyntaxRule, "SR5");
    assert_eq!(code(&s, "SELECT CASE WHEN ID = 1 THEN 'A' ELSE 1 END FROM T"), Code::SyntaxRule, "§9.3 SR3a");
    assert_eq!(code(&s, "SELECT CASE ID WHEN 'A' THEN 1 END FROM T"), Code::SyntaxRule, "SR4a");
    assert_eq!(code(&s, "SELECT COALESCE(ID) FROM T"), Code::Syntax);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR2-GR5]: CAST among
/// numbers and strings, NULL staying null; a number's literal padded to a
/// string's length, refused if longer; a string truncated or padded; a
/// string not a numeric literal refused (22018); a number too large refused
/// (22003).
#[test]
fn casts_6_10() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT CAST(AMT AS CHAR(8)), CAST(ID AS FLOAT), CAST(NAME AS CHAR(2)), CAST(NULL AS INTEGER) FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["10.50   ,1E0,AN,NULL"]));
    assert_eq!(
        rows(&s, "SELECT CAST(' -12.5 ' AS DECIMAL(5,1)), CAST('1E2' AS INTEGER), CAST(F AS CHAR(6)) FROM T WHERE ID = 1"),
        expected(&["-12.5,100,1.5E0 "])
    );
    assert_eq!(rows(&s, "SELECT CAST(2.7E0 AS INTEGER), CAST(-2.5 AS INTEGER) FROM T WHERE ID = 1"), expected(&["3,-2"]));
    let state = |sql: &str| s.open_cursor(sql).and_then(|mut c| c.fetch()).map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(state("SELECT CAST('X1' AS INTEGER) FROM T"), Err("22018".into()));
    assert_eq!(state("SELECT CAST(AMT AS CHAR(2)) FROM T WHERE ID = 1"), Err("22001".into()));
    assert_eq!(state("SELECT CAST(1E30 AS INTEGER) FROM T"), Err("22003".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.6 GR1, GR4-GR6; §6.7 GR1,
/// GR2, GR5; §6.13]: concatenation; SUBSTRING from and for, a negative
/// length refused (22011); UPPER and LOWER; TRIM of each side, of a
/// character, one of more than one character refused (22027); POSITION;
/// the lengths; each null if an operand is; a number given one refused.
#[test]
fn string_functions_6_7() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT TRIM(NAME) || '-' || 'X', SUBSTRING(NAME FROM 2), SUBSTRING(NAME FROM 0 FOR 3), SUBSTRING('ABC' FROM 5) FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["ANN-X,NN   ,AN,"]));
    let q = "SELECT UPPER('aBc'), LOWER(NAME), TRIM(LEADING 'x' FROM 'xxAxx'), TRIM(TRAILING FROM '  A  '), TRIM('x' FROM 'xAx') FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["ABC,ann   ,Axx,  A,A"]));
    let q = "SELECT POSITION('N' IN NAME), POSITION('' IN NAME), POSITION('Z' IN NAME), CHAR_LENGTH(NAME), CHARACTER_LENGTH(TRIM(NAME)), OCTET_LENGTH('é'), BIT_LENGTH('A') FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["2,1,0,6,3,2,8"]));
    assert_eq!(rows(&s, "SELECT NAME || 'X', UPPER(NAME), CHAR_LENGTH(NAME) FROM T WHERE ID = 3"), expected(&["NULL,NULL,NULL"]));
    assert_eq!(code(&s, "SELECT UPPER(ID) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT SUBSTRING(NAME FROM 1.5) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT CONVERT(NAME USING X) FROM T"), Code::Unsupported);
    let state = |sql: &str| s.open_cursor(sql).and_then(|mut c| c.fetch()).map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(state("SELECT SUBSTRING(NAME FROM 1 FOR -1) FROM T WHERE ID = 1"), Err("22011".into()));
    assert_eq!(state("SELECT TRIM('xy' FROM NAME) FROM T WHERE ID = 1"), Err("22027".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.11 SR1, GR1, GR2; §8.2 GR7;
/// §8.3; §8.4; §8.7]: a scalar subquery in a select list, null with no
/// row, refused with more than one (21000) or more than one column; rows
/// compared pair by pair, = and <> and in order; with a row subquery;
/// BETWEEN, IN and quantified comparisons of rows.
#[test]
fn rows_and_subqueries_8_2() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID, (SELECT COUNT(*) FROM U WHERE U.ID = T.ID), (SELECT CODE FROM U WHERE ID = 9) FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,2,NULL", "2,0,NULL", "3,0,NULL"]));
    assert_eq!(code(&s, "SELECT (SELECT ID FROM U) FROM T"), Code::Cardinality);
    assert_eq!(code(&s, "SELECT (SELECT ID, CODE FROM U) + 1 FROM T"), Code::SyntaxRule, "degree 2");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, NAME) = (1, 'ANN')"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, NAME) <> (1, 'ANN') ORDER BY ID"), expected(&["2", "3"]), "row 3: 3 <> 1 decides");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (1, NAME) <> (1, 'ANN') ORDER BY ID"), expected(&["2"]), "row 3: NULL <> 'ANN' unknown");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, AMT) < (2, 0) ORDER BY ID"), expected(&["1", "2"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, AMT) >= (2, 0) ORDER BY ID"), expected(&["3"]), "(2, -3) is less");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, 'B') = (SELECT ID, CODE FROM U WHERE CODE = 'B')"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, ID) BETWEEN (1, 0) AND (2, 0) ORDER BY ID"), expected(&["1"]), "(2, 2) > (2, 0)");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, 'A') IN (SELECT ID, CODE FROM U)"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (ID, 'C') > ALL (SELECT ID, CODE FROM U) ORDER BY ID"), expected(&["1", "2", "3"]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (ID, NAME) = (1)"), Code::SyntaxRule, "degrees 2 and 1");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (ID, NAME) IN (1, 2)"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.9 GR2, §8.10 GR2-GR4,
/// §8.12 GR2]: UNIQUE is false only of two equal rows without nulls; MATCH
/// by its type, UNIQUE asking one row; IS TRUE, FALSE, UNKNOWN and their
/// negations are never unknown.
#[test]
fn unique_match_and_boolean_tests_8_9() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE UNIQUE (SELECT ID FROM U) ORDER BY ID"), expected(&[]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE UNIQUE (SELECT CODE FROM U)"), expected(&["3"]));
    assert_eq!(
        rows(&s, "SELECT COUNT(*) FROM T WHERE UNIQUE (SELECT NAME FROM T X WHERE NAME IS NULL OR ID = 3)"),
        expected(&["3"]),
        "nulls never equal"
    );
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID MATCH (SELECT ID FROM U) ORDER BY ID"), expected(&["1"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID MATCH UNIQUE (SELECT ID FROM U)"), expected(&[]), "1 twice");
    assert_eq!(
        rows(&s, "SELECT ID FROM T WHERE (ID, NAME) MATCH (SELECT ID, CODE FROM U) ORDER BY ID"),
        expected(&["3"]),
        "a null matches"
    );
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (1, NAME) MATCH PARTIAL (SELECT ID, CODE FROM U) AND ID = 3"), expected(&["3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (1, NAME) MATCH FULL (SELECT ID, CODE FROM U)"), expected(&[]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (AMT > 0) IS UNKNOWN"), expected(&["3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (AMT > 0) IS NOT TRUE ORDER BY ID"), expected(&["2", "3"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NOT AMT > 0 IS FALSE ORDER BY ID"), expected(&["1", "3"]), "NOT (AMT > 0 IS FALSE)");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (AMT > 0) IS NULL"), Code::Syntax);
}
