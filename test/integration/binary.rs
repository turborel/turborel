//! Binary strings [ISO/IEC 9075-2:2023 §4.4, §6.1]: BINARY, BINARY VARYING
//! and VARBINARY, their literals, assignment, comparison, keys, and the
//! operations on them; and SQL-92's bit strings, which SQL:2023 has not.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, sorted};

/// Schema A: table T of a BINARY(2) F and a VARBINARY(3) V.
fn binaries() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BINARY(2), V VARBINARY(3))").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &turborel::db::Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR1, SR4, SR5, SR26]
/// BINARY, of a length 1 if none is written, and BINARY VARYING or
/// VARBINARY, the same type, of a maximum length written, as without
/// Feature T081, more than 0; one longer than this implementation's
/// longest binary string is valid but not processed (SQLCODE -900,
/// RULES.md D18). BIT and BIT VARYING, which SQL:2023 has not, are no data
/// types (SQLCODE -104), and BINARY and VARBINARY are no key words, so a
/// column keeps either name.
#[test]
fn the_binary_string_types_6_1() {
    let db = binaries();
    let a = db.session("A");
    a.run("CREATE TABLE U (A BINARY, B BINARY(10), C BINARY VARYING(12), D VARBINARY(5))").unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'U' ORDER BY 1";
    assert_eq!(squeezed(&a, described), ["A,BINARY,1,1", "B,BINARY,10,10", "C,BINARY VARYING,12,12", "D,BINARY VARYING,5,5"]);
    assert_eq!(exec(&a, "CREATE TABLE W (A VARBINARY)").err(), Some(Code::Syntax), "its maximum length written");
    assert_eq!(exec(&a, "CREATE TABLE W (A BINARY VARYING)").err(), Some(Code::Syntax), "its maximum length written");
    assert_eq!(exec(&a, "CREATE TABLE W (A BINARY(0))").err(), Some(Code::SyntaxRule), "more than 0");
    assert_eq!(exec(&a, "CREATE TABLE W (A VARBINARY(40000))").err(), Some(Code::Unsupported), "too long here");
    assert_eq!(exec(&a, "CREATE TABLE W (A BIT(4))").err(), Some(Code::Syntax), "no BIT");
    assert_eq!(exec(&a, "CREATE TABLE W (A BIT VARYING(4))").err(), Some(Code::Syntax), "no BIT VARYING");
    a.run("CREATE TABLE W (BINARY INTEGER, VARBINARY BINARY)").unwrap();
    assert_eq!(rows(&a, "SELECT BINARY, VARBINARY FROM W").unwrap(), Vec::<String>::new(), "names of columns");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.3 SR4-SR7, SR20] a binary
/// string literal, X in either case then quoted pairs of hexits, spaces
/// among them passed over, is BINARY of its octets (IV191); quoted parts
/// separated by a newline are one literal, and without one between them
/// are two tokens. An odd number of hexits, or a character neither a
/// hexit nor a space, is no literal (SQLCODE -101), and B'...', a bit
/// string literal SQL:2023 has not, is no literal either.
#[test]
fn binary_string_literals_5_3() {
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'0A0B', x'')").unwrap();
    assert_eq!(rows(&a, "SELECT X'A5', x'0f', X'', X' 01 02 ', X'A B' FROM T").unwrap(), ["A5,0F,,0102,AB"]);
    assert_eq!(rows(&a, "SELECT X'10'\n  '01', X'FF' -- a comment\n 'aa' FROM T").unwrap(), ["1001,FFAA"], "one literal");
    assert_eq!(rows(&a, "SELECT OCTET_LENGTH(X'0A0B0C'), OCTET_LENGTH(X'') FROM T").unwrap(), ["3,0"]);
    assert_eq!(code(&a, "SELECT X'10' '01' FROM T"), Code::Syntax, "no newline between");
    assert_eq!(code(&a, "SELECT X'ABC' FROM T"), Code::Lexical, "an odd number of hexits");
    assert_eq!(code(&a, "SELECT X'AG' FROM T"), Code::Lexical, "G is no <hexit>");
    assert_eq!(code(&a, "SELECT X'A\tB' FROM T"), Code::Lexical, "a tab is no <space>");
    assert_eq!(code(&a, "SELECT B'101' FROM T"), Code::Syntax, "no bit string literal");
    assert_eq!(rows(&a, "SELECT X FROM (SELECT F FROM T) AS X (X)").unwrap(), ["0A0B"], "X apart from a quote is an identifier");
    let typed = "SELECT DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'L'";
    a.run("CREATE VIEW L (C) AS SELECT X'0A0B0C' FROM T").unwrap();
    assert_eq!(squeezed(&a, typed), ["BINARY,3"], "IV191: BINARY of its octets");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3b viii-xii] a binary
