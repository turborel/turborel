//! Large objects in predicates and aggregates [ISO/IEC 9075-2:2023 §4.1,
//! §9.12, §9.14]: an operation that orders or groups its operands refuses
//! a large object; equality, IN, a quantified comparison, MATCH and a
//! join compare values of many chunks as they are read; LISTAGG joins
//! character large objects; and LIKE under a collation of one character
//! at a time folds a character large object a chunk at a time, under one
//! of contractions holds it whole up to a chunk (RULES.md D79).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{code, rows, state};

/// Schema A: tables T and U, each of a CLOB C and an INTEGER K. T holds
/// 1, a text of 600000 characters; 2, 'x'; 3, the text, its last
/// character changed. U holds 10, the text, and 20, 'x '.
fn tables() -> (Database, Session, String) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (C CLOB, K INTEGER) CREATE TABLE U (C CLOB, K INTEGER)").unwrap();
    let a = db.session("A");
    let long: String = (0..600_000).map(|i| if i % 3 == 0 { 'é' } else { 'a' }).collect();
    let changed = format!("{}b", long.strip_suffix('a').unwrap());
    for (table, key, text) in [("T", 1, &*long), ("T", 2, "x"), ("T", 3, &changed), ("U", 10, &long), ("U", 20, "x ")] {
        let p = a.prepare(&format!("INSERT INTO {table} VALUES (?, ?)")).unwrap();
        a.execute_prepared(&p, &[Value::Lob(Lob::of_text(text)), Value::Exact(Decimal::new(key, 0))]).unwrap();
    }
    a.commit().unwrap();
    (db, a, long)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.1, §9.12 SR3, §9.14 SR3; §11.7
