//! The values of distinct types [ISO/IEC 9075-2:2023 §4.9.5, §4.13, §6.13,
//! §9.2, §9.5; RULES.md D69]: compared within one type alone, ordered and
//! grouped as their source's, assigned and cast to and from their source
//! type, refused by every other operator, brought together only with
//! their own type's, and the elements of arrays.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, money, rows};

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Schema A: MONEY and EUROS, both of NUMERIC(10, 2), TAGS of an array
/// and FLAG of BOOLEAN; table T of a key K, MONEY P with a UNIQUE
/// constraint, EUROS E with a btree index, TAGS G and FLAG F, holding
/// three rows and a fourth of nulls.
fn kinds() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE MONEY AS NUMERIC(10, 2)
           CREATE TYPE EUROS AS NUMERIC(10, 2) CAST (DISTINCT AS SOURCE) WITH AMOUNT
           CREATE TYPE TAGS AS INTEGER ARRAY[3]
           CREATE TYPE FLAG AS BOOLEAN
           CREATE TABLE T (K INTEGER PRIMARY KEY, P MONEY UNIQUE, E EUROS, G TAGS, F FLAG)",
    )
    .unwrap();
    db.create_index("T_E", "A.T", &["E"], "btree").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 5, 6, ARRAY[1], TRUE)").unwrap();
    a.run("INSERT INTO T VALUES (2, 7.5, 6, ARRAY[2, 3], FALSE)").unwrap();
    a.run("INSERT INTO T VALUES (3, 6, 7, ARRAY[], TRUE)").unwrap();
    a.run("INSERT INTO T VALUES (4, NULL, NULL, NULL, NULL)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.9.5, §8.2 SR3, §11.51 GR2c] values of
/// a distinct type compare with values of that type alone, through their
/// source's ordering. MC/DC of comparability, the same type and a source
/// of no array: MONEY with MONEY compares; MONEY with EUROS, of the same
/// source, does not; MONEY with its source type does not; TAGS, of an
/// array, compares with nothing, itself included.
#[test]
fn compared_within_one_type_4_9_5() {
    let (_db, a) = kinds();
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = MONEY(5)"), ["1"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE MONEY(6) < P"), ["2"]);
    for sql in ["SELECT K FROM T WHERE P = E", "SELECT K FROM T WHERE P = 5", "SELECT K FROM T WHERE G = G"] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(query(&a, "SELECT K FROM T WHERE E = EUROS(6) ORDER BY K"), ["1", "2"], "by the index");
    assert_eq!(query(&a, "SELECT K FROM T WHERE P IN (MONEY(5), MONEY(6)) ORDER BY K"), ["1", "3"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P BETWEEN MONEY(5.5) AND MONEY(8) ORDER BY K"), ["2", "3"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE P = ANY (SELECT P FROM T WHERE K > 2)"), ["3"]);
    assert_eq!(query(&a, "SELECT K FROM T WHERE F = FLAG(TRUE) ORDER BY K"), ["1", "3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.9.5, §7.13, §10.9, §11.7] values of a
/// distinct type of a source of no array are ordered, grouped, made
/// distinct and kept unique as their source's: ORDER BY, GROUP BY,
/// DISTINCT, MAX and MIN, COUNT DISTINCT, INTERSECT, and a UNIQUE
/// constraint refusing a second equal value. Those of an array source are
/// neither ordered nor grouped.
#[test]
fn ordered_and_grouped_4_9_5() {
    let (_db, a) = kinds();
    assert_eq!(query(&a, "SELECT P FROM T ORDER BY P"), ["5.00", "6.00", "7.50", "NULL"]);
    assert_eq!(query(&a, "SELECT E, COUNT(*) FROM T GROUP BY E ORDER BY E"), ["6.00,2", "7.00,1", "NULL,1"]);
    assert_eq!(query(&a, "SELECT DISTINCT E FROM T ORDER BY E"), ["6.00", "7.00", "NULL"]);
    assert_eq!(query(&a, "SELECT MAX(P), MIN(P), COUNT(DISTINCT E) FROM T"), ["7.50,5.00,2"]);
    assert_eq!(query(&a, "SELECT P FROM T INTERSECT SELECT MONEY(6) FROM T"), ["6.00"]);
    assert_eq!(a.run("INSERT INTO T VALUES (5, 5, 1, NULL, NULL)").unwrap_err().code, Code::Constraint);
    a.rollback().unwrap();
    for sql in ["SELECT G FROM T ORDER BY G", "SELECT DISTINCT G FROM T", "SELECT G FROM T GROUP BY G"] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(a.run("CREATE TABLE U (G TAGS UNIQUE)").unwrap_err().code, Code::SyntaxRule, "a key of an array type");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 SR3, §4.13; §9.7] a value is
/// assigned to a site of a distinct type if it is of that type, or if its
/// type's precedence list holds the source type's designator and it is
/// assignable to the source type, then checked as the source type is: an
/// integer to MONEY, but no character string, FLOAT, or day-time interval
/// to a type of a year-month interval, none of whose lists holds the
/// designator. Of every source type a distinct type may have, a type whose
/// list holds its designator is assignable to it, so assignability decides
/// nothing alone, and the decision is shown of the list's condition. A
/// value of a distinct type is assigned to a site of exactly its source
/// type alone.
#[test]
fn assigned_to_and_from_its_source_9_2() {
    let db = money();
    let a = db.session("A");
    a.run("CREATE TYPE TERM AS INTERVAL YEAR TO MONTH").unwrap();
    a.run("CREATE TABLE S (K INTEGER, N NUMERIC(10, 2), W NUMERIC(12, 4), M TERM)").unwrap();
    a.run("INSERT INTO T VALUES (1, 5)").unwrap();
    a.run("INSERT INTO T VALUES (2, MONEY(2.5))").unwrap();
    a.run("UPDATE T SET P = 9 WHERE K = 1").unwrap();
    for sql in ["INSERT INTO T VALUES (3, 'x')", "INSERT INTO T VALUES (3, 1E0)", "INSERT INTO S (M) VALUES (INTERVAL '1' DAY)"] {
        assert_eq!(a.run(sql).unwrap_err().code, Code::SyntaxRule, "{sql}");
    }
    a.run("INSERT INTO S (M) VALUES (INTERVAL '1' YEAR)").unwrap();
    assert_eq!(a.run("INSERT INTO T VALUES (3, 123456789)").unwrap_err().code, Code::Data, "NUMERIC(10, 2) checks it");
    a.run("INSERT INTO S (K, N) SELECT K, P FROM T").unwrap();
    for sql in ["INSERT INTO S (W) SELECT P FROM T", "INSERT INTO S (K) SELECT P FROM T"] {
        assert_eq!(a.run(sql).unwrap_err().code, Code::SyntaxRule, "{sql}: not exactly the source type");
    }
    assert_eq!(query(&a, "SELECT K, P FROM T ORDER BY K"), ["1,9.00", "2,2.50"]);
    assert_eq!(query(&a, "SELECT N FROM S WHERE K IS NOT NULL ORDER BY N"), ["2.50", "9.00"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR12, GR] CAST to a distinct type
/// takes what is assigned to it, and CAST from one is to its source type
/// alone, a NUMERIC or DECIMAL of its precision and scale. CAST between
/// two distinct types, or to another type, is refused. A CAST to a type
/// of an array source checks its maximum cardinality (2202F).
#[test]
fn cast_to_and_from_its_source_6_13() {
    let (_db, a) = kinds();
    assert_eq!(query(&a, "SELECT CAST(5.123 AS MONEY) FROM T WHERE K = 1"), ["5.12"]);
    assert_eq!(query(&a, "SELECT CAST(P AS NUMERIC(10, 2)), CAST(P AS DECIMAL(10, 2)) FROM T WHERE K = 1"), ["5.00,5.00"]);
    for sql in [
        "SELECT CAST('5' AS MONEY) FROM T",
        "SELECT CAST(1E0 AS MONEY) FROM T",
        "SELECT CAST(P AS NUMERIC(12, 4)) FROM T",
        "SELECT CAST(P AS INTEGER) FROM T",
        "SELECT CAST(P AS CHARACTER(10)) FROM T",
        "SELECT CAST(E AS MONEY) FROM T",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(query(&a, "SELECT CAST(ARRAY[1, 2] AS TAGS), CAST(G AS INTEGER ARRAY[3]) FROM T WHERE K = 2"), ["ARRAY[1, 2],ARRAY[2, 3]"]);
    assert_eq!(code(&a, "SELECT CAST(ARRAY[1, 2, 3, 4] AS TAGS) FROM T WHERE K = 1"), Code::Data);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.5 SR1, §6.13 SR6, §11.5 SR3, SR4e]
/// ARRAY[] is of a distinct type whose source type is an array type where
/// its context gives that type: a value inserted, a CAST, and a column's
/// default. Regression: it was refused as of no element type, its context's
/// type being no array type. A column of a distinct type of no collection
/// type has no default but NULL [SR3b].
#[test]
fn an_empty_array_of_a_distinct_type_6_5() {
    let (_db, a) = kinds();
    a.run("INSERT INTO T VALUES (5, 1, 1, ARRAY[], NULL)").unwrap();
    a.run("CREATE TABLE U (K INTEGER, G TAGS DEFAULT ARRAY[])").unwrap();
    a.run("INSERT INTO U (K) VALUES (1)").unwrap();
    assert_eq!(query(&a, "SELECT G FROM T WHERE K = 5"), ["ARRAY[]"]);
    assert_eq!(query(&a, "SELECT G, CAST(ARRAY[] AS TAGS) FROM U"), ["ARRAY[],ARRAY[]"]);
    a.run("CREATE TABLE N (M MONEY DEFAULT NULL)").unwrap();
    for sql in ["CREATE TABLE W (M MONEY DEFAULT 5)", "CREATE TABLE W (M MONEY DEFAULT ARRAY[])", "CREATE TABLE W (F FLAG DEFAULT TRUE)"] {
        let e = a.run(sql).unwrap_err();
        assert!(e.code == Code::SyntaxRule && e.to_string().contains("§11.5 SR3b"), "{sql}: {e}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.30 SR3, §6.32, §8.5, §6.36, §10.9;
/// §4.9.5] no operator but comparison takes a value of a distinct type:
/// arithmetic, SUM, a string function, LIKE, a boolean operator, an
/// element reference and CARDINALITY are refused; its value is taken by
/// FNSDT or a CAST to its source type.
#[test]
fn no_operator_of_its_source_6_30() {
    let (_db, a) = kinds();
    for sql in [
        "SELECT P + 1 FROM T",
        "SELECT -P FROM T",
        "SELECT SUM(P) FROM T",
        "SELECT UPPER(P) FROM T",
        "SELECT K FROM T WHERE P LIKE '5'",
        "SELECT K FROM T WHERE F",
        "SELECT K FROM T WHERE NOT F",
        "SELECT K FROM T WHERE G[1] = 1",
        "SELECT CARDINALITY(G) FROM T",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(query(&a, "SELECT \"NUMERIC\"(P) + 1 FROM T WHERE K = 2"), ["8.50"]);
    assert_eq!(query(&a, "SELECT AMOUNT(E) * 2 FROM T WHERE K = 3"), ["14.00"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.5 SR3l; §6.12, §6.9, §7.17] values of
/// a distinct type are brought together only with values of that type:
/// CASE, COALESCE and UNION of MONEY and MONEY are of MONEY, and of MONEY
/// and a number refused, as is a VALUES of more than one row that mixes
/// them.
#[test]
fn brought_together_with_its_own_type_9_5() {
    let (_db, a) = kinds();
    assert_eq!(query(&a, "SELECT K, CASE WHEN K = 1 THEN P ELSE MONEY(0) END FROM T ORDER BY K"), ["1,5.00", "2,0.00", "3,0.00", "4,0.00"]);
    assert_eq!(query(&a, "SELECT COALESCE(P, MONEY(1)) FROM T WHERE K = 4"), ["1.00"]);
    assert_eq!(query(&a, "SELECT P FROM T WHERE K = 1 UNION SELECT MONEY(2) FROM T WHERE K = 1 ORDER BY 1"), ["2.00", "5.00"]);
    for sql in ["SELECT CASE WHEN K = 1 THEN P ELSE 0 END FROM T", "SELECT COALESCE(P, 1) FROM T", "SELECT P FROM T UNION SELECT 1 FROM T"]
    {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(a.run("INSERT INTO T (K, P) VALUES (5, MONEY(1)), (6, 2)").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.10, §6.25, §8.2; S093] an array of a
/// distinct type holds values of it: an element is of the type, compared
/// with its values, and arrays of it compared element by element;
/// ARRAY_AGG makes one.
#[test]
fn arrays_of_a_distinct_type_4_10() {
    let db = money();
    let a = db.session("A");
    a.run("CREATE TABLE L (K INTEGER, M MONEY ARRAY[3])").unwrap();
    a.run("INSERT INTO L VALUES (1, ARRAY[MONEY(1), MONEY(2)])").unwrap();
    a.run("INSERT INTO T VALUES (1, 5), (2, 6)").unwrap();
    assert_eq!(query(&a, "SELECT M, M[2], CARDINALITY(M) FROM L"), ["ARRAY[1.00, 2.00],2.00,2"]);
    assert_eq!(query(&a, "SELECT K FROM L WHERE M[1] = MONEY(1)"), ["1"]);
    assert_eq!(query(&a, "SELECT K FROM L WHERE M = ARRAY[MONEY(1), MONEY(2)]"), ["1"]);
    assert_eq!(code(&a, "SELECT K FROM L WHERE M[1] = 1"), Code::SyntaxRule);
    assert_eq!(query(&a, "SELECT ARRAY_AGG(P ORDER BY P DESC) FROM T"), ["ARRAY[6.00, 5.00]"]);
}
