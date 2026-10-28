//! Character strings of variable length [ISO/IEC 9075:1992 §6.1]:
//! CHARACTER VARYING, CHAR VARYING and VARCHAR, kept as assigned, compared
//! padded with spaces, and the types the string operations give.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, exec, os, rows, sorted};

/// Schema A: table T of a VARCHAR(5) V and a CHARACTER(5) C.
fn varying() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V VARCHAR(5), C CHARACTER(5))").unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1] CHARACTER VARYING, CHAR
/// VARYING and VARCHAR name the type, of a length written, more than 0;
/// one longer than this implementation's longest string is valid but not
/// processed (SQLCODE -900, RULES.md D18).
#[test]
fn the_names_of_varying_strings_6_1() {
    let db = varying();
    let a = db.session("A");
    a.run("CREATE TABLE U (A CHARACTER VARYING(3), B CHAR VARYING(4), D VARCHAR(6))").unwrap();
    let described =
        "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U' ORDER BY 1";
    let squeezed: Vec<String> = rows(&a, described).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(squeezed, ["A,CHARACTER VARYING,3", "B,CHARACTER VARYING,4", "D,CHARACTER VARYING,6"]);
    assert_eq!(exec(&a, "CREATE TABLE W (A VARCHAR)").err(), Some(Code::Syntax), "its length written");
    assert_eq!(exec(&a, "CREATE TABLE W (A VARCHAR(0))").err(), Some(Code::SyntaxRule), "more than 0");
    assert_eq!(exec(&a, "CREATE TABLE W (A VARCHAR(40000))").err(), Some(Code::Unsupported), "too long here");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.2 GR3d, GR3e; §8.2 GR3a] a
/// value is kept as assigned, unpadded, its trailing spaces too; one longer
/// than the maximum loses only spaces beyond it, else is refused (SQLSTATE
/// 22001); values compare padded with spaces, with each other and with
/// fixed-length strings.
#[test]
fn varying_values_are_kept_as_assigned_9_2() {
    let db = varying();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('ab', 'ab'), ('ab  ', 'cd'), ('abcde   ', 'ef')").unwrap();
    assert_eq!(sorted(&a, "SELECT CHAR_LENGTH(V) FROM T"), ["2", "4", "5"], "unpadded; trailing spaces kept; spaces beyond cut");
    assert_eq!(exec(&a, "INSERT INTO T VALUES ('abcdef', 'x')").err(), Some(Code::Data), "22001");
    assert_eq!(sorted(&a, "SELECT C FROM T WHERE V = 'ab'"), ["ab   ", "cd   "], "'ab' and 'ab  ' are equal");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE V = C").unwrap(), ["1"], "with CHARACTER, padded");
    a.run("INSERT INTO T VALUES ('a\t', 't'), ('a', 'a'), ('a!', '!')").unwrap();
    let ordered = rows(&a, "SELECT V, C FROM T WHERE V LIKE 'a_' OR V = 'a' ORDER BY V").unwrap();
    // 'ab  ' is not LIKE 'a_': LIKE pads nothing.
    assert_eq!(ordered, ["a\t,t    ", "a,a    ", "a!,!    ", "ab,ab   "], "a tab first, as 'a' padded holds a space there");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR3a, §11.7] UNIQUE holds
