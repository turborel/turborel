//! What vectors have none of: order, equality, grouping and keys.

use super::{got, table};

/// Verifies: [EXTENSION] vector_standard §3.3 R3.11, R3.12, §17: <, >,
/// <=, >=, BETWEEN, =, <>, IN, ORDER BY, GROUP BY, DISTINCT, a set
/// function's DISTINCT, MAX and UNION of vectors are refused with SQLCODE
/// -103 as the statement is bound; VECTOR_EQUALS compares them.
#[test]
fn vectors_have_no_order_or_equality() {
    let (_db, a) = table("V VECTOR(2), W VECTOR(2)");
    a.run("INSERT INTO T VALUES ('[1, 2]', '[1, 2]')").unwrap();
    for sql in [
        "SELECT 1 FROM T WHERE V < W",
        "SELECT 1 FROM T WHERE V >= W",
        "SELECT 1 FROM T WHERE V BETWEEN W AND W",
        "SELECT 1 FROM T WHERE V = W",
        "SELECT 1 FROM T WHERE V <> W",
        "SELECT 1 FROM T WHERE V IN (W, V)",
        "SELECT V FROM T ORDER BY V",
        "SELECT V FROM T GROUP BY V",
        "SELECT DISTINCT V FROM T",
        "SELECT COUNT(DISTINCT V) FROM T",
        "SELECT MAX(V) FROM T",
        "SELECT V FROM T UNION SELECT W FROM T",
    ] {
        let e = crate::common::failed(&a, sql);
        assert_eq!(e.sqlcode(), -103, "{sql}: {e:?}");
    }
    assert_eq!(got(&a, "SELECT VECTOR_EQUALS(V, W), VECTOR_EQUALS(V, '[1, 3]') FROM T"), ["TRUE,FALSE"]);
    assert_eq!(got(&a, "SELECT COUNT(*) FROM (SELECT V FROM T UNION ALL SELECT W FROM T) AS U"), ["2"]);
}

/// Verifies: [EXTENSION] vector_standard §3.3 R3.11, §17: a vector column
/// is refused as a PRIMARY KEY, UNIQUE or FOREIGN KEY column and as the key
/// of a btree or hash index, each SQLCODE -103.
#[test]
fn vectors_are_no_keys() {
    let (db, a) = table("K INTEGER PRIMARY KEY, V VECTOR(2)");
    for definition in ["V VECTOR(2) PRIMARY KEY", "V VECTOR(2) UNIQUE", "V VECTOR(2), UNIQUE (V)"] {
        let made = a.run(&format!("CREATE TABLE U ({definition})"));
        assert_eq!(made.map_err(|e| e.sqlcode()).err(), Some(-103), "{definition}");
    }
    a.run("CREATE TABLE P (V VECTOR(2))").unwrap();
    let referencing = a.run("CREATE TABLE R (V VECTOR(2) REFERENCES P (V))");
    assert_eq!(referencing.map_err(|e| e.sqlcode()).err(), Some(-103));
    for method in ["btree", "hash"] {
        let made = db.create_index(&format!("I_{method}"), "A.T", &["V"], method);
        assert_eq!(made.map_err(|e| e.sqlcode()).err(), Some(-103), "{method}");
    }
}
