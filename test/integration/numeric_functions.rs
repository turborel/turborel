//! The numeric value functions of Feature T621 [ISO/IEC 9075-2:2023
//! §6.31]: LN, EXP, POWER, SQRT, FLOOR and CEIL or CEILING, of exact,
//! approximate and DECFLOAT operands; nulls; the exceptions 2201E, 2201F
//! and 22003; each Syntax Rule; and the names still columns' where no
//! "(" follows.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows};

/// Schema A: T, of one row: an exact, an approximate and a DECFLOAT
/// column, and a null one.
fn one() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (N NUMERIC(5, 2), F DOUBLE PRECISION, D DECFLOAT, Z INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (-2.5, 2.5E0, 2, NULL)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The one row of `items` selected from T.
fn of(a: &Session, items: &str) -> String {
    let got = rows(a, &format!("SELECT {items} FROM T")).unwrap();
    assert_eq!(got.len(), 1, "{items}");
    got.into_iter().next().unwrap()
}

/// The SQLSTATE of `items` selected from T.
fn state_of(a: &Session, items: &str) -> Option<SqlState> {
    fetched(a, &format!("SELECT {items} FROM T")).expect_err(items).state
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR12c, GR13b, GR14c, GR14d,
/// GR14f, GR14g, GR14h, SR19] LN of 1 is 0 and of e 1; EXP of 0 is 1;
/// POWER of 2 and 10 is 1024, of −2 and 3 −8, of −2 and 2 4, of 0 and 0
/// 1, of 0 and 3 0, of 4 and −0.5 0.5; SQRT of 16 is 4 and of 0 0: each
/// DOUBLE PRECISION.
#[test]
fn logarithms_powers_and_roots_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "LN(1), LN(EXP(1)), EXP(0), EXP(LN(F))"), "0E0,1E0,1E0,2.5E0");
    assert_eq!(
        of(&a, "POWER(2, 10), POWER(-2, 3), POWER(-2E0, 2), POWER(0, 0), POWER(0, 3), POWER(4, -0.5)"),
        "1.024E3,-8E0,4E0,1E0,0E0,5E-1"
    );
    assert_eq!(of(&a, "SQRT(16), SQRT(0), SQRT(F * 10)"), "4E0,0E0,5E0");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR15, GR16, SR20] FLOOR and
/// CEILING, or CEIL, of an exact number are exact of scale 0: −3 and −2
/// of −2.5, 7 of 7; of an approximate one approximate, 2 and 3 of 2.5;
/// of a DECFLOAT, DECFLOAT.
#[test]
fn floor_and_ceiling_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "FLOOR(N), CEILING(N), CEIL(N), FLOOR(7), CEIL(999.99)"), "-3,-2,-2,7,1000");
    assert_eq!(of(&a, "FLOOR(F), CEILING(F), FLOOR(-F)"), "2E0,3E0,-3E0");
    assert_eq!(
        of(
            &a,
            "FLOOR(CAST(-2.5 AS DECFLOAT)), CEIL(CAST(0.001 AS DECFLOAT)), FLOOR(CAST(-0.001 AS DECFLOAT)), CEIL(CAST(1000000 AS DECFLOAT))"
        ),
        "-3,1,-1,1000000"
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR16a, SR17a, SR18a, SR19] of a
/// DECFLOAT the result is DECFLOAT: SQRT of 2 to 34 digits, LN of 1E400
/// = 400 ln 10 and EXP of 0 1, POWER of 10 and 400 1E400, past the range
/// of a double, and of −2 and 3 −8.
#[test]
fn decfloat_operands_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "SQRT(D)"), "1.414213562373095048801688724209698");
    assert_eq!(
        of(&a, "LN(CAST('1E400' AS DECFLOAT)), EXP(D - 2), POWER(CAST(10 AS DECFLOAT), 400), POWER(-D, 3)"),
        "921.0340371976183,1,1E400,-8"
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR12a, GR13a, GR14a, GR15a,
/// GR16a] each is null of a null operand.
#[test]
fn nulls_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "LN(Z), EXP(Z), POWER(Z, 2), POWER(2, Z), SQRT(Z), FLOOR(Z), CEILING(Z)"), "NULL,NULL,NULL,NULL,NULL,NULL,NULL");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR12b, GR13b, GR14b, GR14e]
/// LN of 0 or below raises 2201E; POWER of 0 and a negative exponent,
/// or of a negative base and an exponent not whole, 2201F, as SQRT of a
/// negative number; EXP or POWER past the range of a double 22003.
#[test]
fn exceptions_6_31() {
    let (_db, a) = one();
    let ln = Some(SqlState::INVALID_ARGUMENT_FOR_NATURAL_LOGARITHM);
    let power = Some(SqlState::INVALID_ARGUMENT_FOR_POWER_FUNCTION);
    let range = Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    for (items, want) in [
        ("LN(0)", ln),
        ("LN(N)", ln),
        ("LN(D - 2)", ln),
        ("POWER(0, -1)", power),
        ("POWER(-8, 0.5)", power),
        ("SQRT(N)", power),
        ("SQRT(-D)", power),
        ("EXP(1000)", range),
        ("POWER(10, 400)", range),
        ("EXP(CAST(100000 AS DECFLOAT))", range),
    ] {
        assert_eq!(state_of(&a, items), want, "{items}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31] each operand is a `<numeric
/// value expression>`, else SQLCODE -103; LN, POWER and FLOOR are still
/// names of columns where no "(" follows.
#[test]
fn syntax_rules_6_31() {
    let (_db, a) = one();
    for sql in ["SELECT LN('A') FROM T", "SELECT POWER(2, 'A') FROM T", "SELECT FLOOR(DATE '2026-01-01') FROM T"] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(
        rows(&a, "SELECT LN, POWER + 1, FLOOR FROM (SELECT N AS LN, Z AS POWER, F AS FLOOR FROM T) AS S").unwrap(),
        ["-2.50,NULL,2.5E0"]
    );
}
