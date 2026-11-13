//! Arrays [ISO/IEC 9075-2:2023 §4.12.2, §6.1, §6.25, §6.31, §6.47, §6.48,
//! §6.49, §10.9, §14.15]: made, read, compared, cast, aggregated and
//! assigned element by element. Their storage, defaults, domains and
//! description are in `array_columns.rs`.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{code, failure, fetched, rows, sorted};

/// Schema A: table T of an INTEGER K and an INTEGER ARRAY[3] A, holding
/// (1, [1, 2, 3]), (2, [4]), (3, null) and (4, [null, 5]).
fn arrays() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, A INTEGER ARRAY[3])").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, ARRAY[1, 2, 3]), (2, ARRAY[4]), (3, NULL), (4, ARRAY[CAST(NULL AS INTEGER), 5])").unwrap();
    a.commit().unwrap();
    db
}

/// The one value of the one row of `sql`.
fn one(s: &Session, sql: &str) -> String {
    let found = rows(s, sql).unwrap();
    assert_eq!(found.len(), 1, "{sql}");
    found.into_iter().next().unwrap()
}

/// The SQLCODE class and SQLSTATE of the query `sql`, which must fail.
fn refused(s: &Session, sql: &str) -> (Code, String) {
    failure(fetched(s, sql))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §6.49 SR2, GR2, §6.25 GR2] an
