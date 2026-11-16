//! The regular expression functions of a character large object
//! [ISO/IEC 9075-2:2023 §8.7, §6.31, §6.33, §9.28, §9.29]: LIKE_REGEX,
//! OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX
//! of a value of many chunks, read a chunk at a time, matches crossing
//! chunks' ends; SUBSTRING_REGEX and TRANSLATE_REGEX of one giving one
//! (RULES.md D79).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::lob::{CHUNK_OCTETS, Lob};
use turborel::value::{Decimal, Value};

use crate::common::{code, rows, state};

/// Schema A: table T of an INTEGER K and a CLOB C, and table U of a BLOB.
fn lobs() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB) CREATE TABLE U (BL BLOB)").unwrap();
    db
}

/// Inserts row `k` of T holding `c`, as a parameter.
fn insert(s: &Session, k: i64, c: &str) {
    let p = s.prepare("INSERT INTO T VALUES (?, ?)").unwrap();
    s.execute_prepared(&p, &[Value::Exact(Decimal::new(i128::from(k), 0)), Value::Lob(Lob::of_text(c))]).unwrap();
}

/// The one value `sql` gives.
fn one(s: &Session, sql: &str) -> Value {
    let row = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row.into_iter().next().unwrap()
}

/// The text of the character large object `sql` gives.
fn clob(s: &Session, sql: &str) -> String {
    let Value::Lob(c) = one(s, sql) else { panic!("not a large object: {sql}") };
    String::from_utf8(c.reader().read(0, c.octets()).unwrap()).unwrap()
}

/// A text of about `octets` octets, of characters of 1 to 4 octets, with
/// `marker` where its first chunk ends, its octets on both sides.
fn around(octets: usize, marker: &str) -> String {
    let unit = "ab€é😀cd";
    let lead = CHUNK_OCTETS - marker.len().div_euclid(2);
    let mut before = unit.repeat(lead.div_euclid(unit.len()));
    before.push_str(&"a".repeat(lead - before.len()));
    let after = unit.repeat(octets.saturating_sub(before.len()).div_euclid(unit.len()));
    format!("{before}{marker}{after}")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.7 GR1; §9.28] LIKE_REGEX of a
