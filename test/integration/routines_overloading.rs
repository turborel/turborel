//! SQL-invoked routines of one name told apart by their parameters
//! (Feature T341) [ISO/IEC 9075-2:2023 §11.60 SR20, §9.18 SR9, SR10, §9.6,
//! §9.7]. It covers which definitions may stand beside each other, which
//! one an invocation or a CALL takes by its arguments' types, and how a
//! designator, a grant and the Information Schema name each.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::value::{DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

use crate::common::money;

/// Schema A holds T (K INTEGER, S VARCHAR(10), C CHAR(3), D DOUBLE
/// PRECISION) of one row and L (WHO VARCHAR(10), K INTEGER), a log. Schema
/// B, owned by B, holds M (K INTEGER) of one row.
fn defined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S VARCHAR(10), C CHAR(3), D DOUBLE PRECISION) \
         CREATE TABLE L (WHO VARCHAR(10), K INTEGER)",
    )
    .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (K INTEGER)").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("INSERT INTO T VALUES (5, 'abc', 'abc', 2.5E0)").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO M VALUES (1)").unwrap();
    b.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap_or_else(|e| panic!("{query}: {e}"));
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The error of `sql` run in `s`, which fails, the transaction then rolled
/// back.
fn refusal(s: &Session, sql: &str) -> SqlError {
    let e = s.run(sql).expect_err(sql);
    s.rollback().unwrap();
    e
}

/// The SQLCODE class of `sql` run in `s`, which fails.
fn code(s: &Session, sql: &str) -> Code {
    refusal(s, sql).code
}

/// The SQLCODE class of opening a cursor on `query` in `s`, which fails.
fn opened(s: &Session, query: &str) -> Code {
    s.open_cursor(query).err().unwrap_or_else(|| panic!("{query} opened")).code
}

/// Defines each statement of `sql` in `s` and commits.
fn define(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// An INTEGER parameter's type.
fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The types of the dynamic parameters of `sql` prepared in `s`.
fn described(s: &Session, sql: &str) -> Vec<DataType> {
    let p = s.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    p.parameters().iter().map(|t| t.clone().data_type()).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10d, SR10h, §9.6, §9.7]
/// of F of INTEGER, of CHARACTER VARYING and of DOUBLE PRECISION, an
/// invocation takes the one its argument's type ranks first. An exact
/// literal or column takes INTEGER's, a character literal VARCHAR's (its
/// type's precedence list holds VARCHAR), and an approximate literal or
/// column DOUBLE's.
#[test]
fn an_argument_type_chooses_the_function_9_18() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION F (X CHARACTER VARYING(10)) RETURNS INTEGER CONTAINS SQL RETURN 2",
            "CREATE FUNCTION F (X DOUBLE PRECISION) RETURNS INTEGER CONTAINS SQL RETURN 3",
        ],
    );
    assert_eq!(rows(&a, "SELECT F(1), F('a'), F(1E0) FROM T"), ["1,2,3"], "by the literals");
    assert_eq!(rows(&a, "SELECT F(K), F(S), F(D) FROM T"), ["1,2,3"], "by the columns");
    assert_eq!(rows(&a, "SELECT F(2.5) FROM T"), ["1"], "every exact type is of INTEGER's rank");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b, §9.7] beside F of
