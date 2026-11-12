//! \[EXTENSION\] The value expressions and predicates of ISO/IEC 9075:1992
//! (RULES.md D32): CASE, NULLIF, COALESCE [§6.9], CAST [§6.10], the string
//! functions and concatenation [§6.6, §6.7, §6.13], scalar and row
//! subqueries [§7.11], comparisons of rows [§8.2], BETWEEN and IN of rows
//! [§8.3, §8.4], quantified comparisons of rows [§8.7], UNIQUE [§8.9],
//! MATCH [§8.10], and IS TRUE, FALSE, UNKNOWN [§8.12].

use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{code, exec, expected, rows, setup};

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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.12 SR2e-SR2i, GR2] a
/// simple CASE's WHEN takes a list of operands, as with Feature F263, and
/// is true if the operand equals any of them: the first such WHEN gives
/// the result. A null operand in the list equals nothing, yet the others
/// decide; a null case operand matches no WHEN. Each operand is comparable
/// with the case operand, else SQLCODE -103, and a dynamic parameter
/// among them is of the first's type; an empty operand is a syntax error
/// (SQLCODE -104).
#[test]
fn when_operand_lists_6_12() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID, CASE ID WHEN 1, 3 THEN 'ODD' WHEN 2, 4, 6 THEN 'EVEN' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,ODD ", "2,EVEN", "3,ODD "]));
    let q = "SELECT ID, CASE ID WHEN 5, 2 THEN 'A' WHEN 2, 3 THEN 'B' ELSE 'C' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,C", "2,A", "3,B"]), "the first WHEN true");
    let q = "SELECT ID, CASE ID WHEN NULLIF(1, 1), 1 THEN 'ONE' ELSE 'NOT' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,ONE", "2,NOT", "3,NOT"]), "a null operand in the list");
    let q = "SELECT ID, CASE AMT WHEN -3, 10.5 THEN 'SOME' ELSE 'NONE' END FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,SOME", "2,SOME", "3,NONE"]), "a null case operand");
    let p = s.prepare("SELECT ID FROM T WHERE CASE ID WHEN 1, ? THEN 1 ELSE 0 END = 1").unwrap();
    let mut opened = s.open_prepared(&p, &[Value::Char("3".into())]).unwrap();
    let mut found = Vec::new();
    while let Some(row) = opened.fetch().unwrap() {
        found.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    found.sort();
    assert_eq!(found, ["1", "3"], "a ? of the first operand's type [§17.6 GR5k]");
    assert_eq!(code(&s, "SELECT CASE ID WHEN 1, 'A' THEN 1 END FROM T"), Code::SyntaxRule, "SR2i, §8.2 SR2");
    assert_eq!(code(&s, "SELECT CASE ID WHEN 1, THEN 1 END FROM T"), Code::Syntax);
    assert_eq!(code(&s, "SELECT CASE WHEN ID = 1, ID = 2 THEN 1 END FROM T"), Code::Syntax, "a searched CASE takes one condition");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.12 SR2c, SR2f, SR2i,
/// Feature F262] a simple CASE's `<when operand>` may be a predicate's
/// second part, the case operand its first: a comparison, BETWEEN, IN of a
/// list or a subquery, LIKE, IS NULL, each with NOT where it takes one, a
/// quantified comparison and MATCH; values and predicates mix in one list.
/// The case operand may be a row, compared with rows, and a pair whose
/// operands are OVERLAPS each. A predicate the case operand cannot begin
/// is refused (SQLCODE -103), and so is OVERLAPS beside other operands;
/// NOT before no predicate's operator is a syntax error (SQLCODE -104).
#[test]
fn predicates_as_when_operands_6_12() {
    let db = setup();
    let s = db.session("APP");
    let case = |cases: &str| rows(&s, &format!("SELECT ID, CASE {cases} END FROM T ORDER BY ID"));
    assert_eq!(case("AMT WHEN < 0 THEN 'NEG' WHEN >= 10 THEN 'BIG' WHEN IS NULL THEN 'NONE'"), expected(&["1,BIG ", "2,NEG ", "3,NONE"]));
    assert_eq!(case("ID WHEN BETWEEN 1 AND 2 THEN 'LOW' WHEN NOT BETWEEN 1 AND 2 THEN 'HIGH'"), expected(&["1,LOW ", "2,LOW ", "3,HIGH"]));
    assert_eq!(
        case("ID WHEN IN (SELECT ID FROM U) THEN 'U' WHEN NOT IN (2, 4) THEN 'NOT' ELSE 'ELSE'"),
        expected(&["1,U   ", "2,ELSE", "3,NOT "])
    );
    assert_eq!(
        case("NAME WHEN LIKE 'A%' THEN 'A' WHEN NOT LIKE 'A%' THEN 'NOT' WHEN IS NOT NULL THEN 'NO'"),
        expected(&["1,A  ", "2,NOT", "3,NULL"])
    );
    assert_eq!(
        case("ID WHEN > ALL (SELECT ID FROM U) THEN 'ABOVE' WHEN = ANY (SELECT ID FROM U) THEN 'IN'"),
        expected(&["1,IN   ", "2,ABOVE", "3,ABOVE"])
    );
    assert_eq!(case("ID WHEN 1, > 2 THEN 'X' ELSE 'Y'"), expected(&["1,X", "2,Y", "3,X"]), "a value and a predicate");
    assert_eq!(
        case("(ID, ID) WHEN (1, 1) THEN 'ONE' WHEN (2, 3), (3, 3) THEN 'SAME' ELSE 'NO'"),
        expected(&["1,ONE ", "2,NO  ", "3,SAME"])
    );
    assert_eq!(case("(ID, 'A') WHEN MATCH (SELECT ID, CODE FROM U) THEN 'M' ELSE 'N'"), expected(&["1,M", "2,N", "3,N"]));
    let pair = "(DATE '2026-01-01', DATE '2026-02-01')";
    let q = format!("SELECT CASE {pair} WHEN OVERLAPS (DATE '2026-01-15', DATE '2026-03-01') THEN 'O' ELSE 'N' END FROM T WHERE ID = 1");
    assert_eq!(rows(&s, &q), expected(&["O"]));
    let mixed = format!("SELECT CASE {pair} WHEN OVERLAPS {pair}, {pair} THEN 1 END FROM T");
    assert_eq!(code(&s, &mixed), Code::SyntaxRule, "SR2c");
    assert_eq!(code(&s, "SELECT CASE ID WHEN LIKE 'A%' THEN 1 END FROM T"), Code::SyntaxRule, "LIKE of a number");
    assert_eq!(code(&s, "SELECT CASE ID WHEN (1, 2) THEN 1 END FROM T"), Code::SyntaxRule, "rows of two degrees");
    assert_eq!(code(&s, "SELECT CASE ID WHEN NOT 1 THEN 1 END FROM T"), Code::Syntax);
    assert_eq!(code(&s, "SELECT CASE ID WHEN IS TRUE THEN 1 END FROM T"), Code::Syntax, "no <boolean test>'s part");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.15, Feature T054]
/// GREATEST and LEAST of two values or more: the greatest or the least, of
/// the type of all [SR6], each cast to it [SR7]; null if any is null
/// [GR1]; of numbers, strings padded with spaces, datetimes and truth
/// values; under the collation of all, the first of two it holds equal
/// [GR3]. Values not comparable are refused (SQLCODE -103) [SR4], one value
/// is a syntax error (SQLCODE -104); a `?` takes the first's type. Neither
/// is a key word, so a column keeps either name.
#[test]
fn greatest_and_least_6_15() {
    let db = setup();
    let s = db.session("APP");
    let one = |e: &str| rows(&s, &format!("SELECT {e} FROM T WHERE ID = 1"));
    assert_eq!(one("GREATEST(1, 2.5, -3), LEAST(1, 2.5, -3), GREATEST(2, 1E0)"), expected(&["2.5,-3.0,2E0"]));
    assert_eq!(
        rows(&s, "SELECT ID, GREATEST(ID, AMT), LEAST(NAME, 'BOB') FROM T ORDER BY ID"),
        expected(&["1,10.50,ANN   ", "2,2.00,BOB   ", "3,NULL,NULL"])
    );
    assert_eq!(
        one("GREATEST('ab', 'ab  ', 'a'), LEAST(DATE '2026-01-01', DATE '2025-12-31'), GREATEST(TRUE, FALSE, FALSE)"),
        expected(&["ab  ,2025-12-31,TRUE"])
    );
    assert_eq!(
        one("GREATEST('a' COLLATE CASE_INSENSITIVE, 'A'), LEAST('B' COLLATE CASE_INSENSITIVE, 'a'), LEAST('B', 'a')"),
        expected(&["a,a,B"])
    );
    s.run("CREATE VIEW V (G) AS SELECT GREATEST(ID, AMT) FROM T").unwrap();
    let described = "SELECT DATA_TYPE, NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'V'";
    assert_eq!(rows(&s, described), expected(&["NUMERIC,12,2"]), "the type of all [§9.5]");
    let p = s.prepare("SELECT GREATEST(ID, ?) FROM T WHERE ID = 1").unwrap();
    let mut opened = s.open_prepared(&p, &[Value::Char("7".into())]).unwrap();
    assert_eq!(opened.fetch().unwrap().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>()), Some(vec!["7".to_owned()]));
    drop(opened);
    assert_eq!(code(&s, "SELECT GREATEST(1, 'A') FROM T"), Code::SyntaxRule, "SR4");
    assert_eq!(code(&s, "SELECT LEAST(DATE '2026-01-01', 1) FROM T"), Code::SyntaxRule, "SR4");
    assert_eq!(code(&s, "SELECT GREATEST(1) FROM T"), Code::Syntax, "two values at least");
    s.run("CREATE TABLE G (GREATEST INTEGER, LEAST INTEGER)").unwrap();
    assert_eq!(rows(&s, "SELECT GREATEST, LEAST FROM G"), expected(&[]), "names of columns");
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR15, GR13] LTRIM,
/// RTRIM and BTRIM remove each character of a set from the start, the end
/// or both, a space if none is written; the source as it is if it or the
/// set is empty; null if either is null; a number refused; varying, as long
/// as the source; LTRIM still a column's name.
#[test]
fn multi_character_trims_6_33() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT LTRIM('xyaxy', 'yx'), RTRIM('xyaxy', 'yx'), BTRIM('xyaxy', 'yx'), BTRIM('  a  ') || '-', RTRIM(NAME) || '-', LTRIM(NAME, 'A') FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["axy,xya,a,a-,ANN-,NN   "]));
    let q = "SELECT LTRIM('', 'x') || '-', RTRIM('ab', '') || '-', BTRIM('xxx', 'x') || '-', LTRIM('a b', ' b') FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["-,ab-,-,a b"]));
    let q = "SELECT LTRIM(NAME), RTRIM('a', NAME), BTRIM(CAST(NULL AS CHAR(1)), 'a') FROM T WHERE ID = 3";
    assert_eq!(rows(&s, q), expected(&["NULL,NULL,NULL"]));
    assert_eq!(code(&s, "SELECT LTRIM(ID) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT BTRIM(NAME, ID) FROM T"), Code::SyntaxRule);
    exec(&s, "CREATE TABLE TRIMS (LTRIM INTEGER, S VARCHAR(4))").unwrap();
    exec(&s, "INSERT INTO TRIMS VALUES (7, 'abc')").unwrap();
    exec(&s, "CREATE VIEW TRIMMED (R) AS SELECT RTRIM(S, 'c') FROM TRIMS").unwrap();
    assert_eq!(rows(&s, "SELECT LTRIM, R FROM TRIMS, TRIMMED"), expected(&["7,ab"]));
    let q = "SELECT CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'TRIMMED'";
    assert_eq!(rows(&s, q), expected(&["4"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR16, GR14] LPAD and
/// RPAD pad with the padding, repeated and cut, to the total length, a
/// space if none is written; cut a longer source; '' of a length less than
/// 1, even of a null padding; the source of an empty padding; at most
/// 32,767 characters; null of a null source, length or padding; a length
/// of a scale or a type not 0 refused; a `?` of a length NUMERIC.
#[test]
fn pads_6_33() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT LPAD('abc', 8, 'xy'), RPAD('abc', 8, 'xy'), RPAD('ab', 4) || '-', LPAD('abcd', 2), RPAD(TRIM(NAME), 3, 'z'), LPAD('a', 3, '') FROM T WHERE ID = 1";
    assert_eq!(rows(&s, q), expected(&["xyxyxabc,abcxyxyx,ab  -,ab,ANN,a"]));
    let null = "CAST(NULL AS VARCHAR(1))";
    let q = format!(
        "SELECT LPAD('a', 0, 'x') || '-', RPAD('a', -5) || '-', RPAD('a', 0, {null}) || '-', CHAR_LENGTH(RPAD('a', 40000, 'bc')) FROM T WHERE ID = 1"
    );
    assert_eq!(rows(&s, &q), expected(&["-,-,-,32767"]));
    let q = format!("SELECT RPAD('a', 2, {null}), LPAD(NAME, 2), RPAD('a', CAST(NULL AS INTEGER)) FROM T WHERE ID = 3");
    assert_eq!(rows(&s, &q), expected(&["NULL,NULL,NULL"]));
    assert_eq!(code(&s, "SELECT LPAD(NAME, 2.5) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT RPAD(NAME, '2') FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT RPAD(NAME, 2, ID) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT RPAD(ID, 2) FROM T"), Code::SyntaxRule);
    let p = s.prepare("SELECT RPAD('ab', ?, '.') FROM T WHERE ID = 1").unwrap();
    let mut c = s.open_prepared(&p, &[Value::Exact(Decimal::new(5, 0))]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("ab...".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.11 GR2; §7.11 SR1, GR1; §8.2 GR7;
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
    // [ISO/IEC 9075-2:2023 §6.46, §8.8] Of SQL:2023, a truth value is a
    // value, null when Unknown, which SQL-92 refused as a syntax error.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE (AMT > 0) IS NULL"), expected(&["3"]));
}
