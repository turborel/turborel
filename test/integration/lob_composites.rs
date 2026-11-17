//! Large objects in rows and arrays compared for equality [ISO/IEC
//! 9075-2:2023 §8.2 GR1b, §8.15 GR1, §9.11] (RULES.md D79): each place of
//! a large object read and compared as the equality operations read it,
//! a null at such a place unknown, and under an explicit collation.

use turborel::db::{Database, Session};
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// Schema A: table T of K and a CLOB C, rows (1, 'abc'), (2, 'abd') and
/// (3, null), the first of many chunks with 'abc' at its head.
fn lobs() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB)").unwrap();
    let a = db.session("A");
    let insert = a.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    for (k, c) in [(1, Some("abc")), (2, Some("abd")), (3, None)] {
        let value = c.map_or(Value::Null, |c| Value::Lob(Lob::of_text(c)));
        a.execute_prepared(&insert, &[Value::Exact(Decimal::new(k, 0)), value]).unwrap();
    }
    a.commit().unwrap();
    (db, a)
}

/// The truth value of `c` of each row of T, in order of K.
fn truths(s: &Session, c: &str) -> Vec<String> {
    rows(s, &format!("SELECT CASE WHEN {c} THEN 'T' WHEN NOT ({c}) THEN 'F' ELSE 'U' END FROM T ORDER BY K")).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR1b i, GR1b ii, §8.15
/// GR1] a row or an array holding a large object is equal to another
/// exactly when their large objects are, read as compared, padded with
/// spaces; a null large object leaves the comparison unknown unless
/// another field decides it, and IS
/// DISTINCT FROM takes it as not distinct from a null.
#[test]
fn rows_and_arrays_of_large_objects_compared_8_2() {
    let (_db, a) = lobs();
    assert_eq!(truths(&a, "ROW (C, K) = ROW (CAST('abc  ' AS CLOB), 1)"), ["T", "F", "F"], "3 is not 1");
    assert_eq!(truths(&a, "ROW (C, 1) = ROW (CAST('abc' AS CLOB), 1)"), ["T", "F", "U"]);
    assert_eq!(truths(&a, "ARRAY[C] = ARRAY[CAST('abd' AS CLOB)]"), ["F", "T", "U"]);
    assert_eq!(truths(&a, "(C, K) <> ('abc', 1)"), ["F", "T", "T"]);
    assert_eq!(truths(&a, "ROW (C) IS DISTINCT FROM ROW (CAST(NULL AS CLOB))"), ["T", "T", "F"]);
    assert_eq!(truths(&a, "CAST(NULL AS CLOB) = C"), ["U", "U", "U"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR3, §9.5] equality of a
/// character large object under an explicit collation compares as it
/// says: case folded under a case-insensitive collation.
#[test]
fn large_objects_compared_under_a_collation_8_2() {
    let (_db, a) = lobs();
    a.run("CREATE COLLATION FOLD FOR SQL_TEXT FROM CASE_INSENSITIVE PAD SPACE").unwrap();
    assert_eq!(truths(&a, "C = 'ABC' COLLATE A.FOLD"), ["T", "F", "U"]);
    assert_eq!(truths(&a, "C = 'ABC'"), ["F", "F", "U"]);
}