/// string assigned to BINARY(n) is of n octets: extended with X'00'
/// octets if shorter, its first n if longer and those after are X'00',
/// else refused (SQLSTATE 22001); to VARBINARY(n), kept as assigned if no
/// longer, else its first n octets if those after are X'00', else
/// refused; no character string or number is assigned to one [§9.2 SR3].
#[test]
fn binary_strings_are_assigned_9_2() {
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'0A0B', X'01'), (X'0F', X''), (X'ABCD0000', X'01020300')").unwrap();
    assert_eq!(sorted(&a, "SELECT F, V, OCTET_LENGTH(V) FROM T"), ["0A0B,01,1", "0F00,,0", "ABCD,010203,3"]);
    assert_eq!(exec(&a, "INSERT INTO T VALUES (X'0A0B0C', X'01')").err(), Some(Code::Data), "22001: longer");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (X'0A0B', X'01020304')").err(), Some(Code::Data), "22001: longer than V");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (X'0A0B', X'0100000001')").err(), Some(Code::Data), "22001: an octet not X'00'");
    assert_eq!(exec(&a, "INSERT INTO T VALUES ('AB', X'01')").err(), Some(Code::SyntaxRule), "a character string");
    assert_eq!(exec(&a, "INSERT INTO T VALUES (X'0A0B', 1)").err(), Some(Code::SyntaxRule), "a number");
    a.run("UPDATE T SET V = V || X'FF' WHERE F = X'0A0B'").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM T WHERE F = X'0A0B'").unwrap(), ["01FF"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR5; §4.4.1] binary
/// strings compare octet by octet, a string less than every longer one it
/// begins, trailing X'00' octets and all (IA219, IA022), and with binary
/// strings alone (SQLCODE -103 against a character string or a number);
/// they order, group and are distinct so, of fixed and variable length
/// alike.
#[test]
fn binary_strings_compare_8_2() {
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'0000', X'01'), (X'0001', X'0100'), (X'0002', X''), (X'0003', X'0001'), (X'0100', X'0100')").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM T ORDER BY V").unwrap(), ["", "0001", "01", "0100", "0100"]);
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT V) FROM T").unwrap(), ["4"]);
    assert_eq!(sorted(&a, "SELECT V, COUNT(*) FROM T GROUP BY V"), [",1", "0001,1", "01,1", "0100,2"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE V < X'01' AND F <> X'0000'").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT F FROM T WHERE V = X'0100' AND F > X'0001'").unwrap(), ["0100"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE F = V").unwrap(), ["1"], "BINARY with VARBINARY");
    assert_eq!(code(&a, "SELECT F FROM T WHERE F = '0000'"), Code::SyntaxRule, "with a character string");
    assert_eq!(code(&a, "SELECT F FROM T WHERE F = 0"), Code::SyntaxRule, "with a number");
    assert_eq!(rows(&a, "SELECT MAX(V), MIN(F) FROM T").unwrap(), ["0100,0000"]);
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.32 SR6a, GR3; §6.33 SR21,
/// GR17; §6.31 GR3, GR6] || joins binary strings, BINARY if both are, else
/// BINARY VARYING; SUBSTRING of a binary string is BINARY VARYING, of its
/// octets from S, L of them (SQLSTATE 22011 if L is negative); POSITION
/// finds one in another, 1 for the empty one and 0 for none; OCTET_LENGTH
/// counts its octets and BIT_LENGTH, an extension SQL:2023 has not, eight
/// bits to each; CHAR_LENGTH and UPPER take character strings alone; ||
/// and POSITION join no binary string with a character string, and +
/// takes none.
#[test]
fn operations_on_binary_strings_6_32() {
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'1100', X'0A0B0C')").unwrap();
    assert_eq!(
        rows(&a, "SELECT F || V, V || X'FF', SUBSTRING(V FROM 2 FOR 1), SUBSTRING(F FROM 2) FROM T").unwrap(),
        ["11000A0B0C,0A0B0CFF,0B,00"]
    );
    assert_eq!(
        rows(&a, "SELECT SUBSTRING(V FROM 9), SUBSTRING(V FROM -1 FOR 3), SUBSTRING(V FROM CAST(NULL AS INTEGER)) FROM T").unwrap(),
        [",0A,NULL"]
    );
    assert_eq!(code(&a, "SELECT SUBSTRING(V FROM 1 FOR -1) FROM T"), Code::Data, "22011");
    assert_eq!(
        rows(&a, "SELECT POSITION(X'0B0C' IN V), POSITION(X'' IN V), POSITION(X'0C0A' IN V), POSITION(X'00' IN F) FROM T").unwrap(),
        ["2,1,0,2"]
    );
    assert_eq!(rows(&a, "SELECT OCTET_LENGTH(V || F), BIT_LENGTH(V || F), BIT_LENGTH(X'') FROM T").unwrap(), ["5,40,0"]);
    assert_eq!(code(&a, "SELECT CHAR_LENGTH(V) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT UPPER(V) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT V || 'a' FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT POSITION('a' IN V) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT V + 1 FROM T"), Code::SyntaxRule);
    a.run("CREATE VIEW W (FF, FV, S) AS SELECT F || F, F || V, SUBSTRING(F FROM 1) FROM T").unwrap();
    a.run("CREATE VIEW Y (U) AS SELECT F FROM T UNION SELECT V FROM T").unwrap();
    let described = "SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME IN ('W', 'Y') ORDER BY 1, 2";
    assert_eq!(squeezed(&a, described), ["W,FF,BINARY,4", "W,FV,BINARY VARYING,5", "W,S,BINARY VARYING,2", "Y,U,BINARY VARYING,3"]);
    assert_eq!(code(&a, "SELECT F FROM T UNION SELECT 'a' FROM T"), Code::SyntaxRule, "§9.3 SR3b");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.32 GR3b ii] || of binary
/// strings longer than the longest varying one keeps its first octets if
/// those after are X'00', else is refused (SQLSTATE 22001).
#[test]
fn a_concatenation_too_long_is_cut_or_refused_6_32() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V VARBINARY(32767))").unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (X'{}')", "AB".repeat(32_767))).unwrap();
    assert_eq!(rows(&a, "SELECT OCTET_LENGTH(V || X'0000') FROM T").unwrap(), ["32767"], "X'00' octets cut");
    assert_eq!(code(&a, "SELECT V || X'01' FROM T"), Code::Data, "22001");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR7, GR13] CAST of a
