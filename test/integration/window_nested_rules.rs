//! Nested window functions refused [ISO/IEC 9075-2:2023 §6.11 SR1, SR3,
//! SR4]: each place SR1 leaves out, a default of no type in common, an
//! offset not exact of scale 0 or not a `<simple value specification>`;
//! their words still names elsewhere, functions of their names still
//! callable, AT TIME ZONE read in a value expression; and a constraint of
//! them taken, as §9.16 refuses none.

use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, Value};

use crate::common::{code, function_calls, sorted};
use crate::windows::sales;

/// The SQLCODE of `failed`, and whether its error cites `rule`.
fn cites<T: std::fmt::Debug>(failed: std::result::Result<T, SqlError>, rule: &str) -> (Code, bool) {
    let e = failed.expect_err("a failure");
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 SR1, §9.20] a nested
/// window function is only in the aggregated argument of a set function a
/// window function immediately contains: not in a select list or a search
/// condition, a set function of the groups, even one in a window
/// function's argument, a window's ordering, LEAD, LAG or a value
/// function. Each is SQLCODE -103.
#[test]
fn nested_functions_are_only_in_a_window_set_function_6_11() {
    let (_db, a) = sales();
    for sql in [
        "SELECT ROW_NUMBER(CURRENT_ROW) FROM F",
        "SELECT I FROM F WHERE VALUE_OF(V AT CURRENT_ROW) > 1",
        "SELECT SUM(V) OVER (ORDER BY I) + ROW_NUMBER(CURRENT_ROW) FROM F",
        "SELECT SUM(ROW_NUMBER(FRAME_ROW)) FROM F",
        "SELECT K FROM F GROUP BY K HAVING MAX(ROW_NUMBER(FRAME_ROW)) > 1",
        "SELECT SUM(SUM(ROW_NUMBER(FRAME_ROW))) OVER () FROM F",
        "SELECT SUM(V) OVER (ORDER BY ROW_NUMBER(CURRENT_ROW)) FROM F",
        "SELECT LAG(VALUE_OF(V AT CURRENT_ROW)) OVER (ORDER BY I) FROM F",
        "SELECT LEAD(I, 1, ROW_NUMBER(CURRENT_ROW)) OVER (ORDER BY I) FROM F",
        "SELECT FIRST_VALUE(ROW_NUMBER(FRAME_ROW)) OVER (ORDER BY I) FROM F",
    ] {
        assert_eq!(cites(crate::common::fetched(&a, sql), "§6.11 SR1"), (Code::SyntaxRule, true), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.11 SR3, SR4, §9.5]
/// VALUE_OF's value expression and default have a type in common; a
/// `<row marker offset>` is exact of scale 0, SQLCODE -103 if not, and a
/// `<simple value specification>`, a syntax error if not.
#[test]
fn defaults_and_offsets_are_typed_6_11() {
    let (_db, a) = sales();
    for (sql, rule) in [
        ("SELECT SUM(VALUE_OF(V AT CURRENT_ROW, 'x')) OVER () FROM F", "§6.11 SR3"),
        ("SELECT MAX(VALUE_OF(V AT CURRENT_ROW, DATE '2024-01-01')) OVER () FROM F", "§6.11 SR3"),
        ("SELECT SUM(VALUE_OF(V AT CURRENT_ROW + 1.5)) OVER () FROM F", "§6.11 SR4"),
        ("SELECT SUM(VALUE_OF(V AT CURRENT_ROW - 1E0)) OVER () FROM F", "§6.11 SR4"),
        ("SELECT SUM(VALUE_OF(V AT CURRENT_ROW - '1')) OVER () FROM F", "§6.11 SR4"),
    ] {
        assert_eq!(cites(crate::common::fetched(&a, sql), rule), (Code::SyntaxRule, true), "{sql}");
    }
    for sql in [
        "SELECT SUM(VALUE_OF(V AT CURRENT_ROW + I)) OVER () FROM F",
        "SELECT SUM(VALUE_OF(V AT CURRENT_ROW + (1))) OVER () FROM F",
        "SELECT SUM(VALUE_OF(V AT CURRENT_ROW * 2)) OVER () FROM F",
    ] {
        assert_eq!(code(&a, sql), Code::Syntax, "{sql}: an offset is a <simple value specification> after VALUE_OF's marker");
    }
}

/// A function of one argument, its argument plus one, named as asked.
struct PlusOne(&'static str);

impl ScalarFunction for PlusOne {
    fn name(&self) -> &'static str {
        self.0
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §6.11] VALUE_OF and
/// the row markers are read so only in a nested window function: each
/// names a column elsewhere; ROW_NUMBER of no marker and VALUE_OF of no
/// AT and marker call functions of those names; AT TIME ZONE and AT LOCAL
/// are read in VALUE_OF's value expression, before its AT.
#[test]
fn nested_window_words_are_names_elsewhere_5_2() {
    let (db, a) = sales();
    a.run(
        "CREATE TABLE NW (BEGIN_PARTITION INTEGER, BEGIN_FRAME INTEGER, CURRENT_ROW INTEGER, FRAME_ROW INTEGER, END_FRAME INTEGER, \
         END_PARTITION INTEGER, VALUE_OF INTEGER)",
    )
    .unwrap();
    a.run("INSERT INTO NW VALUES (1, 2, 3, 4, 5, 6, 7)").unwrap();
    let all = "SELECT BEGIN_PARTITION, BEGIN_FRAME, CURRENT_ROW, FRAME_ROW, END_FRAME, END_PARTITION, VALUE_OF FROM NW";
    assert_eq!(sorted(&a, all), ["1,2,3,4,5,6,7"]);
    assert_eq!(sorted(&a, "SELECT SUM(CURRENT_ROW + VALUE_OF) OVER (ORDER BY FRAME_ROW) FROM NW"), ["10"]);
    assert_eq!(sorted(&a, "SELECT SUM(VALUE_OF(VALUE_OF AT CURRENT_ROW)) OVER () FROM NW"), ["7"]);
    db.register_scalar_function(Box::new(PlusOne("VALUE_OF"))).unwrap();
    db.register_scalar_function(Box::new(PlusOne("ROW_NUMBER"))).unwrap();
    function_calls(&a);
    assert_eq!(sorted(&a, "SELECT VALUE_OF(VALUE_OF), ROW_NUMBER(FRAME_ROW + 1), VALUE_OF((CURRENT_ROW)) FROM NW"), ["8,6,4"]);
    assert_eq!(sorted(&a, "SELECT SUM(VALUE_OF(VALUE_OF AT CURRENT_ROW - 1, VALUE_OF(END_FRAME))) OVER () FROM NW"), ["6"]);
    let zoned = "SELECT MAX(VALUE_OF(TS AT TIME ZONE INTERVAL '01:00' HOUR TO MINUTE AT CURRENT_ROW)) OVER (ORDER BY TS ROWS CURRENT ROW) \
                 FROM H ORDER BY 1";
    let plain = "SELECT TS AT TIME ZONE INTERVAL '01:00' HOUR TO MINUTE FROM H ORDER BY 1";
    assert_eq!(sorted(&a, zoned), sorted(&a, plain));
    let local = "SELECT MAX(VALUE_OF(TS AT LOCAL AT END_PARTITION)) OVER () FROM H";
    assert_eq!(sorted(&a, local), sorted(&a, "SELECT MAX(TS AT LOCAL) OVER () FROM H"));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1] no nested window
/// function is a potential source of non-determinism, so a CHECK
/// constraint and an assertion of them are taken.
#[test]
fn nested_functions_may_be_in_a_constraint_9_16() {
    let (_db, a) = sales();
    let call = "SUM(VALUE_OF(D AT END_PARTITION, 0) - ROW_NUMBER(BEGIN_PARTITION)) OVER (PARTITION BY R)";
    a.run(&format!("CREATE ASSERTION N CHECK ((SELECT MAX(X) FROM (SELECT {call} FROM S) AS Q (X)) < 100)")).unwrap();
    a.run(&format!("CREATE TABLE C (X INTEGER CHECK (X < (SELECT MAX(Y) FROM (SELECT {call} FROM S) AS Q (Y))))")).unwrap();
    a.rollback().unwrap();
}
