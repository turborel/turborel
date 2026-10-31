//! Bit strings [ISO/IEC 9075:1992 §6.1]: BIT and BIT VARYING, their
//! literals, assignment, comparison, keys, and the operations on them.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, sorted};

/// Schema A: table T of a BIT(4) F and a BIT VARYING(6) V.
fn bits() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BIT(4), V BIT VARYING(6))").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &turborel::db::Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.1 SR3, SR4] BIT, of a
/// length 1 if none is written, and BIT VARYING, of a length written, more
/// than 0; one longer than this implementation's longest bit string is
/// valid but not processed (SQLCODE -900, RULES.md D18).
#[test]
fn the_bit_string_types_6_1() {
    let db = bits();
    let a = db.session("A");
    a.run("CREATE TABLE U (A BIT, B BIT(10), C BIT VARYING(12))").unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'U' ORDER BY 1";
    assert_eq!(squeezed(&a, described), ["A,BIT,1,1", "B,BIT,10,2", "C,BIT VARYING,12,2"]);
    assert_eq!(exec(&a, "CREATE TABLE W (A BIT VARYING)").err(), Some(Code::Syntax), "its length written");
    assert_eq!(exec(&a, "CREATE TABLE W (A BIT(0))").err(), Some(Code::SyntaxRule), "more than 0");
    assert_eq!(exec(&a, "CREATE TABLE W (A BIT VARYING(300000))").err(), Some(Code::Unsupported), "too long here");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.3 SR2-SR4, SR10, SR11] a
/// bit string literal, B then quoted 0s and 1s, and a hex string literal, X
/// then quoted hexits of four bits each, in either case, are of fixed
/// length, the bits they hold; quoted parts separated by a newline are one
/// literal; one without a newline between them is two tokens.
#[test]
fn bit_string_literals_5_3() {
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'1010', b'')").unwrap();
    assert_eq!(rows(&a, "SELECT B'101', X'A5', x'0f', b'' FROM T").unwrap(), ["101,10100101,00001111,"]);
    assert_eq!(rows(&a, "SELECT B'10'\n  '01', X'F' -- a comment\n 'a' FROM T").unwrap(), ["1001,11111010"], "one literal");
    assert_eq!(rows(&a, "SELECT BIT_LENGTH(B'1010'), BIT_LENGTH(X'ABC') FROM T").unwrap(), ["4,12"]);
    assert_eq!(code(&a, "SELECT B'10' '01' FROM T"), Code::Syntax, "no newline between");
    assert_eq!(code(&a, "SELECT B'102' FROM T"), Code::Lexical, "2 is no <bit>");
    assert_eq!(code(&a, "SELECT X'AG' FROM T"), Code::Lexical, "G is no <hexit>");
    assert_eq!(rows(&a, "SELECT B FROM (SELECT F FROM T) AS B (B)").unwrap(), ["1010"], "B apart from a quote is an identifier");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.2 GR3f-GR3j] a bit string
/// assigned to BIT(n) is of n bits, else refused, SQLSTATE 22001 if longer
/// and 22026 if shorter; to BIT VARYING(n), of at most n bits, kept as
/// assigned; no other value is assigned to a bit string column.
#[test]
fn bit_strings_are_assigned_9_2() {
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'1010', B'1'), (X'F', B'')").unwrap();
    assert_eq!(sorted(&a, "SELECT F, V, BIT_LENGTH(V) FROM T"), ["1010,1,1", "1111,,0"]);
    assert_eq!(exec(&a, "INSERT INTO T VALUES (B'10101', B'1')").err(), Some(Code::Data), "22001: longer");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (B'101', B'1')").err(), Some(Code::Data), "22026: shorter");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (B'1010', B'1010101')").err(), Some(Code::Data), "22001: longer than V");
    assert!(exec(&a, "INSERT INTO T VALUES ('1010', B'1')").is_err(), "a character string");
    assert!(exec(&a, "INSERT INTO T VALUES (B'1010', 1)").is_err(), "a number");
    a.run("UPDATE T SET V = V || B'01' WHERE F = B'1010'").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM T WHERE F = X'A'").unwrap(), ["101"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR4; §4.3.1] bit strings