/// array column keeps the arrays inserted, each read whole and element by
/// element, a null array and null elements kept.
#[test]
fn arrays_are_kept_6_49() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(sorted(&a, "SELECT K, A FROM T"), ["1,ARRAY[1, 2, 3]", "2,ARRAY[4]", "3,NULL", "4,ARRAY[NULL, 5]"]);
    assert_eq!(sorted(&a, "SELECT K, A[1], CARDINALITY(A) FROM T"), ["1,1,3", "2,4,1", "3,NULL,NULL", "4,NULL,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.49 SR2, SR3, §9.5, §6.5 SR1] an
/// `<array value constructor by enumeration>` is of the type its elements
/// bring together, its maximum cardinality their number, and may nest.
/// ARRAY[] has a type only where its context gives one, elements of no
/// common type are refused, and a bare NULL is no element, as an element
/// is a value expression.
#[test]
fn constructed_by_enumeration_6_49() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT ARRAY[1, 2.5] FROM T WHERE K = 1"), "ARRAY[1.0, 2.5]", "§9.5");
    assert_eq!(one(&a, "SELECT ARRAY_MAX_CARDINALITY(ARRAY[7, 8]) FROM T WHERE K = 1"), "2", "SR3");
    assert_eq!(one(&a, "SELECT ARRAY[ARRAY[1], ARRAY[2, 3]] FROM T WHERE K = 1"), "ARRAY[ARRAY[1], ARRAY[2, 3]]");
    assert_eq!(code(&a, "SELECT ARRAY[] FROM T"), Code::SyntaxRule, "§6.5 SR1");
    assert_eq!(code(&a, "SELECT ARRAY[1, 'a'] FROM T"), Code::SyntaxRule, "§9.5");
    assert_eq!(code(&a, "SELECT ARRAY[1, NULL] FROM T"), Code::SyntaxRule, "an element is a value expression");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.49 SR3a, GR3] an `<array value
/// constructor by query>` is the array of the rows of its query, in the
/// order its ORDER BY gives, and empty of none; its query is of degree
/// one.
#[test]
fn constructed_by_query_6_49() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT ARRAY(SELECT K FROM T ORDER BY K DESC) FROM T WHERE K = 1"), "ARRAY[4, 3, 2, 1]");
    assert_eq!(one(&a, "SELECT ARRAY(SELECT K FROM T WHERE K > 9) FROM T WHERE K = 1"), "ARRAY[]");
    assert_eq!(code(&a, "SELECT ARRAY(SELECT K, A FROM T) FROM T"), Code::SyntaxRule, "SR3a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.25 SR2, GR1, GR2] an element
/// reference is the element at its index, from 1; null if the array or the
/// index is; 2202E outside 1 to the cardinality. MC/DC of "i > 0 and i is
/// at most the cardinality": index 1 against 0 (the first condition), and
/// 3 against 4 of an array of 3 (the second). An index of a scale is
/// refused. An array in parentheses is referenced too, as an element of
/// VALUES.
#[test]
fn element_reference_6_25() {
    let db = arrays();
    let a = db.session("A");
    let error = (Code::Data, "2202E".to_owned());
    assert_eq!(one(&a, "SELECT A[1] FROM T WHERE K = 1"), "1", "i = 1: in range");
    assert_eq!(refused(&a, "SELECT A[0] FROM T WHERE K = 1"), error, "i = 0: the first condition false");
    assert_eq!(one(&a, "SELECT A[3] FROM T WHERE K = 1"), "3", "i = 3: in range");
    assert_eq!(refused(&a, "SELECT A[4] FROM T WHERE K = 1"), error, "i = 4: the second condition false");
    assert_eq!(refused(&a, "SELECT A[-1] FROM T WHERE K = 1"), error);
    assert_eq!(one(&a, "SELECT A[1] FROM T WHERE K = 3"), "NULL", "GR1: a null array");
    assert_eq!(one(&a, "SELECT A[CAST(NULL AS INTEGER)] FROM T WHERE K = 1"), "NULL", "GR1: a null index");
    assert_eq!(code(&a, "SELECT A[1.5] FROM T"), Code::SyntaxRule, "SR2");
    assert_eq!(one(&a, "VALUES (ARRAY[7, 8])[2]"), "8", "§7.1 SR2: one element, not a list");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR1, GR7] CARDINALITY is the
/// number of elements, 0 of an empty array, null of a null one [GR1b];
/// ARRAY_MAX_CARDINALITY is the maximum cardinality of the array's
/// declared type, of a null array too [GR1a]. Neither takes a value of no
/// array type.
#[test]
fn cardinalities_6_31() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(sorted(&a, "SELECT K, CARDINALITY(A), ARRAY_MAX_CARDINALITY(A) FROM T"), ["1,3,3", "2,1,3", "3,NULL,3", "4,2,3"]);
    assert_eq!(one(&a, "SELECT CARDINALITY(CAST(ARRAY[] AS INTEGER ARRAY[2])) FROM T WHERE K = 1"), "0");
    assert_eq!(code(&a, "SELECT CARDINALITY(K) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT ARRAY_MAX_CARDINALITY(K) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.48 GR2] TRIM_ARRAY drops the last
/// elements: null if the count is [GR2b] or the array is [GR2e]; 2202E if
/// the count is negative [GR2c] or more than the cardinality [GR2g]; an
/// empty array of all [GR2i i].
#[test]
fn trimmed_6_48() {
    let db = arrays();
    let a = db.session("A");
    let error = (Code::Data, "2202E".to_owned());
    assert_eq!(one(&a, "SELECT TRIM_ARRAY(A, 1) FROM T WHERE K = 1"), "ARRAY[1, 2]");
    assert_eq!(one(&a, "SELECT TRIM_ARRAY(A, 3) FROM T WHERE K = 1"), "ARRAY[]", "GR2i i");
    assert_eq!(one(&a, "SELECT TRIM_ARRAY(A, CAST(NULL AS INTEGER)) FROM T WHERE K = 1"), "NULL", "GR2b");
    assert_eq!(one(&a, "SELECT TRIM_ARRAY(A, 1) FROM T WHERE K = 3"), "NULL", "GR2e");
    assert_eq!(refused(&a, "SELECT TRIM_ARRAY(A, -1) FROM T WHERE K = 1"), error, "GR2c");
    assert_eq!(refused(&a, "SELECT TRIM_ARRAY(A, 4) FROM T WHERE K = 1"), error, "GR2g");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.47 SR2, SR3, GR2] `||` of two arrays
/// is the elements of the first then the second, of the maximum
/// cardinality both sum to; null if either is; of no other operand.
#[test]
fn concatenated_6_47() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT A || ARRAY[9], ARRAY_MAX_CARDINALITY(A || ARRAY[9]) FROM T WHERE K = 1"), "ARRAY[1, 2, 3, 9],4");
    assert_eq!(one(&a, "SELECT A || ARRAY[9] FROM T WHERE K = 3"), "NULL", "GR2a");
    assert_eq!(one(&a, "SELECT ARRAY[9] || A FROM T WHERE K = 3"), "NULL", "GR2a");
    assert_eq!(code(&a, "SELECT A || 9 FROM T"), Code::SyntaxRule, "SR2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR1b ii] arrays are equal if of
/// one cardinality, each pair of elements equal. MC/DC of the three
/// outcomes: cardinalities differing give False though a null is there
/// ([1, null] = [1]); a pair known unequal gives False though another is
/// unknown, before it or after ([1, null] = [2, null], [null, 1] = [null,
/// 2]); otherwise a null gives Unknown ([1, null] = [1, 2]); otherwise
/// True ([1, 2] = [1, 2]).
#[test]
fn compared_for_equality_8_2() {
    let db = arrays();
    let a = db.session("A");
    let truth = |l: &str, r: &str| {
        one(&a, &format!("SELECT CASE WHEN {l} = {r} THEN 'T' WHEN NOT ({l} = {r}) THEN 'F' ELSE 'U' END FROM T WHERE K = 1"))
    };
    let null = "CAST(NULL AS INTEGER)";
    assert_eq!(truth(&format!("ARRAY[1, {null}]"), "ARRAY[1]"), "F", "cardinalities differ");
    assert_eq!(truth(&format!("ARRAY[1, {null}]"), &format!("ARRAY[2, {null}]")), "F", "a pair unequal");
    assert_eq!(truth(&format!("ARRAY[{null}, 1]"), &format!("ARRAY[{null}, 2]")), "F", "a pair unequal after one unknown");
    assert_eq!(truth(&format!("ARRAY[1, {null}]"), "ARRAY[1, 2]"), "U", "a pair unknown");
    assert_eq!(truth("ARRAY[1, 2]", "ARRAY[1, 2]"), "T", "every pair equal");
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE A = ARRAY[1, 2, 3]"), ["1"]);
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE A <> ARRAY[4]"), ["1", "4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.11, §9.14 SR3] arrays are compared
/// for equality alone: an ordering comparison, BETWEEN, MAX and MIN,
/// GREATEST and an ORDER BY of an array are refused.
#[test]
fn not_ordered_9_14() {
    let db = arrays();
    let a = db.session("A");
    for sql in [
        "SELECT K FROM T WHERE A < ARRAY[4]",
        "SELECT K FROM T WHERE A BETWEEN ARRAY[1] AND ARRAY[5]",
        "SELECT MAX(A) FROM T",
        "SELECT MIN(A) FROM T",
        "SELECT GREATEST(A, ARRAY[1]) FROM T",
        "SELECT K, A FROM T ORDER BY A",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.12 SR3, §7.10 SR10e] arrays are
/// grouped by no operation: GROUP BY, DISTINCT, a set operation of
/// DISTINCT and a DISTINCT set function of an array column are refused,
/// as is a row holding an array. UNION ALL groups nothing, and a NATURAL
/// join of array columns compares them in an equality operation [§9.11],
/// as Feature F402 allows, so both are taken.
#[test]
fn not_grouped_9_12() {
    let db = arrays();
    let a = db.session("A");
    for sql in [
        "SELECT A FROM T GROUP BY A",
        "SELECT DISTINCT A FROM T",
        "SELECT A FROM T UNION SELECT A FROM T",
        "SELECT A FROM T EXCEPT SELECT A FROM T",
        "SELECT A FROM T INTERSECT SELECT A FROM T",
        "SELECT COUNT(DISTINCT A) FROM T",
        "SELECT DISTINCT ROW(K, A) FROM T",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(fetched(&a, "SELECT A FROM T UNION ALL SELECT A FROM T").unwrap(), 8);
    assert_eq!(sorted(&a, "SELECT K FROM T NATURAL JOIN T AS S"), ["1", "2"], "a null array or element joins none");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR5, GR7] an array is cast
/// element by element to an array type, 2202F if it has more elements
/// than that type's maximum cardinality; to no other type, and nothing
/// else to an array type.
#[test]
fn cast_6_13() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT CAST(A AS DECIMAL(5,1) ARRAY[2]) FROM T WHERE K = 2"), "ARRAY[4.0]");
    assert_eq!(one(&a, "SELECT CAST(A AS CHARACTER(2) ARRAY[3]) FROM T WHERE K = 4"), "ARRAY[NULL, '5 ']");
    assert_eq!(refused(&a, "SELECT CAST(A AS DECIMAL(5,1) ARRAY[2]) FROM T WHERE K = 1"), (Code::Data, "2202F".to_owned()));
    assert_eq!(code(&a, "SELECT CAST(A AS VARCHAR(20)) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CAST(K AS INTEGER ARRAY[1]) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3, §4.12.4] an array is stored
/// element by element in an array column's element type, 2202F if it
/// has more elements than the column's maximum cardinality; one of
/// elements not assignable to it is refused.
#[test]
fn assigned_9_2() {
    let db = arrays();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (5, ARRAY[1.0, 2.0])").unwrap();
    assert_eq!(one(&a, "SELECT A FROM T WHERE K = 5"), "ARRAY[1, 2]");
    assert_eq!(failure(a.run("INSERT INTO T VALUES (6, ARRAY[1, 2, 3, 4])")), (Code::Data, "2202F".to_owned()));
    assert_eq!(failure(a.run("INSERT INTO T VALUES (6, ARRAY['a'])")).0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.15 SR11a, GR10b ii 1, §15.5 GR3b
/// i] SET A[i] = v sets the element at i, the array extended by nulls to
/// reach it; 2202E at 0 or past the maximum cardinality, 2200E of a null
/// array; DEFAULT is no element's; and only an array's element is set.
#[test]
fn element_assigned_14_15() {
    let db = arrays();
    let a = db.session("A");
    a.run("UPDATE T SET A[1] = 7 WHERE K = 2").unwrap();
    a.run("UPDATE T SET A[3] = 9 WHERE K = 4").unwrap();
    assert_eq!(sorted(&a, "SELECT K, A FROM T WHERE K IN (2, 4)"), ["2,ARRAY[7]", "4,ARRAY[NULL, 5, 9]"]);
    a.run("UPDATE T SET A[3] = 6 WHERE K = 2").unwrap();
    assert_eq!(one(&a, "SELECT A FROM T WHERE K = 2"), "ARRAY[7, NULL, 6]", "extended by nulls");
    assert_eq!(failure(a.run("UPDATE T SET A[4] = 7 WHERE K = 2")), (Code::Data, "2202E".to_owned()), "V 3");
    assert_eq!(failure(a.run("UPDATE T SET A[0] = 7 WHERE K = 2")), (Code::Data, "2202E".to_owned()), "V 3");
    assert_eq!(failure(a.run("UPDATE T SET A[1] = 7 WHERE K = 3")), (Code::Data, "2200E".to_owned()), "A");
    assert!(a.run("UPDATE T SET A[1] = DEFAULT").is_err(), "§6.5 SR3");
    assert_eq!(failure(a.run("UPDATE T SET K[1] = 7")).0, Code::SyntaxRule, "SR11a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR, GR] ARRAY_AGG is the array of
/// its argument's values, null ones among them, in the order of its ORDER
/// BY, of each group; null of no rows; and it orders by no array.
#[test]
fn aggregated_10_9() {
    let db = arrays();
    let a = db.session("A");
    assert_eq!(one(&a, "SELECT ARRAY_AGG(K ORDER BY K DESC) FROM T"), "ARRAY[4, 3, 2, 1]");
    assert_eq!(one(&a, "SELECT ARRAY_AGG(A[1] ORDER BY K) FROM T"), "ARRAY[1, 4, NULL, NULL]");
    assert_eq!(one(&a, "SELECT ARRAY_AGG(K) FROM T WHERE K > 9"), "NULL");
    let grouped = "SELECT C, ARRAY_AGG(K ORDER BY K) FROM (SELECT K, CARDINALITY(A) AS C FROM T) AS S WHERE C IS NOT NULL GROUP BY C";
    assert_eq!(sorted(&a, grouped), ["1,ARRAY[2]", "2,ARRAY[4]", "3,ARRAY[1]"]);
    assert_eq!(code(&a, "SELECT ARRAY_AGG(K ORDER BY A) FROM T"), Code::SyntaxRule, "§9.14 SR3");
}
