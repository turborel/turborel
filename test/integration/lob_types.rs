//! Large object string types [ISO/IEC 9075-2:2023 §4.3, §4.4, §5.2, §6.1]:
//! CHARACTER LARGE OBJECT, NATIONAL CHARACTER LARGE OBJECT and BINARY
//! LARGE OBJECT, their lengths and multipliers, and their values kept by a
//! table in chunks, read back whole or by length, across commit and
//! rollback (RULES.md D79).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// Schema A: table T of an INTEGER K, a CLOB C and a BLOB B.
fn lobs() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB, B BLOB)").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// A text of `n` characters, each of `per` octets in UTF-8, so a chunk's
/// end falls within a character.
fn text_of(n: usize, per: usize) -> String {
    let c = match per {
        1 => 'a',
        2 => 'é',
        3 => '€',
        _ => '😀',
    };
    std::iter::repeat_n(c, n).collect()
}

/// Inserts row `key` of T holding `text` and `octets`, as parameters.
fn insert(session: &Session, key: i64, text: &str, octets: &[u8]) {
    let prepared = session.prepare("INSERT INTO T VALUES (?, ?, ?)").unwrap();
    let values = [Value::Exact(Decimal::new(i128::from(key), 0)), Value::Lob(Lob::of_text(text)), Value::Lob(Lob::of_octets(octets))];
    session.execute_prepared(&prepared, &values).unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR1, SR10-SR13, SR17; §5.2
/// SR10] CHARACTER LARGE OBJECT, CHAR LARGE OBJECT and CLOB, NATIONAL CHARACTER
/// LARGE OBJECT, NCHAR LARGE OBJECT and NCLOB, BINARY LARGE OBJECT and BLOB; a
/// `<large object length>` of digits alone, or with a multiplier K, M or G, as
/// one token or two, of characters or octets; of 4G if none is written (ID068),
/// the longest (IL042), so a length of T or P, or past 4G, is valid but not
/// processed (SQLCODE -900), and one of 0 is refused (SQLCODE -103).
#[test]
fn the_large_object_types_6_1() {
    let db = lobs();
    let a = db.session("A");
    a.run(
        "CREATE TABLE U (A CHARACTER LARGE OBJECT(10), B CHAR LARGE OBJECT(2K), C CLOB(3 M CHARACTERS), D NCLOB(1G), \
         E NATIONAL CHARACTER LARGE OBJECT, F NCHAR LARGE OBJECT(5 OCTETS), G BINARY LARGE OBJECT(64k), H BLOB, I CLOB(4G))",
    )
    .unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'U' ORDER BY 1";
    assert_eq!(
        squeezed(&a, described),
        [
            "A,CHARACTER LARGE OBJECT,10,40",
            "B,CHARACTER LARGE OBJECT,2048,8192",
            "C,CHARACTER LARGE OBJECT,3145728,12582912",
            "D,CHARACTER LARGE OBJECT,1073741824,4294967296",
            "E,CHARACTER LARGE OBJECT,4294967296,17179869184",
            "F,CHARACTER LARGE OBJECT,5,5",
            "G,BINARY LARGE OBJECT,65536,65536",
            "H,BINARY LARGE OBJECT,4294967296,4294967296",
            "I,CHARACTER LARGE OBJECT,4294967296,17179869184",
        ]
    );
    assert_eq!(exec(&a, "CREATE TABLE W (A CLOB(1T))").err(), Some(Code::Unsupported), "T is past the longest");
    assert_eq!(exec(&a, "CREATE TABLE W (A BLOB(1 P))").err(), Some(Code::Unsupported), "P is past the longest");
    assert_eq!(exec(&a, "CREATE TABLE W (A BLOB(4294967297))").err(), Some(Code::Unsupported), "past 4G");
    assert_eq!(exec(&a, "CREATE TABLE W (A CLOB(0))").err(), Some(Code::SyntaxRule), "more than 0");
    assert_eq!(exec(&a, "CREATE TABLE W (A CLOB(5X))").err().map(|c| c == Code::Lexical || c == Code::Syntax), Some(true), "no X");
    a.run("CREATE TABLE W (CLOB INTEGER, BLOB INTEGER, LARGE INTEGER, OBJECT INTEGER)").unwrap();
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3, §4.4; §6.30, §6.31] values
/// of a CLOB and a BLOB inserted and read back: short ones, and ones of many
/// chunks, characters of each width crossing chunks' ends; CHAR_LENGTH and
/// CHARACTER_LENGTH of the CLOB, in characters or octets, and OCTET_LENGTH of
/// each, from its counts; the same after commit, and none of a row whose insert
/// was rolled back.
#[test]
fn large_objects_are_kept_and_read_back() {
    let db = lobs();
    let a = db.session("A");
    insert(&a, 1, "short", &[1, 2, 3]);
    let long = format!("{}{}{}", text_of(300_000, 1), text_of(200_000, 3), text_of(100_000, 4));
    let octets: Vec<u8> = (0..700_000_u32).map(|i| (i % 251) as u8).collect();
    insert(&a, 2, &long, &octets);
    let lengths = "SELECT K, CHAR_LENGTH(C), OCTET_LENGTH(C), CHARACTER_LENGTH(C USING OCTETS), OCTET_LENGTH(B) FROM T ORDER BY K";
    let expected = ["1,5,5,5,3", "2,600000,1300000,1300000,700000"];
    assert_eq!(squeezed(&a, lengths), expected);
    a.commit().unwrap();
    assert_eq!(squeezed(&a, lengths), expected, "after commit");
    let read = a.open_cursor("SELECT C, B FROM T WHERE K = 2").unwrap().fetch().unwrap().unwrap();
    let [Value::Lob(c), Value::Lob(b)] = read.as_slice() else { panic!("not large objects: {read:?}") };
    assert_eq!(c.reader().read(0, c.octets()).unwrap(), long.as_bytes(), "the text whole");
    assert_eq!(b.reader().read(0, b.octets()).unwrap(), octets, "the octets whole");
    assert_eq!(c.reader().read(299_998, 5).unwrap(), "aa€".as_bytes(), "a range across a character's width");
    insert(&a, 3, &long, &octets);
    a.rollback().unwrap();
    assert_eq!(squeezed(&a, "SELECT K FROM T ORDER BY K"), ["1", "2"], "the insert rolled back");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3b, GR3e; §6.13 GR10,
/// GR11; §17.9 GR5g] assignment of a large object to a column of a lesser
/// length: one longer than it of characters other than spaces, or of octets, is
/// refused (SQLSTATE 22001), whether of many chunks or few; its trailing spaces
/// are cut; a dynamic parameter's value is cast to the column's type, so cut; a
/// CAST to a shorter character string or large object cuts it, with the warning
/// 01004 if other than spaces were.
#[test]
fn large_objects_assigned_and_cast_shorter_9_2_6_13() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE S (C CLOB(10), B BLOB(4), L CLOB(300K)) CREATE TABLE W (K INTEGER, C CLOB, B BLOB)",
    )
    .unwrap();
    let a = db.session("A");
    let source = a.prepare("INSERT INTO W VALUES (?, ?, ?)").unwrap();
    let past = format!("{}x", text_of(307_200, 2));
    let spaced = format!("{}   ", text_of(307_200, 2));
    for (k, c, b) in [(1, "abcdefghijk", &[1_u8][..]), (2, "a", &[1, 2, 3, 4, 5]), (3, &*past, &[1]), (4, &*spaced, &[1, 2, 3, 4])] {
        a.execute_prepared(&source, &[Value::Exact(Decimal::new(k, 0)), Value::Lob(Lob::of_text(c)), Value::Lob(Lob::of_octets(b))])
            .unwrap();
    }
    let state = |sql: &str| a.run(sql).err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(state("INSERT INTO S (C) SELECT C FROM W WHERE K = 1").as_deref(), Some("22001"));
    assert_eq!(state("INSERT INTO S (B) SELECT B FROM W WHERE K = 2").as_deref(), Some("22001"));
    assert_eq!(state("INSERT INTO S (L) SELECT C FROM W WHERE K = 3").as_deref(), Some("22001"), "many chunks");
    a.run("INSERT INTO S SELECT CAST('abcdefghij  ' AS CLOB), B, C FROM W WHERE K = 4").unwrap();
    assert_eq!(squeezed(&a, "SELECT CHAR_LENGTH(C), OCTET_LENGTH(B), CHAR_LENGTH(L) FROM S"), ["10,4,307200"], "spaces cut");
    let insert = a.prepare("INSERT INTO S (C) VALUES (?)").unwrap();
    a.execute_prepared(&insert, &[Value::Lob(Lob::of_text("abcdefghijk"))]).unwrap();
    assert_eq!(squeezed(&a, "SELECT C FROM S WHERE L IS NULL"), ["abcdefghij"], "a parameter, cast");
    assert_eq!(rows(&a, "SELECT CAST(C AS VARCHAR(4)) FROM S WHERE L IS NOT NULL").unwrap(), ["abcd"]);
    assert_eq!(a.warning().map(|w| w.as_str().to_owned()).as_deref(), Some("01004"));
    assert_eq!(rows(&a, "SELECT CHAR_LENGTH(CAST(L AS CLOB(5))) FROM S WHERE L IS NOT NULL").unwrap(), ["5"]);
    assert_eq!(a.warning().map(|w| w.as_str().to_owned()).as_deref(), Some("01004"));
    assert_eq!(rows(&a, "SELECT CAST(CAST('ab  ' AS CLOB) AS VARCHAR(2)) FROM S WHERE L IS NOT NULL").unwrap(), ["ab"]);
    assert_eq!(a.warning(), None, "only spaces cut");
}
