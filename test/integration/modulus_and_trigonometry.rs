//! The numeric value functions of Features T441, T622, T623 and T624
//! [ISO/IEC 9075-2:2023 §6.31]: ABS and MOD, the trigonometric functions,
//! LOG and LOG10, of exact, approximate and DECFLOAT operands; their result
//! types; nulls; the exceptions 22003 and 22012; each Syntax Rule; and the
//! names still columns' where no "(" follows.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows};

/// Schema A with T, of one row, a SMALLINT S of its least value, an INTEGER
/// I, a NUMERIC(5, 2) N, a DOUBLE PRECISION F, a DECFLOAT D and a null
/// INTEGER Z.
fn one() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T \
         (S SMALLINT, I INTEGER, N NUMERIC(5, 2), F DOUBLE PRECISION, D DECFLOAT, Z INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (-32768, -7, -2.50, -2.5E0, -7, NULL)").unwrap();
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

/// The declared type of each of `items`, as a view of them describes it:
/// its DATA_TYPE, then its precision and scale where it has them.
fn types_of(a: &Session, items: &str) -> Vec<String> {
    a.run(&format!("CREATE VIEW V AS SELECT {items} FROM T")).unwrap();
    let described = "SELECT DATA_TYPE, NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'V' ORDER BY ORDINAL_POSITION";
    let got = rows(a, described).unwrap();
    a.run("DROP VIEW V CASCADE").unwrap();
    got
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR11, GR8] ABS of a number is of
/// its declared type: N of N not below 0, −N of N below it, of exact,
/// approximate and DECFLOAT numbers; null of null. −N is a value of the
/// type, of a SMALLINT of −32768 or −99999 too, as an exact type is of a
/// decimal precision, so GR8c's 22003 is never raised.
#[test]
fn absolute_value_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "ABS(I), ABS(-I), ABS(N), ABS(F), ABS(D), ABS(0), ABS(Z)"), "7,7,2.50,2.5E0,7,0,NULL");
    assert_eq!(of(&a, "ABS(S), ABS(CAST(-99999 AS SMALLINT))"), "32768,99999");
    assert_eq!(
        types_of(&a, "ABS(S) AS A, ABS(N) AS B, ABS(F) AS C, ABS(D) AS E"),
        ["SMALLINT,5,0", "NUMERIC,5,2", "DOUBLE PRECISION,53,NULL", "DECFLOAT,34,NULL"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR12, GR9] MOD of exact numbers
/// of scale 0 is the remainder of the dividend's sign, less than the
/// divisor in magnitude, of the divisor's declared type; of a DECFLOAT
/// operand DECFLOAT(34) (IV140), found exactly, of a dividend past a
/// double's range too; null of a null operand, a divisor of 0 then too;
/// else a divisor of 0 raises 22012.
#[test]
fn modulus_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "MOD(7, 2), MOD(I, 2), MOD(7, -2), MOD(I, -2), MOD(6, 3), MOD(1, 7)"), "1,-1,1,-1,0,1");
    assert_eq!(of(&a, "MOD(CAST(S AS INTEGER), -1), MOD(Z, 2), MOD(2, Z)"), "0,NULL,NULL");
    assert_eq!(of(&a, "MOD(D, 2), MOD(CAST('1E100' AS DECFLOAT), 7), MOD(CAST(7.5 AS DECFLOAT), 2)"), "-1,4,1.5");
    assert_eq!(
        of(&a, "MOD(CAST(5 AS DECFLOAT), CAST('1E40' AS DECFLOAT)), MOD(CAST('-12E3' AS DECFLOAT), CAST('7E2' AS DECFLOAT))"),
        "5,-100"
    );
    assert_eq!(state_of(&a, "MOD(I, 0)"), Some(SqlState::DIVISION_BY_ZERO));
    assert_eq!(state_of(&a, "MOD(D, CAST(0 AS DECFLOAT))"), Some(SqlState::DIVISION_BY_ZERO));
    assert_eq!(of(&a, "MOD(Z, 0)"), "NULL", "a null operand first [GR9a]");
    assert_eq!(types_of(&a, "MOD(I, S) AS A, MOD(S, I) AS B, MOD(D, I) AS C"), ["SMALLINT,5,0", "INTEGER,10,0", "DECFLOAT,34,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR12, SR11] MOD takes exact
/// operands of scale 0 or DECFLOAT, ABS numbers, else SQLCODE -103; ABS,
/// MOD, SIN and LOG are still names of columns where no "(" follows.
#[test]
fn syntax_rules_6_31() {
    let (_db, a) = one();
    for sql in [
        "SELECT MOD(N, 2) FROM T",
        "SELECT MOD(7, F) FROM T",
        "SELECT ABS('A') FROM T",
        "SELECT SIN(DATE '2026-01-01') FROM T",
        "SELECT LOG(2, 'A') FROM T",
        "SELECT LOG10(TRUE) FROM T",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(
        rows(&a, "SELECT ABS, MOD + 1, SIN, LOG FROM (SELECT I AS ABS, Z AS MOD, F AS SIN, N AS LOG FROM T) AS U").unwrap(),
        ["-7,NULL,-2.5E0,-2.50"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR13, GR10] each trigonometric
/// function of an angle in radians, of an exact or approximate operand
/// DOUBLE PRECISION (IV144), of a DECFLOAT one DECFLOAT(34) (IV143); null
/// of null; ASIN and ACOS of a value below −1 or above 1 raise 22003, as
/// does COSH past the range of a double.
#[test]
fn trigonometric_functions_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "SIN(0), COS(0), TAN(0), SINH(0), COSH(0), TANH(0)"), "0E0,1E0,0E0,0E0,1E0,0E0");
    assert_eq!(of(&a, "ASIN(1) * 2, ACOS(-1), ATAN(1) * 4"), "3.141592653589793E0,3.141592653589793E0,3.141592653589793E0");
    assert_eq!(of(&a, "ASIN(0), ACOS(1), ATAN(Z), SIN(Z)"), "0E0,0E0,NULL,NULL");
    assert_eq!(of(&a, "COS(D - D), ACOS(D + 6)"), "1,3.141592653589793");
    let range = Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    for items in ["ASIN(1.5)", "ACOS(-1.0001)", "ASIN(F)", "ACOS(D)", "COSH(1000)"] {
        assert_eq!(state_of(&a, items), range, "{items}");
    }
    assert_eq!(types_of(&a, "SIN(I) AS A, ATAN(F) AS B, COS(D) AS C"), ["FLOAT,53,NULL", "FLOAT,53,NULL", "DECFLOAT,34,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR14, SR15, GR11] LOG of a base
/// and an argument, LOG10 of an argument as LOG of 10: powers of the base
/// whole numbers; DOUBLE PRECISION, or DECFLOAT(34) of a DECFLOAT operand
/// (IV141, IV142); null of a null operand; an argument of 0 or below, or a
/// base of 0 or below or of 1, raises 22003.
#[test]
fn logarithms_6_31() {
    let (_db, a) = one();
    assert_eq!(of(&a, "LOG(2, 8), LOG(10, 1000), LOG10(1000), LOG(3, 81), LOG10(0.01), LOG(4, 2)"), "3E0,3E0,3E0,4E0,-2E0,5E-1");
    assert_eq!(of(&a, "LOG10(CAST('1E400' AS DECFLOAT)), LOG(2, -D + 1)"), "400,3");
    assert_eq!(of(&a, "LOG(Z, 2), LOG(2, Z), LOG10(Z), LOG(Z, 0)"), "NULL,NULL,NULL,NULL");
    let range = Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    for items in ["LOG(2, 0)", "LOG(2, I)", "LOG(0, 2)", "LOG(-2, 2)", "LOG(1, 2)", "LOG(1.0, 2)", "LOG10(0)", "LOG10(D)"] {
        assert_eq!(state_of(&a, items), range, "{items}");
    }
    assert_eq!(types_of(&a, "LOG(2, I) AS A, LOG10(F) AS B, LOG(D, 2) AS C"), ["FLOAT,53,NULL", "FLOAT,53,NULL", "DECFLOAT,34,NULL"]);
}
