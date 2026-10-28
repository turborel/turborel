//! The `<between predicate>`, `<in predicate>` with a value list and
//! `<like predicate>` [X3.135-1986 §5.12, §5.13, §5.14], and exact division
//! [X3.135-1986 §5.9 GR6b].

use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{big, code, exec, rows, setup};
use turborel::value::Spelled;

/// Verifies: [X3.135-1986 §5.12 GR2, GR3] "x BETWEEN y AND z" is "x >= y
/// AND x <= z", bounds included, and NOT BETWEEN its negation; a null x is
/// unknown either way; the operands are comparable [SR1]; a selective
/// bound is answered by an ordered index.
#[test]
fn between_is_a_range_5_12() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID BETWEEN 1 AND 2 ORDER BY ID"), Ok(vec!["1".into(), "2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID NOT BETWEEN 1 AND 2"), Ok(vec!["3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID BETWEEN 2 AND 1"), Ok(vec![]), "an empty range");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT BETWEEN -5 AND 11 ORDER BY ID"), Ok(vec!["1".into(), "2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT NOT BETWEEN -5 AND 11"), Ok(vec![]), "row 3's AMT is null: unknown");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID + 1 BETWEEN 2 * 1 AND 3"), Ok(vec!["1".into(), "2".into()]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID BETWEEN 'A' AND 2"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID BETWEEN 1 2"), Code::Syntax);
    let big = big();
    let plan = big.session("APP").physical_plan("SELECT ID FROM T WHERE ID BETWEEN 58 AND 60", &Params::new()).unwrap();
    assert!(plan.contains("IndexScan"), "a selective bound probes an index: {plan}");
    assert_eq!(rows(&big.session("APP"), "SELECT ID FROM T WHERE ID BETWEEN 58 AND 60 ORDER BY ID").unwrap().len(), 3);
}

/// Verifies: [X3.135-1986 §5.13 GR2, §5.16 GR2c-e] "x IN (s1, s2, ...)"
/// is true if x equals one, false if it equals none, unknown otherwise, and
/// NOT IN its negation: so a null x, or a null in the list and no match,
/// leaves the row out either way; the list has two or more values [F]; its
/// values are comparable with x [SR1].
#[test]
fn in_lists_are_any_equal_5_13() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID IN (1, 3) ORDER BY ID"), Ok(vec!["1".into(), "3".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID NOT IN (1, 3)"), Ok(vec!["2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME IN ('BOB', 'ANN  ') ORDER BY ID"), Ok(vec!["1".into(), "2".into()]), "padded");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME NOT IN ('BOB', 'X')"), Ok(vec!["1".into()]), "row 3's NAME is null");
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME IN (USER, 'BOB')"), Ok(vec!["2".into()]));
    let mut p = Params::new()
        .declare("P", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
        .indicator("PI", -1)
        .declare("N", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.execute("SELECT COUNT(*) INTO N FROM T WHERE ID NOT IN (1, P PI)", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "0", "2 = NULL is unknown, so NOT IN is too");
    s.execute("SELECT COUNT(*) INTO N FROM T WHERE ID IN (1, P PI)", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "1");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID IN (1)"), Code::Syntax, "two or more values");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID IN (1, 'A')"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID IN ()"), Code::Syntax);
}

/// Verifies: [X3.135-1986 §5.14 GR2-GR6] LIKE: `_` one character, `%` any
/// number, others themselves, by the partitioning of GR5 with no padding,
/// so a CHAR(6) 'ANN' matches 'ANN   ' and 'ANN%' but not 'ANN'; NOT LIKE
/// its negation, a null value unknown either way; with an escape character
/// [GR2a], `_` and `%` match themselves, and a pattern the escape character
/// cannot partition is a data exception; the left operand is a character
/// string column [F, SR1], the pattern a character string [SR2], the
/// escape character one character [SR3].
#[test]
fn like_matches_patterns_5_14() {
    let db = setup();
    let s = db.session("APP");
    let ids = |sql: &str| rows(&s, &format!("{sql} ORDER BY ID"));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE 'A%'"), Ok(vec!["1".into()]));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE '_O_%'"), Ok(vec!["2".into()]));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE 'ANN'"), Ok(vec![]), "the value is 'ANN   '");
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE 'ANN   '"), Ok(vec!["1".into()]));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME NOT LIKE 'A%'"), Ok(vec!["2".into()]), "row 3's NAME is null");
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE '%'"), Ok(vec!["1".into(), "2".into()]));
    exec(&s, "INSERT INTO T VALUES (4, '50%_x', 0, 0)").unwrap();
    exec(&s, "INSERT INTO T VALUES (5, '500_x', 0, 0)").unwrap();
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE '50%'"), Ok(vec!["4".into(), "5".into()]));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE '50!%%' ESCAPE '!'"), Ok(vec!["4".into()]));
    assert_eq!(ids("SELECT ID FROM T WHERE NAME LIKE '%!_x%' ESCAPE '!'"), Ok(vec!["4".into(), "5".into()]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE 'A!B' ESCAPE '!'"), Code::Data, "no partitioning");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE 'A%' ESCAPE '!!'"), Code::Data, "two characters [ISO/IEC 9075:1992 §8.5 GR3a i]");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID LIKE '1%'"), Code::SyntaxRule, "not a character string");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE 1"), Code::SyntaxRule);
    assert_eq!(ids("SELECT ID FROM T WHERE (NAME) LIKE 'A%'"), Ok(vec!["1".into()]), "any character string [ISO/IEC 9075:1992 §8.5]");
    assert_eq!(ids("SELECT ID FROM T WHERE TRIM(NAME) || 'X' LIKE '%' || 'NX'"), Ok(vec!["1".into()]), "CHAR(6) keeps its spaces");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (NAME, NAME) LIKE 'A%'"), Code::Syntax, "a row");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME NOT = 'A'"), Code::Syntax);
    let plan = s.explain("SELECT ID FROM T WHERE NAME LIKE 'A%' ESCAPE '!'", &Params::new()).unwrap();
    assert!(plan.contains("NAME LIKE 'A%' ESCAPE '!'"), "{plan}");
}

/// Verifies: [X3.135-1986 §5.9 SR4d, GR6b] a quotient of exact numbers
/// loses no leading significant digit: 1 / 3 is not 0 but has 18 digits,
/// 6 / 2 is 3, 1 / 4 is 0.25, and AVG keeps its own scale.
#[test]
fn exact_division_keeps_leading_digits_5_9() {
    let db = setup();
    let s = db.session("APP");
    let one = |sql: &str| rows(&s, &format!("SELECT {sql} FROM T WHERE ID = 1"));
    assert_eq!(one("1 / 3"), Ok(vec!["0.333333333333333333".into()]));
    assert_eq!(one("6 / 2, 1 / 4, -7 / 2"), Ok(vec!["3,0.25,-3.5".into()]));
    assert_eq!(one("AMT / 4"), Ok(vec!["2.625".into()]));
    assert_eq!(one("(1 / 3) * 3"), Ok(vec!["0.999999999999999999".into()]));
    assert_eq!(code(&s, "SELECT 1 / 0 FROM T WHERE ID = 1"), Code::Data);
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID / 3 > 0.5 ORDER BY ID"), Ok(vec!["2".into(), "3".into()]));
}
