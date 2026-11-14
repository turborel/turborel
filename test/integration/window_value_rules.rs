//! NTILE, LEAD, LAG, FIRST_VALUE, LAST_VALUE and NTH_VALUE refused
//! [ISO/IEC 9075-2:2023 §9.23 SR5, SR6, §9.16]: each Syntax Rule, each in a
//! constraint; and their names and the words of a null treatment still
//! names elsewhere, a function of such a name still callable.

use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, Value};

use crate::common::{code, function_calls, sorted};
use crate::ident::Ident;
use crate::windows::sales;

/// The SQLCODE of `failed`, and whether its error cites `rule`.
fn cites<T: std::fmt::Debug>(failed: std::result::Result<T, SqlError>, rule: &str) -> (Code, bool) {
    let e = failed.expect_err("a failure");
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5a, SR5b, SR5i i,
/// SR5j iii, SR6a ii] NTILE, LEAD and LAG have a window ordering and no
/// frame; a number of tiles or an nth row is exact of scale 0, a literal
/// or a parameter; a default is of a type compatible with the extent's. Each is
/// SQLCODE -103.
#[test]
fn value_function_syntax_rules_9_23() {
    let (_db, a) = sales();
    let refused = [
        ("SELECT NTILE(2) OVER () FROM S", "§9.23 SR5a"),
        ("SELECT NTILE(2) OVER (PARTITION BY R) FROM S", "§9.23 SR5a"),
        ("SELECT LAG(D) OVER () FROM S", "§9.23 SR5a"),
        ("SELECT LEAD(D) OVER (PARTITION BY R) FROM S", "§9.23 SR5a"),
        ("SELECT NTILE(2) OVER (ORDER BY D ROWS UNBOUNDED PRECEDING) FROM S", "§9.23 SR5b"),
        ("SELECT LEAD(D) OVER (ORDER BY D RANGE BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING) FROM S", "§9.23 SR5b"),
        ("SELECT NTILE(1.5) OVER (ORDER BY D) FROM S", "§9.23 SR5i i"),
        ("SELECT NTILE(2E0) OVER (ORDER BY D) FROM S", "§9.23 SR5i i"),
        ("SELECT NTH_VALUE(D, 2.5) OVER (ORDER BY D) FROM S", "§9.23 SR6a ii"),
        ("SELECT LAG(D, 1, 'x') OVER (ORDER BY D) FROM S", "§9.23 SR5j iii"),
        ("SELECT LEAD(R, 1, 5) OVER (ORDER BY D) FROM S", "§9.23 SR5j iii"),
        ("SELECT LAG(D, 1, DATE '2024-01-01') OVER (ORDER BY D) FROM S", "§9.23 SR5j iii"),
    ];
    for (sql, rule) in refused {
        assert_eq!(cites(crate::common::fetched(&a, sql), rule), (Code::SyntaxRule, true), "{sql}");
    }
    for sql in [
        "SELECT LEAD(D, -1) OVER (ORDER BY D) FROM S",
        "SELECT NTILE(D) OVER (ORDER BY D) FROM S",
        "SELECT LEAD(D, 1.5) OVER (ORDER BY D) FROM S",
        "SELECT LAG(D, ?) OVER (ORDER BY D) FROM S",
        "SELECT NTH_VALUE(D, 2) FROM MIDDLE OVER (ORDER BY D) FROM S",
        "SELECT LAG(D) IGNORE OVER (ORDER BY D) FROM S",
    ] {
        assert_eq!(
            code(&a, sql),
            Code::Syntax,
            "{sql}: an <offset> is an <unsigned integer>, a <number of tiles> a <simple value specification>, and the words are as written"
        );
    }
    let framed =
        "SELECT FIRST_VALUE(D) OVER (ORDER BY D ROWS 1 PRECEDING), LAST_VALUE(D) OVER (), NTH_VALUE(D, 1) OVER (PARTITION BY R) FROM S";
    assert_eq!(sorted(&a, framed).len(), 7, "the value functions take any window");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5j iii, §9.2 SR3]
/// LEAD and LAG's default is compatible with the extent if each is
/// assignable to the other: a character string assigned to a type beyond
/// the standard that takes one, but not back, is refused; a value of the
/// type is not.
#[test]
fn defaults_are_assignable_both_ways_9_23() {
    let db = Database::new();
    db.register_type(Box::new(Ident)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K IDENT, N INTEGER)").unwrap();
    let a = db.session("A");
    let one = format!("{:032X}", 1_u8);
    a.run(&format!("INSERT INTO T VALUES (CAST('{one}' AS IDENT), 1)")).unwrap();
    let text = format!("SELECT LAG(K, 1, '{one}') OVER (ORDER BY N) FROM T");
    assert_eq!(cites(crate::common::fetched(&a, &text), "§9.23 SR5j iii"), (Code::SyntaxRule, true));
    let cast = format!("SELECT N, LAG(K, 1, CAST('{one}' AS IDENT)) OVER (ORDER BY N) IS NOT NULL FROM T");
    assert_eq!(sorted(&a, &cast), ["1,TRUE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1c] NTILE, LEAD,
/// LAG, FIRST_VALUE, LAST_VALUE and NTH_VALUE are potential sources of
/// non-determinism, refused in a table's CHECK constraint and in an
/// assertion, SQLCODE -103.
#[test]
fn value_functions_constrain_nothing_9_16() {
    let (_db, a) = sales();
    let calls = [
        "NTILE(2) OVER (ORDER BY D)",
        "LEAD(D) OVER (ORDER BY D)",
        "LAG(D, 1, 0) OVER (ORDER BY D)",
        "FIRST_VALUE(D) OVER (ORDER BY D)",
        "LAST_VALUE(D) IGNORE NULLS OVER ()",
        "NTH_VALUE(D, 2) FROM LAST OVER (PARTITION BY R)",
    ];
    for call in calls {
        let assertion = format!("CREATE ASSERTION N CHECK ((SELECT MAX(X) FROM (SELECT {call} FROM S) AS Q (X)) < 100)");
        assert_eq!(cites(a.run(&assertion), "§9.16 SR1c"), (Code::SyntaxRule, true), "{assertion}");
        let check = format!("CREATE TABLE C (X INTEGER CHECK (X < (SELECT MAX(Y) FROM (SELECT {call} FROM S) AS Q (Y))))");
        assert_eq!(cites(a.run(&check), "§9.16 SR1c"), (Code::SyntaxRule, true), "{check}");
    }
    a.rollback().unwrap();
}

/// A function named LAG: its argument plus one.
struct Lag;

impl ScalarFunction for Lag {
    fn name(&self) -> &'static str {
        "LAG"
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        Ok(args.first().cloned().unwrap_or(DataType::Boolean))
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(match args.first() {
            Some(Value::Exact(d)) => Value::Exact(d.checked_add(Decimal::new(1, 0))?),
            Some(_) | None => Value::Null,
        })
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §6.10] NTILE, LEAD,
/// LAG, FIRST_VALUE, LAST_VALUE, NTH_VALUE, RESPECT and IGNORE are read so
/// only before a window, so each names a column elsewhere, and a function
/// of such a name is called where OVER does not follow its call.
#[test]
fn value_function_words_are_names_elsewhere_5_2() {
    let (db, a) = sales();
    a.run(
        "CREATE TABLE VW (NTILE INTEGER, LEAD INTEGER, LAG INTEGER, FIRST_VALUE INTEGER, LAST_VALUE INTEGER, \
         NTH_VALUE INTEGER, RESPECT INTEGER, IGNORE INTEGER)",
    )
    .unwrap();
    a.run("INSERT INTO VW VALUES (1, 2, 3, 4, 5, 6, 7, 8)").unwrap();
    let all = "SELECT NTILE, LEAD, LAG, FIRST_VALUE, LAST_VALUE, NTH_VALUE, RESPECT, IGNORE FROM VW";
    assert_eq!(sorted(&a, all), ["1,2,3,4,5,6,7,8"]);
    assert_eq!(sorted(&a, "SELECT RESPECT IGNORE FROM VW"), ["7"]);
    assert_eq!(sorted(&a, "SELECT LAG OVER FROM VW"), ["3"]);
    assert_eq!(sorted(&a, "SELECT LAG(LAG) OVER (ORDER BY LEAD) FROM VW"), ["NULL"]);
    db.register_scalar_function(Box::new(Lag)).unwrap();
    function_calls(&a);
    assert_eq!(sorted(&a, "SELECT LAG(LAG), LAG(NTILE) + 1 FROM VW"), ["4,3"]);
    assert_eq!(sorted(&a, "SELECT LAG(LAG(LAG)) OVER (ORDER BY LEAD) FROM VW"), ["NULL"]);
    a.rollback().unwrap();
}