/// INTEGER, a function F of BIGINT, SMALLINT or DECIMAL is refused, every
/// exact numeric type of one rank so none told apart (SQLCODE -103); one of
/// DOUBLE PRECISION, or of two parameters, is defined.
#[test]
fn functions_alike_are_refused_11_60() {
    let db = defined();
    let a = db.session("A");
    define(&a, &["CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 1"]);
    for ty in ["BIGINT", "SMALLINT", "DECIMAL(5,2)", "NUMERIC(3)"] {
        let sql = format!("CREATE FUNCTION F (Y {ty}) RETURNS INTEGER CONTAINS SQL RETURN 9");
        assert_eq!(code(&a, &sql), Code::SyntaxRule, "{ty}");
    }
    define(
        &a,
        &[
            "CREATE FUNCTION F (X DOUBLE PRECISION) RETURNS INTEGER CONTAINS SQL RETURN 2",
            "CREATE FUNCTION F (X INTEGER, Y INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + Y",
        ],
    );
    assert_eq!(rows(&a, "SELECT F(1), F(1E0), F(3, 4) FROM T"), ["1,2,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b, §9.18 SR10h, §9.7]
/// F of CHARACTER stands beside F of CHARACTER VARYING, since CHARACTER's
/// precedence list holds both and VARCHAR's holds itself alone. A
/// CHARACTER literal or column takes CHARACTER's, a VARCHAR column
/// VARCHAR's.
#[test]
fn character_stands_beside_varchar_9_7() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X CHARACTER VARYING(3)) RETURNS INTEGER CONTAINS SQL RETURN 2",
            "CREATE FUNCTION F (X CHARACTER(3)) RETURNS INTEGER CONTAINS SQL RETURN 1",
        ],
    );
    assert_eq!(rows(&a, "SELECT F('abc'), F(C), F(S) FROM T"), ["1,1,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.6, §9.18 SR10h] of G of
/// (INTEGER, DOUBLE PRECISION) and of (DOUBLE PRECISION, INTEGER), both
/// taking two exact arguments, the first argument decides, from left to
/// right; with an approximate first argument, the second.
#[test]
fn arguments_decide_from_left_to_right_9_6() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION G (A INTEGER, B DOUBLE PRECISION) RETURNS INTEGER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION G (A DOUBLE PRECISION, B INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 2",
        ],
    );
    assert_eq!(rows(&a, "SELECT G(1, 1), G(1, 1E0), G(1E0, 1) FROM T"), ["1,1,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10a, SR10d, SR10h iv 3,
/// §9.6] a dynamic parameter, of no type, fits F of INTEGER and F of
/// CHARACTER alike, so the invocation is refused; of one function H, it
/// takes the type of H's parameter (Feature B033). No function of the
/// arity is SR10a's refusal; none of it that takes the arguments' types,
/// SR10d's.
#[test]
fn invocations_without_one_subject_are_refused_9_18() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION F (X CHARACTER(1)) RETURNS INTEGER CONTAINS SQL RETURN 2",
            "CREATE FUNCTION H (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 1",
        ],
    );
    assert_eq!(a.prepare("SELECT F(?) FROM T").err().map(|e| e.code), Some(Code::SyntaxRule), "alike");
    assert_eq!(described(&a, "SELECT H(?) FROM T"), [integer()]);
    let p = a.prepare("SELECT H(?) FROM T").unwrap();
    let mut c = a.open_prepared(&p, &[int(41)]).unwrap();
    assert_eq!(c.fetch().unwrap().unwrap().first().map(ToString::to_string).as_deref(), Some("42"));
    let arity = a.open_cursor("SELECT F(1, 2) FROM T").err().expect("no F of two");
    assert_eq!((arity.code, arity.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR10a")));
    let fit = a.open_cursor("SELECT F(DATE '2020-01-01') FROM T").err().expect("no F of a date");
    assert_eq!((fit.code, fit.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR10d")));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20a, §9.18 SR9, §16.1]
/// procedures of a name are told apart by their number of parameters
/// alone. P of one INTEGER and P of two stand together, and P of CHARACTER
/// beside P of INTEGER is refused. CALL runs the one of its arity, and a
/// dynamic parameter of CALL takes the type of that one's parameter.
#[test]
fn procedures_are_told_apart_by_arity_11_60() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE PROCEDURE P (IN K INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES ('ONE', K)",
            "CREATE PROCEDURE P (IN K INTEGER, IN N INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES ('TWO', K + N)",
        ],
    );
    let alike = "CREATE PROCEDURE P (IN X CHARACTER(1)) MODIFIES SQL DATA INSERT INTO L VALUES (X, 0)";
    assert_eq!(code(&a, alike), Code::SyntaxRule);
    a.run("CALL P(1)").unwrap();
    a.run("CALL P(1, 2)").unwrap();
    assert_eq!(described(&a, "CALL P(?)"), [integer()]);
    let p = a.prepare("CALL P(?)").unwrap();
    a.execute_prepared(&p, &[int(7)]).unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT WHO, K FROM L ORDER BY K"), ["ONE,1", "TWO,3", "ONE,7"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b, §11.51, §9.18
/// SR10h] the function a distinct type MONEY of NUMERIC(10, 2) makes is
/// one an SQL function MONEY must be told apart from. MONEY of INTEGER is
/// refused (both exact), MONEY of CHARACTER(5) defined, and each is invoked
/// by its argument's type. A type is likewise refused beside an SQL function
/// DOLLARS of DECIMAL.
#[test]
fn a_distinct_type_function_is_an_overload_11_60() {
    let db = money();
    let a = db.session("A");
    define(&a, &["INSERT INTO T VALUES (1, MONEY(1))"]);
    assert_eq!(code(&a, "CREATE FUNCTION MONEY (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN 7"), Code::SyntaxRule);
    define(&a, &["CREATE FUNCTION MONEY (X CHARACTER(5)) RETURNS INTEGER CONTAINS SQL RETURN 7"]);
    assert_eq!(rows(&a, "SELECT MONEY('12.50'), MONEY(1.5) FROM T"), ["7,1.50"]);
    let e = a.open_cursor("SELECT MONEY(1.5) + 1 FROM T").err().expect("a MONEY value");
    assert_eq!(e.code, Code::SyntaxRule, "MONEY(1.5) is of the distinct type, not a number: {e}");
    define(&a, &["CREATE FUNCTION DOLLARS (X DECIMAL(9,2)) RETURNS INTEGER CONTAINS SQL RETURN 1"]);
    assert_eq!(code(&a, "CREATE TYPE DOLLARS AS NUMERIC(10, 2)"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR6, §12.3, §10.6 SR3c]
/// EXECUTE is held on one function of a name, not on its name. B, granted
/// EXECUTE on A's F of CHARACTER by its specific name, invokes it, and is
/// refused F of INTEGER (SQLCODE -104).
#[test]
fn execute_is_held_on_one_overload_12_3() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER SPECIFIC F_NUMBER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION F (X CHARACTER(1)) RETURNS INTEGER SPECIFIC F_LETTER CONTAINS SQL RETURN 2",
            "GRANT EXECUTE ON SPECIFIC FUNCTION F_LETTER TO B",
        ],
    );
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT A.F('x') FROM M"), ["2"]);
    assert_eq!(opened(&b, "SELECT A.F(1) FROM M"), Code::Privilege);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.6 SR3c, §11.62, §12.3]
/// a routine designator by name alone designates no one of two functions
/// (SQLCODE -103); one with its parameters' types, or a specific name,
/// designates one, dropped or granted on, and the other stays.
#[test]
fn designators_name_one_overload_10_6() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER SPECIFIC F_NUMBER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION F (X CHARACTER(1)) RETURNS INTEGER SPECIFIC F_LETTER CONTAINS SQL RETURN 2",
            "CREATE FUNCTION F (X DOUBLE PRECISION) RETURNS INTEGER SPECIFIC F_FLOAT CONTAINS SQL RETURN 3",
        ],
    );
    assert_eq!(code(&a, "DROP FUNCTION F CASCADE"), Code::SyntaxRule);
    assert_eq!(code(&a, "GRANT EXECUTE ON FUNCTION F TO B"), Code::SyntaxRule);
    define(&a, &["GRANT EXECUTE ON FUNCTION F(CHARACTER(1)) TO B"]);
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT A.F('x') FROM M"), ["2"]);
    assert_eq!(opened(&b, "SELECT A.F(1) FROM M"), Code::Privilege);
    define(&a, &["DROP FUNCTION F(INTEGER) CASCADE"]);
    assert_eq!(rows(&a, "SELECT F('x'), F(1E0), F(1) FROM T"), ["2,3,3"], "an exact argument now takes DOUBLE's");
    define(&a, &["DROP SPECIFIC FUNCTION F_FLOAT RESTRICT"]);
    assert_eq!(rows(&a, "SELECT F('x') FROM T"), ["2"]);
    assert_eq!(opened(&a, "SELECT F(1) FROM T"), Code::SyntaxRule, "no F left takes a number");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.6 SR3c, §12.3] beside an SQL
