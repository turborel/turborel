//! The `<regex like predicate>` [ISO/IEC 9075-2:2023 §8.7, §9.28;
//! RULES.md D61]: what LIKE_REGEX matches, under which flags, and what it
//! refuses.

use turborel::db::Session;
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{code, exec, expected, rows, setup};

/// The SQLSTATE of the first row `sql` fetches, if it fails.
fn state(s: &Session, sql: &str) -> Result<(), String> {
    s.open_cursor(sql).and_then(|mut c| c.fetch()).map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.7 SR2, SR3, GR1] LIKE_REGEX
/// is true if a substring of the value matches, NOT LIKE_REGEX its
/// negation; the flag the empty string if none is written; unknown if the
/// value, pattern or flag is null; a column's pattern taken (Feature F847).
#[test]
fn like_regex_8_7() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID FROM T WHERE NAME LIKE_REGEX 'N{2}' OR NAME LIKE_REGEX '^b' FLAG 'i' ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1", "2"]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME NOT LIKE_REGEX 'A' ORDER BY ID"), expected(&["2"]));
    let q = "SELECT ID, NAME LIKE_REGEX 'O', NAME LIKE_REGEX 'O' FLAG CAST(NULL AS CHAR(1)) FROM T ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,FALSE,NULL", "2,TRUE,NULL", "3,NULL,NULL"]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE (NAME LIKE_REGEX 'x') IS UNKNOWN"), expected(&["1"]));
    exec(&s, "CREATE TABLE PATTERNS (PAT VARCHAR(10), FL VARCHAR(2))").unwrap();
    exec(&s, "INSERT INTO PATTERNS VALUES ('^A', ''), ('b$', 'i'), ('^.N', 'x')").unwrap();
    let q = "SELECT PAT, ID FROM T, PATTERNS WHERE TRIM(NAME) LIKE_REGEX PAT FLAG FL ORDER BY PAT, ID";
    assert_eq!(rows(&s, q), expected(&["^.N,1", "^A,1", "b$,2"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR9a; F&O 3.1 §5.6]
/// matching as F&O 3.1 defines it, with the standard's newlines: `s`, `m`
/// and `x` and `q`; `\s` and `.` and a back-reference; and no collation,
/// so a CASE_INSENSITIVE column matches case and all.
#[test]
fn matching_9_28() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE TABLE LINES (L VARCHAR(20))").unwrap();
    let p = s.prepare("INSERT INTO LINES VALUES (?)").unwrap();
    for text in ["one\ntwo", "one\r\ntwo", "one\u{2028}two"] {
        s.execute_prepared(&p, &[Value::Char(text.into())]).unwrap();
    }
    let q =
        "SELECT L LIKE_REGEX 'one.two', L LIKE_REGEX 'one.two' FLAG 's', L LIKE_REGEX '^two$' FLAG 'm', L LIKE_REGEX '^two$' FROM LINES";
    assert_eq!(rows(&s, q), expected(&["FALSE,TRUE,TRUE,FALSE", "FALSE,TRUE,TRUE,FALSE", "FALSE,TRUE,TRUE,FALSE"]));
    let q = "VALUES ('a.b' LIKE_REGEX 'a.b' FLAG 'q', 'axb' LIKE_REGEX 'a.b' FLAG 'q', 'ab' LIKE_REGEX 'a b' FLAG 'x', 'abab' LIKE_REGEX '^(ab)\\1$')";
    assert_eq!(rows(&s, q), expected(&["TRUE,FALSE,TRUE,TRUE"]));
    let q =
        "VALUES ('a b' LIKE_REGEX 'a\\sb', 'αβ' LIKE_REGEX '^\\p{IsGreekandCoptic}+$', '١٢' LIKE_REGEX '^\\d+$', 'é' LIKE_REGEX '^\\w$')";
    assert_eq!(rows(&s, q), expected(&["TRUE,TRUE,TRUE,TRUE"]));
    exec(&s, "CREATE TABLE PEOPLE (N VARCHAR(10) COLLATE CASE_INSENSITIVE)").unwrap();
    exec(&s, "INSERT INTO PEOPLE VALUES ('Bob')").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM PEOPLE WHERE N LIKE_REGEX 'bob'"), expected(&["0"]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM PEOPLE WHERE N LIKE_REGEX 'bob' FLAG 'i'"), expected(&["1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR4, GR5, §8.7 GR1a] a
/// pattern that is no XQuery regular expression is SQLSTATE 2201S, a flag
/// none 2201T, the pattern judged first; neither if an operand is null;
/// RULES.md D18 a pattern past a limit SQLCODE -900.
#[test]
fn bad_patterns_and_flags_9_28() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(state(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX '(a'"), Err("2201S".into()));
    assert_eq!(state(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX 'a' FLAG 'g'"), Err("2201T".into()));
    assert_eq!(state(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX '[' FLAG 'g'"), Err("2201S".into()));
    assert_eq!(state(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX '\\p{IsNoSuchBlock}'"), Err("2201S".into()));
    assert_eq!(rows(&s, "VALUES ((CAST(NULL AS VARCHAR(1)) LIKE_REGEX '(' FLAG 'g') IS UNKNOWN)"), expected(&["TRUE"]));
    exec(&s, "CREATE TABLE EMPTY (S VARCHAR(5))").unwrap();
    assert_eq!(rows(&s, "SELECT S FROM EMPTY WHERE S LIKE_REGEX '('"), expected(&[]));
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX 'a{9000}'"), Code::Unsupported);
    assert_eq!(
        code(&s, "SELECT ID FROM T WHERE NAME || 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' LIKE_REGEX '(a|aa)*\\1c'"),
        Code::Unsupported
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.7 SR1; §20.7 GR5a xxxvi]
/// the value, pattern and flag are character strings, the value one, not
/// a row; a `?` pattern or flag is CHARACTER VARYING beside the other not
/// a `?`, the implicit flag counting, and a `?` value or both refused;
/// LIKE_REGEX and FLAG stay identifiers.
#[test]
fn operands_8_7() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID LIKE_REGEX '1'"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX 1"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE NAME LIKE_REGEX 'a' FLAG 1"), Code::SyntaxRule);
    assert_eq!(code(&s, "SELECT ID FROM T WHERE (NAME, NAME) LIKE_REGEX 'a'"), Code::SyntaxRule);
    let p = s.prepare("SELECT ID FROM T WHERE NAME LIKE_REGEX ? ORDER BY ID").unwrap();
    let mut c = s.open_prepared(&p, &[Value::Char("^B".into())]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("2".into()));
    let p = s.prepare("SELECT ID FROM T WHERE NAME LIKE_REGEX 'b' FLAG ?").unwrap();
    let mut c = s.open_prepared(&p, &[Value::Char("i".into())]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("2".into()));
    assert!(s.prepare("SELECT ID FROM T WHERE NAME LIKE_REGEX ? FLAG ?").is_err());
    assert!(s.prepare("SELECT ID FROM T WHERE ? LIKE_REGEX 'a'").is_err());
    exec(&s, "CREATE TABLE WORDS (LIKE_REGEX VARCHAR(5), FLAG VARCHAR(5))").unwrap();
    exec(&s, "INSERT INTO WORDS VALUES ('abc', 'i')").unwrap();
    assert_eq!(rows(&s, "SELECT FLAG FROM WORDS WHERE LIKE_REGEX LIKE_REGEX 'B' FLAG FLAG"), expected(&["i"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.12; §11.9] LIKE_REGEX is
/// a `<when operand>` of a simple CASE, and a CHECK constraint, which
/// refuses a row it is false of.
#[test]
fn where_like_regex_is_written_8_7() {
    let db = setup();
    let s = db.session("APP");
    let q = "SELECT ID, CASE NAME WHEN LIKE_REGEX '^A' THEN 'a' WHEN NOT LIKE_REGEX 'O' THEN 'other' ELSE 'o' END FROM T WHERE ID < 3 ORDER BY ID";
    assert_eq!(rows(&s, q), expected(&["1,a    ", "2,o    "]));
    exec(&s, "CREATE TABLE CODES (C CHAR(5) CHECK (C LIKE_REGEX '^[A-Z]{2}[0-9]{3}$'))").unwrap();
    exec(&s, "INSERT INTO CODES VALUES ('AB123')").unwrap();
    assert_eq!(exec(&s, "INSERT INTO CODES VALUES ('ab123')").err(), Some(Code::Constraint));
}
