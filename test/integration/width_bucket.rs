//! WIDTH_BUCKET [ISO/IEC 9075-2:2023 §6.31, Feature T612]: the bucket of
//! each operand, of bounds rising and falling, of exact, approximate and
//! decimal floating-point numbers; nulls; the failures 2201G and 22003;
//! the result of the count's type; `?` operands; each Syntax Rule; and
//! WIDTH_BUCKET still a column's name.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::value::{Decimal, Value};

use crate::common::{code, fetched, rows, sorted};

/// Schema A: N, of numbers each of an exact, approximate and decimal
/// floating-point column, a null among them.
fn numbers() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE N (I INTEGER, X NUMERIC(6, 2), F DOUBLE PRECISION, D DECFLOAT)").unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO N VALUES (1, -0.01, -1E-2, -0.01), (2, 0, 0E0, 0), (3, 2.49, 2.49E0, 2.49), (4, 2.5, 2.5E0, 2.5), \
         (5, 9.99, 9.99E0, 9.99), (6, 10, 1E1, 10), (7, NULL, NULL, NULL)",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The SQLSTATE `sql` fails with.
fn state_of(a: &Session, sql: &str) -> Option<SqlState> {
    fetched(a, sql).expect_err(sql).state
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR17a, GR17d, GR17e]
/// four buckets from 0 to 10: 0 below 0, bucket 1 from 0, bucket 2 from
/// 2.5, 5 at 10 and past; of bounds 10 and 0 mirrored, so 0 above 10, 5 at
/// 0 and below, 2.49 and 2.5 in bucket 4; alike of exact, approximate and
/// decimal floating-point operands; null of a null operand.
#[test]
fn buckets_of_each_operand_6_31() {
    let (_db, a) = numbers();
    let rising = "SELECT I, WIDTH_BUCKET(X, 0, 10, 4), WIDTH_BUCKET(F, 0, 10, 4), WIDTH_BUCKET(D, 0, 10, 4) FROM N ORDER BY I";
    let want = ["1,0,0,0", "2,1,1,1", "3,1,1,1", "4,2,2,2", "5,4,4,4", "6,5,5,5", "7,NULL,NULL,NULL"];
    assert_eq!(rows(&a, rising).unwrap(), want);
    let falling = "SELECT I, WIDTH_BUCKET(X, 10, 0, 4), WIDTH_BUCKET(F, 1E1, 0, 4), WIDTH_BUCKET(D, 10, 0.0, 4) FROM N ORDER BY I";
    let want = ["1,5,5,5", "2,5,5,5", "3,4,4,4", "4,4,4,4", "5,1,1,1", "6,1,1,1", "7,NULL,NULL,NULL"];
    assert_eq!(rows(&a, falling).unwrap(), want);
    assert_eq!(
        sorted(
            &a,
            "SELECT WIDTH_BUCKET(5, CAST(NULL AS INTEGER), 10, 4), WIDTH_BUCKET(5, 0, 10, CAST(NULL AS INTEGER)) FROM N WHERE I = 1"
        ),
        ["NULL,NULL"]
    );
    assert_eq!(sorted(&a, "SELECT WIDTH_BUCKET(0.1, 0, 0.3, 3), WIDTH_BUCKET(0.2, 0, 0.3, 3) FROM N WHERE I = 1"), ["2,3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR17b, GR17c, GR17d
/// ii] a count of 0 or below raises 2201G, as do equal bounds, even of
/// operands of other types; WBC+1 past the count's type raises 22003,
/// SMALLINT of 5 digits, though the operand in range does not.
#[test]
fn failures_are_2201g_and_22003_6_31() {
    let (_db, a) = numbers();
    let invalid = Some(SqlState::INVALID_ARGUMENT_FOR_WIDTH_BUCKET);
    assert_eq!(state_of(&a, "SELECT WIDTH_BUCKET(X, 0, 10, 0) FROM N"), invalid);
    assert_eq!(state_of(&a, "SELECT WIDTH_BUCKET(X, 0, 10, I - 3) FROM N"), invalid);
    assert_eq!(state_of(&a, "SELECT WIDTH_BUCKET(X, 1, 1E0, 4) FROM N"), invalid);
    let most = "CAST(99999 AS SMALLINT)";
    assert_eq!(state_of(&a, &format!("SELECT WIDTH_BUCKET(X, 0, 5, {most}) FROM N")), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(sorted(&a, &format!("SELECT WIDTH_BUCKET(X, 0, 10, {most}) FROM N WHERE I = 5")), ["99900"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR21] the result is
/// of the count's type, SMALLINT or NUMERIC(9, 0); a count of a scale
/// other than 0, approximate or a string, or an operand not a number, is
/// SQLCODE -103.
#[test]
fn types_are_syntax_rule_21_6_31() {
    let (_db, a) = numbers();
    let view =
        "CREATE VIEW B (S, M) AS SELECT WIDTH_BUCKET(F, 0, 10, CAST(4 AS SMALLINT)), WIDTH_BUCKET(X, 0, 10, CAST(4 AS NUMERIC(9))) FROM N";
    a.run(view).unwrap();
    let typed =
        "SELECT COLUMN_NAME, DATA_TYPE, NUMERIC_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'B' ORDER BY COLUMN_NAME";
    assert_eq!(rows(&a, typed).unwrap(), ["M,NUMERIC,9", "S,SMALLINT,5"]);
    for sql in [
        "SELECT WIDTH_BUCKET(X, 0, 10, 4.0) FROM N",
        "SELECT WIDTH_BUCKET(X, 0, 10, 4E0) FROM N",
        "SELECT WIDTH_BUCKET(X, 0, 10, '4') FROM N",
        "SELECT WIDTH_BUCKET('1', 0, 10, 4) FROM N",
        "SELECT WIDTH_BUCKET(X, DATE '2024-01-01', 10, 4) FROM N",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(code(&a, "SELECT WIDTH_BUCKET(X, 0, 10) FROM N"), Code::Syntax);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31, §20.7 GR5] a `?`
/// count is NUMERIC(18, 0), a `?` operand or bound of the type of the
/// other operand or bound it is compared with; a call of `?` alone but its
/// count is refused, SQLCODE -103 [ISO/IEC 9075:1992 §17.6 GR4].
#[test]
fn parameters_take_the_operands_types_6_31() {
    let (_db, a) = numbers();
    let p = a.prepare("SELECT WIDTH_BUCKET(?, 0, X, ?) FROM N WHERE I = 5").unwrap();
    let mut c = a.open_prepared(&p, &[Value::Exact(Decimal::new(5, 0)), Value::Exact(Decimal::new(3, 0))]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("2".into()));
    drop(c);
    let refused = a.prepare("SELECT WIDTH_BUCKET(?, ?, ?, 4) FROM N").map(|_| ()).map_err(|e| e.code);
    assert_eq!(refused, Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §6.31] WIDTH_BUCKET,
/// no key word of SQL-92, is read so only before "(", so it names a
/// column elsewhere.
#[test]
fn width_bucket_names_a_column_5_2() {
    let (_db, a) = numbers();
    a.run("CREATE TABLE WB (WIDTH_BUCKET INTEGER)").unwrap();
    a.run("INSERT INTO WB VALUES (3)").unwrap();
    assert_eq!(sorted(&a, "SELECT WIDTH_BUCKET, WIDTH_BUCKET(WIDTH_BUCKET, 0, 6, 2) FROM WB"), ["3,2"]);
    a.rollback().unwrap();
}