/// CLOB of 600 KB and more: a match crossing a chunk's end, a back-reference to
/// a capture in the chunk before, anchors at its start and end, and none for a
/// pattern it has not.
#[test]
fn like_regex_of_a_large_object_8_7() {
    let db = lobs();
    let a = db.session("A");
    insert(&a, 1, &around(600_000, "#needle€😀#needle€😀#"));
    insert(&a, 2, &around(900_000, "#needle€😀#Needle€😀#"));
    let keys = |sql: &str| rows(&a, sql).unwrap();
    assert_eq!(keys("SELECT K FROM T WHERE C LIKE_REGEX 'needle€😀#n' ORDER BY K"), ["1"]);
    assert_eq!(keys("SELECT K FROM T WHERE C LIKE_REGEX '#([a-z€😀]+)#\\1#' ORDER BY K"), ["1"]);
    assert_eq!(keys("SELECT K FROM T WHERE C LIKE_REGEX '#([a-z€😀]+)#\\1#' FLAG 'i' ORDER BY K"), ["1", "2"]);
    assert_eq!(keys("SELECT K FROM T WHERE C LIKE_REGEX '^ab€.*😀cd$' FLAG 's' ORDER BY K"), ["1", "2"]);
    assert_eq!(keys("SELECT K FROM T WHERE NOT C LIKE_REGEX 'needles' ORDER BY K"), ["1", "2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR18, GR19]
/// OCCURRENCES_REGEX and POSITION_REGEX of a CLOB of 600 KB: matches counted
/// across chunks' ends, where one or its group begins and after it ends in
/// characters and in octets, from a start position, and 0 or -1 where there is
/// none.
#[test]
fn occurrences_and_positions_in_a_large_object_6_31() {
    let db = lobs();
    let a = db.session("A");
    let s = around(600_000, "<x€y>");
    insert(&a, 1, &s);
    let n = |sql: &str| rows(&a, &format!("SELECT {sql} FROM T")).unwrap().join(",");
    assert_eq!(n("OCCURRENCES_REGEX('😀' IN C)"), s.matches('😀').count().to_string());
    assert_eq!(n("OCCURRENCES_REGEX('d(a|<)' IN C)"), (s.matches("da").count() + s.matches("d<").count()).to_string());
    let marker = s.find("<x€y>").unwrap();
    let chars = s.get(..marker).unwrap().chars().count();
    assert_eq!(n("POSITION_REGEX('<x(€)y>' IN C)"), (chars + 1).to_string());
    assert_eq!(n("POSITION_REGEX(AFTER '<x(€)y>' IN C)"), (chars + 6).to_string());
    assert_eq!(n("POSITION_REGEX('<x(€)y>' IN C GROUP 1)"), (chars + 3).to_string());
    assert_eq!(n("POSITION_REGEX('<x(€)y>' IN C USING OCTETS)"), (marker + 1).to_string());
    assert_eq!(n("POSITION_REGEX(AFTER '<x(€)y>' IN C USING OCTETS GROUP 1)"), (marker + 6).to_string());
    let from = format!("POSITION_REGEX('😀' IN C FROM {} USING OCTETS)", marker + 1);
    let next = marker + s.get(marker..).unwrap().find('😀').unwrap();
    assert_eq!(n(&from), (next + 1).to_string());
    assert_eq!(n("POSITION_REGEX('<x€y>' IN C OCCURRENCE 2)"), "0");
    assert_eq!(n(&format!("OCCURRENCES_REGEX('a' IN C FROM {})", s.chars().count() + 1)), "-1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR4b, SR8f i 2]
/// SUBSTRING_REGEX of a CLOB is a CLOB: a group of a match longer than a chunk,
/// across chunks' ends, and a short one; null where there is no such match.
#[test]
fn substring_regex_of_a_large_object_6_33() {
    let db = lobs();
    let a = db.session("A");
    let s = around(700_000, "[€]");
    insert(&a, 1, &s);
    let open = s.find('[').unwrap();
    let long = format!("{}[", s.get(..open).unwrap());
    assert_eq!(clob(&a, "SELECT SUBSTRING_REGEX('^(.*)€\\]' IN C GROUP 1) FROM T"), long);
    assert_eq!(clob(&a, "SELECT SUBSTRING_REGEX('\\[.\\]' IN C) FROM T"), "[€]");
    assert_eq!(clob(&a, "SELECT SUBSTRING_REGEX('😀c' IN C OCCURRENCE 3) FROM T"), "😀c");
    assert_eq!(clob(&a, "SELECT SUBSTRING_REGEX('.*' IN C FROM 2 USING OCTETS) FROM T"), s.get(1..).unwrap().to_owned());
    assert!(matches!(one(&a, "SELECT SUBSTRING_REGEX('\\[.\\]' IN C OCCURRENCE 2) FROM T"), Value::Null));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 GR9b, SR12f i; §9.29]
/// TRANSLATE_REGEX of a CLOB of 600 KB and more is a CLOB: each match replaced,
/// longer than the subject; one occurrence replaced; groups put in, one
/// crossing a chunk's end; and the subject itself where nothing matches.
#[test]
fn translate_regex_of_a_large_object_6_33() {
    let db = lobs();
    let a = db.session("A");
    let s = around(650_000, "{split€😀}");
    insert(&a, 1, &s);
    assert_eq!(clob(&a, "SELECT TRANSLATE_REGEX('€' IN C WITH '<euro>') FROM T"), s.replace('€', "<euro>"));
    assert_eq!(clob(&a, "SELECT TRANSLATE_REGEX('😀' IN C) FROM T"), s.replace('😀', ""));
    assert_eq!(clob(&a, "SELECT TRANSLATE_REGEX('\\{([a-z]+)(€)' IN C WITH '$2$1') FROM T"), s.replace("{split€", "€split"));
    let second = s.match_indices("cd").nth(1).unwrap().0;
    assert_eq!(
        clob(&a, "SELECT TRANSLATE_REGEX('cd' IN C WITH 'X' OCCURRENCE 2) FROM T"),
        format!("{}X{}", s.get(..second).unwrap(), s.get(second + 2..).unwrap())
    );
    assert_eq!(clob(&a, "SELECT TRANSLATE_REGEX('nothing' IN C WITH 'X') FROM T"), s);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR4, GR5; §9.29 GR3, GR5;
/// §6.31 SR3a] the regular expression functions of a CLOB refuse as of a
/// string: a pattern that is none, SQLSTATE 2201S; a flag that is none, 2201T;
/// a pattern matching the zero-length string replaced, 2201U; a replacement
/// that is none, 2201V; and a BLOB is no subject (SQLCODE -104 is the syntax
/// rule's -105 here).
#[test]
fn regular_expressions_of_a_large_object_refused() {
    let db = lobs();
    let a = db.session("A");
    insert(&a, 1, &around(600_000, "abc"));
    assert_eq!(state(&a, "SELECT K FROM T WHERE C LIKE_REGEX '('"), "2201S");
    assert_eq!(state(&a, "SELECT OCCURRENCES_REGEX('a' FLAG 'z' IN C) FROM T"), "2201T");
    assert_eq!(state(&a, "SELECT TRANSLATE_REGEX('x*' IN C WITH 'y') FROM T"), "2201U");
    assert_eq!(state(&a, "SELECT TRANSLATE_REGEX('a' IN C WITH '\\n') FROM T"), "2201V");
    assert_eq!(code(&a, "SELECT OCCURRENCES_REGEX('a' IN BL) FROM U"), Code::SyntaxRule);
}
