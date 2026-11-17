//! The Syntax Rules of the multiset predicates and of a multiset element
//! grouping operation in a constraint [ISO/IEC 9075-2:2023 §8.16 to §8.18,
//! §9.16 SR1]: a dynamic parameter typed by its operand, operands of
//! another type refused, MEMBER and SUBMULTISET of large objects compared
//! as read, and the element types whose values not distinct may differ.

use turborel::db::Database;
use turborel::error::Code;
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{code, failure};
use crate::multisets::{multisets, one};

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.16 SR2, SR3, §8.17 SR1,
/// §8.18 SR1] a dynamic parameter X of MEMBER takes the element type of
/// the multiset; an operand of IS A SET or SUBMULTISET that is a row of
/// several values, or not a multiset, is refused (SQLCODE -103).
#[test]
fn operands_of_the_multiset_predicates_8_16() {
    let db = multisets();
    let a = db.session("A");
    let p = a.prepare("SELECT K FROM T WHERE ? MEMBER OF M ORDER BY K").unwrap();
    let mut c = a.open_prepared(&p, &[Value::Exact(Decimal::new(2, 0))]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("1".to_owned()));
    assert!(c.fetch().unwrap().is_none());
    assert_eq!(code(&a, "SELECT K FROM T WHERE (M, M) IS A SET"), Code::SyntaxRule, "§8.18 SR1");
    assert_eq!(code(&a, "SELECT K FROM T WHERE K SUBMULTISET OF M"), Code::SyntaxRule, "§8.17 SR1, a number");
    assert_eq!(code(&a, "SELECT K FROM T WHERE M SUBMULTISET OF K"), Code::SyntaxRule, "§8.17 SR2, a number");
    assert_eq!(code(&a, "SELECT K FROM T WHERE K IS A SET"), Code::SyntaxRule, "§8.18 SR1, a number");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.16 GR3, §8.2 GR3; RULES.md
/// D79] MEMBER of a multiset of character large objects compares them as
/// read, padded with spaces: true of an equal element, false of none.
#[test]
fn member_of_large_objects_8_16() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (K INTEGER, C CLOB)").unwrap();
    let a = db.session("A");
    let insert = a.prepare("INSERT INTO L VALUES (?, ?)").unwrap();
    for (k, text) in [(1, "ab"), (2, "cd")] {
        a.execute_prepared(&insert, &[Value::Exact(Decimal::new(k, 0)), Value::Lob(Lob::of_text(text))]).unwrap();
    }
    let member = "SELECT CASE WHEN C MEMBER OF (SELECT COLLECT(C) FROM L WHERE K = 1) THEN 'T' ELSE 'F' END FROM L ORDER BY K";
    assert_eq!(crate::common::rows(&a, member).unwrap(), ["T", "F"]);
    let literal = "SELECT CASE WHEN CAST('ab  ' AS CLOB) MEMBER OF MULTISET[CAST('ab' AS CLOB)] THEN 'T' ELSE 'F' END FROM L WHERE K = 1";
    assert_eq!(one(&a, literal), "T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1n, SR1o, SR1aq,
/// §11.9 CR2] of a CHECK, a multiset element grouping operation is a
/// potential source of non-determinism where the elements are based on a
/// character string type: a character large object, an array or a row of
/// character strings, a multiset of them, or a datetime with time zone;
/// of a binary large object's array, of numbers' rows and of a datetime
/// without time zone it is not. MC/DC of the zone: TIMESTAMP and
/// TIMESTAMP WITH TIME ZONE differ in it alone.
#[test]
fn grouping_of_uncertain_elements_in_constraints_9_16() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (A VARCHAR(2) ARRAY[2] MULTISET, R ROW (X VARCHAR(2)) MULTISET, \
         N VARCHAR(2) MULTISET MULTISET, Z TIMESTAMP WITH TIME ZONE MULTISET, \
         I INTEGER ARRAY[2] MULTISET, S ROW (X INTEGER) MULTISET, T TIMESTAMP MULTISET)",
    )
    .unwrap();
    let a = db.session("A");
    for column in ["R", "N", "Z"] {
        let sql = format!("ALTER TABLE C ADD CHECK (CARDINALITY(SET({column})) < 9)");
        assert_eq!(failure(a.run(&sql)).0, Code::SyntaxRule, "{column}");
    }
    for column in ["S", "T"] {
        a.run(&format!("ALTER TABLE C ADD CHECK (CARDINALITY(SET({column})) < 9)")).unwrap_or_else(|e| panic!("{column}: {e}"));
    }
    a.run("ALTER TABLE C ADD CHECK (CARDINALITY(A MULTISET UNION ALL A) < 9)").unwrap();
    a.run("ALTER TABLE C ADD CHECK (CARDINALITY(I MULTISET UNION ALL I) < 9)").unwrap();
}