/// strings equal padded with spaces to be the same; an index finds values
/// by their key and gives them as stored, trailing spaces and all.
#[test]
fn varying_keys_and_indexes_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V VARCHAR(20) UNIQUE, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('a', 1), ('a long value here', 2), ('b  ', 3)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES ('a  ', 4)").err(), Some(Code::Constraint), "'a  ' is 'a'");
    a.commit().unwrap();
    a.run("CREATE INDEX BY_V ON T (V)").unwrap();
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(V) FROM T WHERE V = 'b'").unwrap(), ["3"], "as stored");
    assert_eq!(rows(&a, "SELECT N, V FROM T WHERE V > 'a' ORDER BY V").unwrap(), ["2,a long value here", "3,b  "]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR6; §6.7 SR2a, SR3a,
/// SR6e; §6.13 SR3a; §9.3 SR3a] CAST to a varying string gives a number's
/// literal if it fits (SQLSTATE 22001 else), a string cut to the length;
/// SUBSTRING and TRIM give varying strings, UPPER and LOWER their
/// operand's type, || a varying string if either operand is, and a union
/// or CASE of one and a fixed one a varying string as long as the longest.
#[test]
fn types_of_varying_string_operations_6_10() {
    let db = varying();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('xy', 'pq')").unwrap();
    assert_eq!(rows(&a, "SELECT CAST(12.5 AS VARCHAR(4)) FROM T").unwrap(), ["12.5"]);
    assert_eq!(crate::common::code(&a, "SELECT CAST(12.5 AS VARCHAR(3)) FROM T"), Code::Data);
    assert_eq!(rows(&a, "SELECT CAST('abcdef' AS VARCHAR(3)) FROM T").unwrap(), ["abc"], "cut");
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(SUBSTRING(C FROM 1 FOR 1)) FROM T").unwrap(), ["1"], "unpadded");
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(TRIM(C)) FROM T").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(UPPER(V)), CHAR_LENGTH(UPPER(C)) FROM T").unwrap(), ["2,5"]);
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(V || V), CHAR_LENGTH(C || C) FROM T").unwrap(), ["4,10"]);
    a.run("CREATE VIEW W (S) AS SELECT V FROM T UNION SELECT C FROM T").unwrap();
    let described = "SELECT DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'W'";
    let squeezed: Vec<String> = rows(&a, described).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(squeezed, ["CHARACTER VARYING,5"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D25: varying strings, their trailing
/// spaces, and an index on them are kept: after a crash and after a clean
/// close.
#[test]
fn varying_strings_are_kept() {
    let scratch = Scratch::new("varying");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V VARCHAR(10) UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('ab  '), ('c')").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT CHAR_LENGTH(V) FROM T"), ["1", "4"]);
        assert_eq!(exec(&a, "INSERT INTO T VALUES ('ab')").err(), Some(Code::Constraint), "its index kept");
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.1 GR3d, GR3e] a varying
/// string retrieved into a parameter of a varying type is set as it is,
/// unpadded; one longer than its maximum to its first characters, its
/// indicator set to the value's length.
#[test]
fn varying_strings_are_retrieved_9_1() {
    use turborel::params::Params;
    use turborel::value::Spelled;
    use turborel::value::{DataType, Value};
    let db = varying();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES ('ab', 'x'), ('abcde', 'y')").unwrap();
    let declared = || {
        Params::new().declare("OUT", DataType::Varying { length: 3 }, Value::Null).declare(
            "IND",
            DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
            Value::Null,
        )
    };
    let mut p = declared();
    a.execute("SELECT V INTO OUT IND FROM T WHERE C = 'x'", &mut p).unwrap();
    assert_eq!((p.value("OUT").to_string(), p.value("IND").to_string()), ("ab".to_owned(), "0".to_owned()), "unpadded");
    let mut p = declared();
    a.execute("SELECT V INTO OUT IND FROM T WHERE C = 'y'", &mut p).unwrap();
    assert_eq!((p.value("OUT").to_string(), p.value("IND").to_string()), ("abc".to_owned(), "5".to_owned()), "cut; its length told");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1 SR1, SR2; §5.3] NATIONAL
/// CHARACTER, NATIONAL CHAR and NCHAR, fixed or VARYING, are the character
/// string types of the national character set, SQL_TEXT, the one there is;
/// a national literal, N then a quoted string, is a character string of
/// it, compared with any; N apart from the quote is an identifier.
#[test]
fn national_character_strings_6_1() {
    let db = varying();
    let a = db.session("A");
    a.run("CREATE TABLE U (A NATIONAL CHARACTER(3), B NATIONAL CHAR VARYING(4), C NCHAR(2), D NCHAR VARYING(5), N INTEGER)").unwrap();
    let described =
        "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U' AND COLUMN_NAME <> 'N' ORDER BY 1";
    let squeezed: Vec<String> = rows(&a, described).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(squeezed, ["A,CHARACTER", "B,CHARACTER VARYING", "C,CHARACTER", "D,CHARACTER VARYING"]);
    a.run("INSERT INTO U VALUES (N'ab', n'it''s', 'x', N'', 1)").unwrap();
    assert_eq!(rows(&a, "SELECT A, B, C, CHAR_LENGTH(D) FROM U WHERE A = 'ab' AND B = N'it''s'").unwrap(), ["ab ,it's,x ,0"]);
    assert_eq!(rows(&a, "SELECT N 'x' FROM U").err(), Some(Code::Syntax), "N apart: the column N, then a string");
    assert_eq!(exec(&a, "CREATE TABLE W (A NATIONAL VARCHAR(3))").err(), Some(Code::Syntax), "NATIONAL CHARACTER or CHAR");
    a.commit().unwrap();
}