/// function MONEY of CHARACTER(5), a grant on MONEY with the parameter
/// type NUMERIC(10, 2) designates the distinct type's function. B then
/// invokes it, and not the SQL one.
#[test]
fn a_grant_designates_the_type_function_10_6() {
    let db = money();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (K INTEGER)").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    define(&b, &["INSERT INTO M VALUES (1)"]);
    define(
        &a,
        &[
            "CREATE FUNCTION MONEY (X CHARACTER(5)) RETURNS INTEGER CONTAINS SQL RETURN 7",
            "GRANT USAGE ON TYPE MONEY TO B",
            "GRANT EXECUTE ON FUNCTION MONEY(NUMERIC(10, 2)) TO B",
        ],
    );
    let held = "SELECT SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES WHERE GRANTEE = 'B'";
    assert_eq!(rows(&b, held), ["MONEY_TO_DISTINCT"]);
    assert_eq!(rows(&b, "SELECT A.MONEY(3) FROM M"), ["3.00"]);
    assert_eq!(opened(&b, "SELECT A.MONEY('12.50') FROM M"), Code::Privilege);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b] two functions
/// of a name are two rows of INFORMATION_SCHEMA.ROUTINES, of one routine
/// name and different specific names.
#[test]
fn each_overload_is_a_routine_row_11_60() {
    let db = defined();
    let a = db.session("A");
    define(
        &a,
        &[
            "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER SPECIFIC F_NUMBER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION F (X CHARACTER(1)) RETURNS INTEGER CONTAINS SQL RETURN 2",
        ],
    );
    let listed = rows(&a, "SELECT ROUTINE_NAME, SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY 2");
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(listed.iter().all(|r| r.starts_with("F,")), "{listed:?}");
    assert!(listed.contains(&"F,F_NUMBER".to_owned()), "{listed:?}");
    assert_ne!(listed.first(), listed.get(1));
}
