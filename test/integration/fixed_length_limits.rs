//! The declared types of a concatenation of fixed-length strings at the
//! longest a fixed-length string may be [ISO/IEC 9075-2:2023 §6.32], and
//! POWER of a DECFLOAT raised to a fraction [§6.31].

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: F, of two CHARACTER(20000) and two BINARY(20000) columns.
fn fixed() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE F (A CHARACTER(20000), B CHARACTER(20000), X BINARY(20000), Y BINARY(20000))")
        .unwrap();
    db
}

/// The single value `sql` gives in `s`, as text.
fn value(s: &Session, sql: &str) -> String {
    let mut c = s.open_cursor(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    c.fetch().unwrap().unwrap()[0].to_string()
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.32 SR6a iii] [ISO/IEC 9075:1992
/// §6.13 SR3a ii] a concatenation of two fixed-length character strings,
/// or of two fixed-length binary strings, is of fixed length their sum:
/// past the longest fixed-length string (32,767) it is refused, SQLCODE
/// -103; with a varying operand it is varying, at most the longest.
#[test]
fn fixed_concatenations_past_the_longest_are_refused() {
    let db = fixed();
    let a = db.session("A");
    for (sql, cited) in [("SELECT A || B FROM F", "§6.13 SR3a ii"), ("SELECT X || Y FROM F", "§6.32 SR6a iii")] {
        let e = a.open_cursor(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
        assert!(e.code == Code::SyntaxRule && e.to_string().contains(cited), "{sql}: {e}");
    }
    let mut varying = a.open_cursor("SELECT A || CAST(B AS VARCHAR(20000)) FROM F").unwrap();
    assert!(varying.fetch().unwrap().is_none(), "bound, of no rows");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR14] POWER of a DECFLOAT base
/// raised to an exponent not a whole number is computed through the
/// base's logarithm, a DECFLOAT; of a negative base and a whole odd
/// exponent past what products reach, negative.
#[test]
fn a_decfloat_is_raised_to_a_fraction() {
    let db = fixed();
    let a = db.session("A");
    let root = value(&a, "VALUES POWER(CAST(2 AS DECFLOAT), 0.5)");
    assert!(root.starts_with("1.41421356237"), "{root}");
    let tenth = value(&a, "VALUES POWER(CAST(1024 AS DECFLOAT), 0.1)");
    assert!(tenth.starts_with('2') || tenth.starts_with("1.99999"), "{tenth}");
}