/// binary string to a binary type: of a fixed length, its first octets or
/// extended with X'00' octets; of a variable length, its first octets at
/// most. No character string, number or Boolean is cast to one, nor one
/// to them (SQLCODE -103).
#[test]
fn binary_strings_are_cast_6_13() {
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'4142', X'010203')").unwrap();
    assert_eq!(
        rows(&a, "SELECT CAST(F AS BINARY(3)), CAST(V AS BINARY(1)), CAST(V AS VARBINARY(2)), CAST(F AS VARBINARY(9)) FROM T").unwrap(),
        ["414200,01,0102,4142"]
    );
    assert_eq!(rows(&a, "SELECT CAST(NULL AS BINARY(2)), CAST(X'' AS BINARY(2)) FROM T").unwrap(), ["NULL,0000"]);
    assert_eq!(code(&a, "SELECT CAST(X'4142' AS VARCHAR(5)) FROM T"), Code::SyntaxRule, "to no character string");
    assert_eq!(code(&a, "SELECT CAST('A' AS BINARY(4)) FROM T"), Code::SyntaxRule, "from no character string");
    assert_eq!(code(&a, "SELECT CAST(1 AS BINARY(4)) FROM T"), Code::SyntaxRule, "from no number");
    assert_eq!(code(&a, "SELECT CAST(F AS INTEGER) FROM T"), Code::SyntaxRule, "to no number");
    assert_eq!(code(&a, "SELECT CAST(TRUE AS BINARY(1)) FROM T"), Code::SyntaxRule, "from no Boolean");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 SR4a ii, GR2a iv,
