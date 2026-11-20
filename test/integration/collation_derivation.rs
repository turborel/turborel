//! How an operation's collation is derived from its operands'
//! [ISO/IEC 9075-2:2023 §4.3.2, §9.5], as COLLATION FOR shows it and a
//! comparison or LIKE uses it, of large objects and escapes too.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: P, whose NAME is under CASE_INSENSITIVE; W, whose WORD is
/// under UNICODE; and C, a large object under CASE_INSENSITIVE.
fn collated() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE, AGE INTEGER)
           CREATE TABLE W (WORD VARCHAR(20) COLLATE UNICODE)
           CREATE TABLE C (T CLOB(1K) COLLATE CASE_INSENSITIVE)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES ('Bob', 30), ('Ann', 40)").unwrap();
    a.run("INSERT INTO W VALUES ('apple'), ('z%oo'), ('zoo')").unwrap();
    a.run("INSERT INTO C VALUES ('Bravo'), ('alpha')").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `sql`, each its values joined by commas.
fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.5, §6.4 GR13] two operands of
/// different explicit collations are refused (SQLCODE -103); two of
/// different implicit ones give an operation of none, whose COLLATION FOR
/// is null; an explicit one wins over an implicit one; a literal yields
/// to the other operand, and alone is of the character set's.
#[test]
fn collations_combine_9_5() {
    let db = collated();
    let a = db.session("A");
    let e = a.open_cursor("SELECT 1 FROM W WHERE WORD COLLATE UNICODE = WORD COLLATE UCS_BASIC").err().unwrap();
    assert_eq!(e.code, Code::SyntaxRule, "{e:?}");
    assert_eq!(query(&a, "SELECT COLLATION FOR (NAME || WORD) FROM P, W WHERE AGE = 30 AND WORD = 'zoo'"), ["NULL"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR (NAME || WORD COLLATE UCS_BASIC) FROM P, W WHERE AGE = 30 AND WORD = 'zoo'"), ["UCS_BASIC"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR ('x' || WORD) FROM W WHERE WORD = 'zoo'"), ["UNICODE"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR ('x') FROM W WHERE WORD = 'zoo'"), ["SQL_TEXT"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.4 GR13, §9.23 SR5j v] the value of a
/// window function is of its argument's collation, and a subquery's of
/// its column's.
#[test]
fn window_values_keep_their_collation_9_23() {
    let db = collated();
    let a = db.session("A");
    let got = query(&a, "SELECT DISTINCT COLLATION FOR (FIRST_VALUE(WORD) OVER (ORDER BY WORD)) FROM W");
    assert_eq!(got, ["UNICODE"]);
    assert_eq!(query(&a, "SELECT COLLATION FOR ((SELECT MAX(NAME) FROM P)) FROM W WHERE WORD = 'zoo'"), ["CASE_INSENSITIVE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.5 GR3] LIKE under a collation that
/// folds case reads a large object folded too; under UNICODE an escape
/// character makes "%" a character of the pattern, and a null escape makes
/// the predicate unknown.
#[test]
fn like_of_large_objects_and_escapes_8_5() {
    let db = collated();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT CAST(T AS VARCHAR(10)) FROM C WHERE T LIKE 'b%'"), ["Bravo"]);
    assert_eq!(query(&a, "SELECT WORD FROM W WHERE WORD LIKE 'z!%%' ESCAPE '!'"), ["z%oo"]);
    assert_eq!(query(&a, "SELECT WORD FROM W WHERE WORD LIKE 'z%' ESCAPE CAST(NULL AS CHAR(1))").len(), 0);
    assert_eq!(query(&a, "SELECT AGE FROM P WHERE NAME LIKE 'b!%' ESCAPE '!'").len(), 0);
}