/// compare bit by bit, a string less than every longer one it begins, and
/// with bit strings alone (SQLCODE -103 against a character string or a
/// number); they order, group and are distinct so.
#[test]
fn bit_strings_compare_8_2() {
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'0000', B'1'), (B'0001', B'10'), (B'0010', B''), (B'0011', B'01'), (B'0100', B'10')").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM T ORDER BY V").unwrap(), ["", "01", "1", "10", "10"]);
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT V) FROM T").unwrap(), ["4"]);
    assert_eq!(sorted(&a, "SELECT V, COUNT(*) FROM T GROUP BY V"), [",1", "01,1", "1,1", "10,2"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE V < B'1' AND F <> X'0'").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT F FROM T WHERE V = B'10' AND F > B'0001'").unwrap(), ["0100"]);
    assert_eq!(code(&a, "SELECT F FROM T WHERE F = '0000'"), Code::SyntaxRule, "with a character string");
    assert_eq!(code(&a, "SELECT F FROM T WHERE F = 0"), Code::SyntaxRule, "with a number");
    assert_eq!(rows(&a, "SELECT MAX(V), MIN(F) FROM T").unwrap(), ["10,0000"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.13 SR6, GR3; §6.7 SR7, GR6;
/// §6.6 GR4-GR6] || joins bit strings, BIT if both are, else BIT VARYING;
/// SUBSTRING of a bit string is BIT VARYING, of its bits from S, L of them
/// (SQLSTATE 22011 if L is negative); CHAR_LENGTH and OCTET_LENGTH are
/// the octets its bits fill, BIT_LENGTH its bits; POSITION and UPPER take
/// character strings alone; || joins no bit string with a character
/// string, and + takes none.
#[test]
fn operations_on_bit_strings_6_7() {
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'1100', B'10101')").unwrap();
    assert_eq!(
        rows(&a, "SELECT F || V, V || B'1', SUBSTRING(V FROM 2 FOR 3), SUBSTRING(F FROM 3) FROM T").unwrap(),
        ["110010101,101011,010,00"]
    );
    assert_eq!(
        rows(&a, "SELECT SUBSTRING(V FROM 9), SUBSTRING(V FROM -1 FOR 3), SUBSTRING(V FROM CAST(NULL AS INTEGER)) FROM T").unwrap(),
        [",1,NULL"]
    );
    assert_eq!(code(&a, "SELECT SUBSTRING(V FROM 1 FOR -1) FROM T"), Code::Data, "22011");
    assert_eq!(
        rows(&a, "SELECT CHAR_LENGTH(V || F), OCTET_LENGTH(V || F), BIT_LENGTH(V || F), BIT_LENGTH(B'') FROM T").unwrap(),
        ["2,2,9,0"]
    );
    assert_eq!(code(&a, "SELECT POSITION(B'1' IN V) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT UPPER(V) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT V || 'a' FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT V + 1 FROM T"), Code::SyntaxRule);
    a.run("CREATE VIEW W (FF, FV, S) AS SELECT F || F, F || V, SUBSTRING(F FROM 1) FROM T").unwrap();
    a.run("CREATE VIEW X (U) AS SELECT F FROM T UNION SELECT V FROM T").unwrap();
    let described = "SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME IN ('W', 'X') ORDER BY 1, 2";
    assert_eq!(squeezed(&a, described), ["W,FF,BIT,8", "W,FV,BIT VARYING,10", "W,S,BIT VARYING,4", "X,U,BIT VARYING,6"]);
    assert_eq!(code(&a, "SELECT F FROM T UNION SELECT 'a' FROM T"), Code::SyntaxRule, "§9.3 SR3b");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR5d, GR6d, GR7, GR8;
/// SR4] CAST of a bit string to a character string gives the characters
/// its bits are, in UTF-8, of a fixed length extended with characters of 0
/// bits, SQLSTATE 22018 if they are not UTF-8; of a character string to a
/// bit string, its bits, fitted to the length, of a fixed length extended
/// with 0 bits; of a number to a bit string is refused.
#[test]
fn bit_strings_are_cast_6_10() {
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'4', B'010000')").unwrap();
    assert_eq!(
        rows(&a, "SELECT CAST(X'4142' AS VARCHAR(5)), CAST(X'4142' AS VARCHAR(1)), CAST(X'41' AS CHAR(3)) FROM T").unwrap(),
        ["AB,A,A\0\0"]
    );
    assert_eq!(rows(&a, "SELECT CAST(V AS CHAR(1)), CAST(B'0100000' AS VARCHAR(2)) FROM T").unwrap(), ["@,@"], "padded with 0 bits");
    assert_eq!(code(&a, "SELECT CAST(X'FF' AS VARCHAR(2)) FROM T"), Code::Data, "22018: not UTF-8");
    assert_eq!(
        rows(&a, "SELECT CAST('A' AS BIT(4)), CAST('A' AS BIT(10)), CAST('AB' AS BIT VARYING(12)) FROM T").unwrap(),
        ["0100,0100000100,010000010100"]
    );
    assert_eq!(rows(&a, "SELECT CAST(F AS BIT(6)), CAST(V AS BIT VARYING(3)), CAST(NULL AS BIT(2)) FROM T").unwrap(), ["010000,010,NULL"]);
    assert_eq!(code(&a, "SELECT CAST(1 AS BIT(4)) FROM T"), Code::SyntaxRule, "SR4: no number");
    assert_eq!(code(&a, "SELECT CAST(F AS INTEGER) FROM T"), Code::SyntaxRule, "SR4: to no number");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5 SR3a ii, GR1b iv, GR1b
/// v] a bit string column's default is a bit string literal no longer than
/// it, of a fixed length extended with 0 bits.
#[test]
fn bit_string_defaults_11_5() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BIT(4) DEFAULT B'1', V BIT VARYING(4) DEFAULT X'A', N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (N) VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT F, V FROM T").unwrap(), ["1000,1010"]);
    assert_eq!(exec(&a, "CREATE TABLE U (F BIT(2) DEFAULT B'101')").err(), Some(Code::SyntaxRule), "longer than it");
    assert_eq!(exec(&a, "CREATE TABLE U (F BIT(2) DEFAULT '1')").err(), Some(Code::SyntaxRule), "a character string");
    assert_eq!(exec(&a, "CREATE TABLE U (C CHAR(2) DEFAULT B'1')").err(), Some(Code::SyntaxRule), "of a character string");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5 GR1b v] a bit string
/// default shorter than its fixed-length column or domain is placed in the
/// descriptor extended with 0-valued bits, and the statement placing it
/// completes with the warning 01008, implicit zero-bit padding: a column
/// definition of a table definition [§11.4 GR5], of a schema definition, of
/// an add column definition [§11.11 GR4] or of a temporary table
/// declaration [§13.11], a column of a domain among them; a set column
/// default clause [§11.13 GR2]; a domain definition [§11.21 GR3] and a set
/// domain default clause [§11.23 GR2]. A default of the length, or of a
/// varying length, raises none, nor does a statement that takes a default.
#[test]
fn a_default_placed_padded_warns_11_5() {
    use turborel::db::Completion;
    use turborel::error::SqlState;
    let padded = Ok(Completion::Warning(SqlState::ZERO_BIT_PADDING));
    let db = Database::new();
    assert_eq!(
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE S (F BIT(4) DEFAULT B'1')").map_err(|e| e.code),
        padded.clone(),
        "of a schema definition"
    );
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE DOMAIN D BIT(4) DEFAULT X'A'").unwrap();
    let a = db.session("A");
    let made = "CREATE TABLE T (F BIT(4) DEFAULT B'1', V BIT VARYING(4) DEFAULT X'A', W BIT(4) DEFAULT B'1010', N INTEGER)";
    assert_eq!(exec(&a, made), padded);
    assert_eq!(exec(&a, "CREATE TABLE T2 (V BIT VARYING(4) DEFAULT B'1', W BIT(4) DEFAULT X'A')"), Ok(Completion::Success));
    assert_eq!(exec(&a, "INSERT INTO T (N) VALUES (1)"), Ok(Completion::Success), "taking it");
    assert_eq!(exec(&a, "UPDATE T SET F = DEFAULT"), Ok(Completion::Success));
    assert_eq!(rows(&a, "SELECT F, V, W FROM T").unwrap(), ["1000,1010,1010"]);
    let shown = "SELECT COLUMN_DEFAULT FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'F'";
    assert_eq!(rows(&a, shown).unwrap(), ["B'1000'"], "the descriptor holds it padded");
    assert_eq!(exec(&a, "ALTER TABLE T ADD COLUMN G BIT(3) DEFAULT B'1'"), padded, "§11.11 GR4");
    assert_eq!(exec(&a, "ALTER TABLE T ALTER COLUMN W SET DEFAULT B'01'"), padded, "§11.13 GR2");
    assert_eq!(exec(&a, "ALTER TABLE T ALTER COLUMN V SET DEFAULT B'01'"), Ok(Completion::Success), "varying");
    assert_eq!(exec(&a, "CREATE DOMAIN E BIT(4) DEFAULT B'11'"), padded, "§11.21 GR3");
    assert_eq!(exec(&a, "ALTER DOMAIN E SET DEFAULT B'1'"), padded, "§11.23 GR2");
    assert_eq!(exec(&a, "ALTER DOMAIN E SET DEFAULT B'1111'"), Ok(Completion::Success));
    assert_eq!(exec(&a, "CREATE TABLE U (E E DEFAULT B'1', H E)"), padded, "a column of a domain");
    assert_eq!(exec(&a, "CREATE TABLE U2 (H E)"), Ok(Completion::Success), "the domain's own");
    assert_eq!(exec(&a, "DECLARE LOCAL TEMPORARY TABLE MODULE.X (F BIT(4) DEFAULT B'1')"), padded, "§13.11");
    a.commit().unwrap();
    assert_eq!(exec(&a, "INSERT INTO U VALUES (DEFAULT, DEFAULT)"), Ok(Completion::Success));
    assert_eq!(rows(&a, "SELECT E, H FROM U").unwrap(), ["1000,1111"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR4, §11.7] UNIQUE holds
/// equal bit strings the same, a string and a longer one it begins apart;
/// an index finds bit strings by their key, in order.
#[test]
fn bit_string_keys_and_indexes_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V BIT VARYING(20) UNIQUE, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'1', 1), (B'10', 2), (B'', 3), (X'FFF', 4), (B'01', 5)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (B'10', 6)").err(), Some(Code::Constraint), "B'10' twice");
    a.commit().unwrap();
    a.run("CREATE INDEX BY_V ON T (V)").unwrap();
    assert_eq!(rows(&a, "SELECT N FROM T WHERE V = B'10'").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT N, V FROM T WHERE V >= B'1' ORDER BY V").unwrap(), ["1,1", "2,10", "4,111111111111"]);
    assert_eq!(rows(&a, "SELECT N, V FROM T WHERE V < B'1' ORDER BY V").unwrap(), ["3,", "5,01"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D25: bit strings of fixed and variable
/// length, and an index on them, are kept: after a crash and after a clean
/// close.
#[test]
fn bit_strings_are_kept() {
    let scratch = Scratch::new("bits");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BIT(12), V BIT VARYING(10) UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'ABC', B'101'), (X'000', B'')").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT F, V FROM T"), ["000000000000,", "101010111100,101"]);
        assert_eq!(exec(&a, "INSERT INTO T VALUES (X'111', B'101')").err(), Some(Code::Constraint), "its index kept");
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.1 GR2a, GR3f-GR3j] a bit
/// string retrieved into a parameter: as it is if it fits; its first L
/// bits if longer, its indicator set to its length, with the warning
/// 01004; a fixed length's padded with 0 bits if shorter, with the warning
/// 01008.
#[test]
fn bit_strings_are_retrieved_9_1() {
    use turborel::db::Completion;
    use turborel::error::SqlState;
    use turborel::params::Params;
    use turborel::value::{DataType, Value};
    let db = bits();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (B'1010', B'101010')").unwrap();
    let cases = [
        ("F", DataType::Bit { length: 4 }, "1010", 0, Completion::Success),
        ("F", DataType::Bit { length: 2 }, "10", 4, Completion::Warning(SqlState::STRING_TRUNCATED)),
        ("F", DataType::Bit { length: 8 }, "10100000", 0, Completion::Warning(SqlState::ZERO_BIT_PADDING)),
        ("V", DataType::BitVarying { length: 8 }, "101010", 0, Completion::Success),
        ("V", DataType::BitVarying { length: 3 }, "101", 6, Completion::Warning(SqlState::STRING_TRUNCATED)),
    ];
    for (column, target, value, indicator, done) in cases {
        let mut p = Params::new().declare("OUT", target, Value::Null).indicator("IND", -9);
        let completion = a.execute(&format!("SELECT {column} INTO OUT INDICATOR IND FROM T"), &mut p).unwrap();
        assert_eq!(
            (completion, p.value("OUT").to_string(), p.indicator_value("IND")),
            (done, value.to_owned(), Some(indicator)),
            "{column} into {target}"
        );
    }
}