/// SR6, §11.8 SR4] a large object is an operand of no ordering or grouping
/// operation (SQLCODE -103): ORDER BY, GROUP BY, DISTINCT, MAX, `<`, UNIQUE as
/// a predicate, and UNIQUE, PRIMARY KEY and FOREIGN KEY constraints.
#[test]
fn large_objects_are_neither_ordered_nor_grouped_9_12_9_14() {
    let (db, a, _) = tables();
    for sql in [
        "SELECT K FROM T ORDER BY C",
        "SELECT C, COUNT(*) FROM T GROUP BY C",
        "SELECT DISTINCT C FROM T",
        "SELECT MAX(C) FROM T",
        "SELECT K FROM T WHERE C < 'x'",
        "SELECT K FROM T WHERE UNIQUE (SELECT C FROM U)",
        "SELECT C FROM T UNION SELECT C FROM U",
    ] {
        assert_eq!(code(&a, sql), Code::SyntaxRule, "{sql}");
    }
    for table in ["(C CLOB UNIQUE)", "(C BLOB PRIMARY KEY)", "(C CLOB, UNIQUE (C))"] {
        let refused = db.schema(&format!("CREATE SCHEMA AUTHORIZATION B CREATE TABLE V {table}")).unwrap_err();
        assert_eq!(refused.code, Code::SyntaxRule, "{table}");
    }
    let foreign = db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE P (C CLOB) CREATE TABLE F (C CLOB REFERENCES P (C))");
    assert_eq!(foreign.unwrap_err().code, Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT C FROM T UNION ALL SELECT C FROM U").unwrap().len(), 5, "UNION ALL groups nothing");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2, §8.4, §8.9, §8.12 GR2,
/// §7.10] equality of values of many chunks, read as compared: `=` and `<>`, IN
/// and NOT IN, `= ANY`, MATCH of a value and of a row, EXISTS of a correlated
/// equality and a join. The one differing at its last character matches none; a
/// character string pads as it compares.
#[test]
fn large_objects_of_many_chunks_compare_equal_as_read_8_2() {
    let (_db, a, _) = tables();
    let keys = |cond: &str| rows(&a, &format!("SELECT K FROM T WHERE {cond} ORDER BY K")).unwrap();
    assert_eq!(keys("C = (SELECT C FROM U WHERE K = 10)"), ["1"]);
    assert_eq!(keys("C <> (SELECT C FROM U WHERE K = 10)"), ["2", "3"]);
    assert_eq!(keys("C IN (SELECT C FROM U)"), ["1", "2"]);
    assert_eq!(keys("C NOT IN (SELECT C FROM U)"), ["3"]);
    assert_eq!(keys("C = ANY (SELECT C FROM U)"), ["1", "2"]);
    assert_eq!(keys("C MATCH (SELECT C FROM U)"), ["1", "2"]);
    assert_eq!(keys("C MATCH UNIQUE (SELECT C FROM U)"), ["1", "2"]);
    assert_eq!(keys("(C, K) MATCH (SELECT C, 1 FROM U)"), ["1"]);
    assert_eq!(keys("(C, 1) IN (SELECT C, 1 FROM U)"), ["1", "2"]);
    assert_eq!(keys("EXISTS (SELECT 1 FROM U WHERE U.C = T.C)"), ["1", "2"]);
    assert_eq!(rows(&a, "SELECT T.K, U.K FROM T JOIN U ON T.C = U.C ORDER BY 1").unwrap(), ["1,10", "2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR11d, GR11g] LISTAGG of
/// character large objects (IA044, a CHARACTER VARYING of 32767 characters):
/// joined when short; a value longer than the result is SQLSTATE 22001 under ON
/// OVERFLOW ERROR, and counted but left out under TRUNCATE.
#[test]
fn listagg_of_character_large_objects_10_9() {
    let (_db, a, _) = tables();
    assert_eq!(rows(&a, "SELECT LISTAGG(C, ';') WITHIN GROUP (ORDER BY K) FROM U WHERE K = 20").unwrap(), ["x "]);
    assert_eq!(rows(&a, "SELECT LISTAGG(C, ';') WITHIN GROUP (ORDER BY K) FROM T WHERE K = 2").unwrap(), ["x"]);
    assert_eq!(state(&a, "SELECT LISTAGG(C, ';') WITHIN GROUP (ORDER BY K) FROM T"), "22001");
    let truncated = "SELECT LISTAGG(C, ';' ON OVERFLOW TRUNCATE WITH COUNT) WITHIN GROUP (ORDER BY K DESC) FROM T WHERE K < 3";
    assert_eq!(rows(&a, truncated).unwrap(), ["x;...(1)"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR3d] (IA060) LIKE of a
/// character large object under a collation: CASE_INSENSITIVE, of one character
/// at a time, folds it a chunk at a time, however long; UNICODE, of
/// contractions, holds a short one whole, and refuses one past a chunk (SQLCODE
/// -900, SQLSTATE 0A000).
#[test]
fn like_of_large_objects_under_a_collation_8_5() {
    let (_db, a, long) = tables();
    let keys = |cond: &str| rows(&a, &format!("SELECT K FROM T WHERE {cond} ORDER BY K")).unwrap();
    assert_eq!(keys("C COLLATE CASE_INSENSITIVE LIKE 'ÉA%'"), ["1", "3"]);
    assert_eq!(keys("C COLLATE CASE_INSENSITIVE LIKE '%AB'"), ["3"]);
    assert_eq!(keys("C COLLATE CASE_INSENSITIVE LIKE 'X'"), ["2"]);
    let short = "SELECT 1 FROM (VALUES (CAST('xé' AS CLOB))) AS V (C) WHERE C COLLATE UNICODE LIKE 'x_'";
    assert_eq!(rows(&a, short).unwrap(), ["1"]);
    assert_eq!(state(&a, "SELECT K FROM T WHERE C COLLATE UNICODE LIKE 'x'"), "0A000");
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(C) FROM T WHERE K = 1").unwrap(), [long.chars().count().to_string()]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 SR2, GR4] (Features T022,
/// T042) the `<octet like predicate>`: `_` any octet, `%` any octets, of BINARY
/// VARYING and of a binary large object of many chunks, read as matched; an
/// escape octet making `%` itself; NOT; a null operand unknown; an escape octet
/// not one octet SQLSTATE 2200D, one followed by another octet 22025; a
/// character pattern of a binary match value SQLCODE -103.
#[test]
fn octet_like_of_binary_strings_and_large_objects_8_5() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE B (K INTEGER, V VARBINARY(10), L BLOB)").unwrap();
    let a = db.session("A");
    let long: Vec<u8> = (0..700_000_u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let p = a.prepare("INSERT INTO B VALUES (?, ?, ?)").unwrap();
    for (k, v, l) in [(1, &b"\x01%\x02"[..], &long[..]), (2, &b"\x01\x03\x02"[..], &b"\x00\xfa"[..])] {
        let v = Value::Binary(turborel::value::Binary::new(v.to_vec()));
        a.execute_prepared(&p, &[Value::Exact(Decimal::new(k, 0)), v, Value::Lob(Lob::of_octets(l))]).unwrap();
    }
    a.commit().unwrap();
    let keys = |cond: &str| rows(&a, &format!("SELECT K FROM B WHERE {cond} ORDER BY K")).unwrap();
    assert_eq!(keys("V LIKE X'015F02'"), ["1", "2"]);
    assert_eq!(keys("V LIKE X'01215F02' ESCAPE X'21'"), Vec::<String>::new());
    assert_eq!(keys("V LIKE X'01212502' ESCAPE X'21'"), ["1"]);
    assert_eq!(keys("V NOT LIKE X'01212502' ESCAPE X'21'"), ["2"]);
    assert_eq!(keys("L LIKE X'0001250405' || X'25'"), ["1"]);
    assert_eq!(keys("L LIKE X'2500FA'"), ["2"]);
    assert_eq!(long[long.len() - 3..], [0xD1, 0xD2, 0xD3]);
    assert_eq!(keys("L LIKE X'25D1D2D3'"), ["1"]);
    assert_eq!(keys("L LIKE X'25D1D2D35F'"), Vec::<String>::new());
    assert_eq!(keys("L LIKE CAST(NULL AS VARBINARY(1)) OR K = 2"), ["2"]);
    assert_eq!(state(&a, "SELECT K FROM B WHERE V LIKE X'01' ESCAPE X'2121'"), "2200D");
    assert_eq!(state(&a, "SELECT K FROM B WHERE V LIKE X'2101' ESCAPE X'21'"), "22025");
    assert_eq!(code(&a, "SELECT K FROM B WHERE V LIKE '%'"), Code::SyntaxRule);
}
