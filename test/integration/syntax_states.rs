//! The SQLSTATE of a rule broken, run directly and prepared alike, and of
//! a number no type holds, written as a literal and given as text
//! [ISO/IEC 9075-2:2023 §5.3 SR29, SR30, §6.13 GR8, GR9, §20.7 GR6c,
//! §22.1 GR6a, §24.1] (RULES.md D970, D971).

use turborel::db::Session;
use turborel::error::Code;
use turborel::value::{Decimal, MAX_EXACT_PRECISION, Value};

use crate::common::{failure, rows, setup};

/// A literal of `n` nines.
fn nines(n: u32) -> String {
    "9".repeat(usize::try_from(n).unwrap())
}

/// The SQLCODE class and SQLSTATE `sql` fails with as it is prepared.
fn prepared(s: &Session, sql: &str) -> (Code, String) {
    failure(s.prepare(sql))
}

/// The SQLCODE class and SQLSTATE the query `sql` fails with, run with
/// `values` as its parameters.
fn run(s: &Session, sql: &str, values: &[Value]) -> (Code, String) {
    let p = s.prepare(sql).unwrap();
    failure(s.open_prepared(&p, values).and_then(|mut c| c.fetch().map(|_| ())))
}

/// Syntax error or access rule violation, SQLCODE -103.
fn broken() -> (Code, String) {
    (Code::SyntaxRule, "42000".to_owned())
}

/// Numeric value out of range.
fn out_of_range() -> (Code, String) {
    (Code::Data, "22003".to_owned())
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.3 SR29, SR30, §20.7 GR6c] RULES.md
/// D970, D971: a numeric literal no type holds, exact of 39 digits or
/// approximate past DOUBLE PRECISION's range, breaks a Syntax Rule,
/// SQLCODE -103 and SQLSTATE 42000, in a statement prepared as in one run,
/// as a regression test: it was -900, SQLSTATE 0A000, before the fix.
#[test]
fn a_literal_no_type_holds_breaks_a_syntax_rule_prepared_5_3() {
    let db = setup();
    let s = db.session("APP");
    for literal in [nines(MAX_EXACT_PRECISION + 1), "1E400".to_owned(), "-1E400".to_owned()] {
        let sql = format!("SELECT {literal} FROM T");
        assert_eq!(prepared(&s, &sql), broken(), "{sql}");
        assert_eq!(failure(s.run(&format!("INSERT INTO T (ID, AMT) VALUES (9, {literal})"))), broken(), "{literal}");
    }
    let longest = nines(MAX_EXACT_PRECISION);
    let p = s.prepare(&format!("SELECT {longest} FROM T WHERE ID = 1")).unwrap();
    assert_eq!(s.open_prepared(&p, &[]).unwrap().fetch().unwrap().map(|r| r[0].to_string()), Some(longest));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 GR8b ii, GR8a ii, GR9b ii, GR9a
/// ii] RULES.md D970: text of a number no exact type holds, cast to an
/// exact type, is 22003, as a regression test: text of 39 digits was 22018
/// before the fix; cast to an approximate type it is the number, and past
/// that type's range 22003. Text that is no number stays 22018 [GR8b i];
/// a fraction past the greatest scale is truncated, not refused. Given as
/// a prepared statement's parameter, as text or as an exact value past the
/// type, it is 22003 too.
#[test]
fn text_of_a_number_no_type_holds_is_out_of_range_6_13() {
    let db = setup();
    let s = db.session("APP");
    let wide = nines(MAX_EXACT_PRECISION + 1);
    for sql in [
        format!("SELECT CAST('{wide}' AS DECIMAL(38)) FROM T"),
        "SELECT CAST('1E400' AS DOUBLE PRECISION) FROM T".to_owned(),
        "SELECT CAST('1E400' AS DECIMAL(5)) FROM T".to_owned(),
    ] {
        assert_eq!(failure(s.open_cursor(&sql).and_then(|mut c| c.fetch().map(|_| ()))), out_of_range(), "{sql}");
    }
    let approximate = format!("SELECT CAST(' -{wide} ' AS DOUBLE PRECISION) FROM T WHERE ID = 1");
    assert_eq!(rows(&s, &approximate), Ok(vec!["-1E39".into()]));
    let invalid = failure(s.open_cursor("SELECT CAST('9E' AS DECIMAL(5)) FROM T").and_then(|mut c| c.fetch().map(|_| ())));
    assert_eq!(invalid, (Code::Data, "22018".to_owned()));
    let long = format!("0.12{}", "3".repeat(60));
    assert_eq!(rows(&s, &format!("SELECT CAST('{long}' AS DECIMAL(5,2)) FROM T WHERE ID = 1")), Ok(vec!["0.12".into()]));
    let cast = "SELECT CAST(? AS DECIMAL(38)) FROM T WHERE ID = 1";
    assert_eq!(run(&s, cast, &[Value::Char(wide.as_str().into())]), out_of_range());
    assert_eq!(run(&s, cast, &[Value::Exact(Decimal::new(i128::MAX, 0))]), out_of_range());
    assert_eq!(run(&s, "SELECT ID FROM T WHERE F = ?", &[Value::Char("1E400".into())]), out_of_range());
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.7 GR6c, §22.1 GR6a, §24.1] RULES.md
/// D971: a query naming a table that is not there, one of a column that is
/// not, and one not written as its Format says are SQLSTATE 42000 run
/// directly and prepared alike, each with its SQLCODE, as a regression
/// test: prepared they were 37000, run 2A000, before the fix; so too
/// PREPARE's statement that is not preparable.
#[test]
fn a_rule_broken_is_42000_run_directly_or_prepared_20_7() {
    let db = setup();
    let s = db.session("APP");
    let cases = [
        ("SELECT X FROM NOWHERE", Code::SyntaxRule),
        ("SELECT NOTHING FROM T", Code::SyntaxRule),
        ("SELECT FROM T WHERE", Code::Syntax),
        ("SELECT 'a FROM T", Code::Lexical),
    ];
    for (sql, code) in cases {
        let expected = (code, "42000".to_owned());
        assert_eq!(failure(s.open_cursor(sql).map(|_| ())), expected, "{sql} opened");
        assert_eq!(prepared(&s, sql), expected, "{sql} prepared");
    }
    for sql in ["DELETE FROM NOWHERE", "UPDATE T SET NOTHING = 1"] {
        assert_eq!(failure(s.run(sql)), broken(), "{sql} run");
        assert_eq!(prepared(&s, sql), broken(), "{sql} prepared");
    }
    assert_eq!(prepared(&s, "OPEN C"), (Code::SyntaxRule, "42000".to_owned()), "not preparable");
}