/// GR2a v] a binary string column's default is a binary string literal no
/// longer than it, of a fixed length extended with X'00' octets, the
/// statement completing with no warning; a domain's alike.
#[test]
fn binary_string_defaults_11_5() {
    use turborel::db::Completion;
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BINARY(2) DEFAULT X'01', V VARBINARY(2) DEFAULT X'0A', N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (N) VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT F, V FROM T").unwrap(), ["0100,0A"]);
    let shown = "SELECT COLUMN_DEFAULT FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' AND COLUMN_NAME = 'F'";
    assert_eq!(rows(&a, shown).unwrap(), ["X'0100'"], "the descriptor holds it padded");
    assert_eq!(exec(&a, "CREATE DOMAIN D BINARY(3) DEFAULT X'FF'"), Ok(Completion::Success), "no warning");
    assert_eq!(exec(&a, "ALTER TABLE T ADD COLUMN G BINARY(3) DEFAULT X'01'"), Ok(Completion::Success), "no warning");
    assert_eq!(exec(&a, "CREATE TABLE U (E D, H INTEGER)"), Ok(Completion::Success));
    a.run("INSERT INTO U (H) VALUES (1)").unwrap();
    assert_eq!(rows(&a, "SELECT E FROM U").unwrap(), ["FF0000"]);
    assert_eq!(exec(&a, "CREATE TABLE W (F BINARY(1) DEFAULT X'0102')").err(), Some(Code::SyntaxRule), "longer than it");
    assert_eq!(exec(&a, "CREATE TABLE W (F BINARY(2) DEFAULT '1')").err(), Some(Code::SyntaxRule), "a character string");
    assert_eq!(exec(&a, "CREATE TABLE W (C CHAR(2) DEFAULT X'01')").err(), Some(Code::SyntaxRule), "of a character string");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR5, §11.7] UNIQUE holds
/// equal binary strings the same, a string and a longer one it begins
/// apart, X'00' octets after it too; an index finds binary strings by
/// their key, in order; a key of two columns holds them apart, as no
/// key of one begins another.
#[test]
fn binary_string_keys_and_indexes_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (V VARBINARY(20) UNIQUE, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'01', 1), (X'0100', 2), (X'', 3), (X'FFFF', 4), (X'0001', 5), (X'00', 6)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (X'0100', 7)").err(), Some(Code::Constraint), "X'0100' twice");
    a.commit().unwrap();
    a.run("CREATE INDEX BY_V ON T (V)").unwrap();
    assert_eq!(rows(&a, "SELECT N FROM T WHERE V = X'0100'").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT N, V FROM T WHERE V >= X'01' ORDER BY V").unwrap(), ["1,01", "2,0100", "4,FFFF"]);
    assert_eq!(rows(&a, "SELECT N, V FROM T WHERE V < X'01' ORDER BY V").unwrap(), ["3,", "6,00", "5,0001"]);
    a.run("CREATE TABLE U (V VARBINARY(4), W VARBINARY(4), UNIQUE (V, W))").unwrap();
    a.run("INSERT INTO U VALUES (X'0100', X''), (X'01', X'00')").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM U").unwrap(), ["2"], "no key begins another");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D25: binary strings of fixed and
/// variable length, and an index on them, are kept: after a crash and
/// after a clean close.
#[test]
fn binary_strings_are_kept() {
    let scratch = Scratch::new("binary");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (F BINARY(3), V VARBINARY(10) UNIQUE)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'ABCDEF', X'0100'), (X'000000', X'')").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT F, V FROM T"), ["000000,", "ABCDEF,0100"]);
        assert_eq!(exec(&a, "INSERT INTO T VALUES (X'111111', X'0100')").err(), Some(Code::Constraint), "its index kept");
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.1 GR5a, GR7h-GR7l] a
/// binary string retrieved into a parameter: as it is if it fits; its
/// first L octets if longer, its indicator set to its length, with the
/// warning 01004; a fixed length's extended with X'00' octets if shorter,
/// with no warning.
#[test]
fn binary_strings_are_retrieved_9_1() {
    use turborel::db::Completion;
    use turborel::error::SqlState;
    use turborel::params::Params;
    use turborel::value::{DataType, Value};
    let db = binaries();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (X'0A0B', X'010203')").unwrap();
    let cases = [
        ("F", DataType::Binary { length: 2 }, "0A0B", 0, Completion::Success),
        ("F", DataType::Binary { length: 1 }, "0A", 2, Completion::Warning(SqlState::STRING_TRUNCATED)),
        ("F", DataType::Binary { length: 4 }, "0A0B0000", 0, Completion::Success),
        ("V", DataType::VarBinary { length: 8 }, "010203", 0, Completion::Success),
        ("V", DataType::VarBinary { length: 2 }, "0102", 3, Completion::Warning(SqlState::STRING_TRUNCATED)),
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
