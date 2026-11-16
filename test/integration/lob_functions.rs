//! The string functions of large objects [ISO/IEC 9075-2:2023 §6.31 to
//! §6.33, §8.5]: `||`, SUBSTRING, POSITION, TRIM, UPPER, LOWER, OVERLAY and
//! LIKE of a CLOB or a BLOB a table keeps in chunks, read a chunk at a
//! time, never whole (RULES.md D79); binary TRIM and OVERLAY of binary
//! strings (Features T022, T312); their result types and refusals.

use std::fmt::Write;

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::value::lob::Lob;
use turborel::value::{Decimal, Value};

use crate::common::{code, rows};

/// Schema A: table T of an INTEGER K, a CLOB C, a BLOB B, a VARCHAR(20) V
/// and a VARBINARY(10) X.
fn lobs() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, C CLOB(1M), B BLOB(2M), V VARCHAR(20), X VARBINARY(10))").unwrap();
    db
}

/// Inserts row `key` of T holding `text` and `octets`, as parameters, V 'é€' and X
/// X'0001'.
fn insert(session: &Session, key: i64, text: &str, octets: &[u8]) {
    let prepared = session.prepare("INSERT INTO T VALUES (?, ?, ?, 'é€', X'0001')").unwrap();
    let values = [Value::Exact(Decimal::new(i128::from(key), 0)), Value::Lob(Lob::of_text(text)), Value::Lob(Lob::of_octets(octets))];
    session.execute_prepared(&prepared, &values).unwrap();
}

/// The long text of row 2: 262140 'a's, then '€😀é' across the end of the
/// first chunk, then characters of one to four octets, 600060 in all.
fn long_text() -> String {
    let mixed: String = ['b', 'é', '€', '😀'].iter().cycle().take(337_917).collect();
    format!("{}€😀é{mixed}", "a".repeat(262_140))
}

/// The long octets of row 2: 700000 of them, 0 at both ends, X'FFEE'
/// across the end of the first chunk.
fn long_octets() -> Vec<u8> {
    let mut octets: Vec<u8> = (0..700_000_u32).map(|i| u8::try_from(i % 200 + 1).unwrap()).collect();
    octets.splice(262_143..262_145, [0xFF, 0xEE]);
    octets.splice(0..3, [0; 3]);
    octets.splice(699_995..700_000, [0; 5]);
    octets
}

/// T with row 1 short and row 2 long.
fn filled() -> (Database, Session) {
    let db = lobs();
    let a = db.session("A");
    insert(&a, 1, "  a€😀  ", &[0, 1, 2, 0]);
    insert(&a, 2, &long_text(), &long_octets());
    (db, a)
}

/// The first column of the one row of `sql`, a string, whole: a character
/// string's text, or a binary string's octets as hex.
fn first(s: &Session, sql: &str) -> String {
    let row = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    match row.first().unwrap() {
        Value::Lob(l) => {
            let bytes = l.reader().read(0, l.octets()).unwrap();
            match l.kind() {
                turborel::value::lob::LobKind::Character => String::from_utf8(bytes).unwrap(),
                turborel::value::lob::LobKind::Binary => hex(&bytes),
            }
        }
        v => v.to_string(),
    }
}

/// The SQLCODE class and SQLSTATE `sql` fails with as its rows are read.
fn refused(s: &Session, sql: &str) -> (Code, String) {
    let run = || -> Result<(), SqlError> {
        let mut c = s.open_cursor(sql)?;
        while c.fetch()?.is_some() {}
        Ok(())
    };
    let e = run().expect_err(sql);
    (e.code, e.sqlstate().as_str().to_owned())
}

/// The data type and maximum length of each column of view `view`.
fn described(s: &Session, view: &str) -> Vec<String> {
    let sql = format!(
        "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = '{view}' ORDER BY 1"
    );
    rows(s, &sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.32 SR3b i, SR6a i, GR2, GR3]
/// `||` of a CLOB and a character string, or of a BLOB and a binary string, is
/// a large object of their family, of the sum of their lengths, joined without
/// a copy: lengths and text across a chunk's end; null if either is; a CLOB and
/// a BLOB refused (SQLCODE -103).
#[test]
fn concatenation_of_large_objects_6_32() {
    let (_db, a) = filled();
    a.run("CREATE VIEW J AS SELECT C || V AS CV, V || C AS VC, B || X AS BX, C || C AS CC FROM T").unwrap();
    assert_eq!(
        described(&a, "J"),
        [
            "BX,BINARY LARGE OBJECT,2097162",
            "CC,CHARACTER LARGE OBJECT,2097152",
            "CV,CHARACTER LARGE OBJECT,1048596",
            "VC,CHARACTER LARGE OBJECT,1048596"
        ]
    );
    let lengths = "SELECT CHAR_LENGTH(C || V), CHAR_LENGTH(V || C || C), OCTET_LENGTH(B || X), OCTET_LENGTH(X || B) FROM T WHERE K = 2";
    assert_eq!(rows(&a, lengths).unwrap(), ["600062,1200122,700002,700002"]);
    assert_eq!(first(&a, "SELECT SUBSTRING(C || V FROM 600058) FROM T WHERE K = 2"), "€😀bé€");
    assert_eq!(first(&a, "SELECT V || C FROM T WHERE K = 1"), "é€  a€😀  ");
    assert_eq!(first(&a, "SELECT X || B FROM T WHERE K = 1"), "000100010200");
    assert_eq!(
        rows(&a, "SELECT C || CAST(NULL AS VARCHAR(1)) IS NULL, CAST(NULL AS BLOB) || B IS NULL FROM T WHERE K = 1").unwrap(),
        ["TRUE,TRUE"]
    );
    assert_eq!(code(&a, "SELECT C || B FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT X || C FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR4a ii, SR21b, GR3, GR17]
/// SUBSTRING of a CLOB or a BLOB is a large object of its type, by characters,
/// or octets of a CLOB with USING OCTETS, whole characters alone (UV074),
/// across a chunk's end; of negative length SQLSTATE 22011; USING of a BLOB
/// refused (SQLCODE -103).
#[test]
fn substring_of_large_objects_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW S AS SELECT SUBSTRING(C FROM 2) AS SC, SUBSTRING(B FROM 2 FOR 3) AS SB FROM T").unwrap();
    assert_eq!(described(&a, "S"), ["SB,BINARY LARGE OBJECT,2097152", "SC,CHARACTER LARGE OBJECT,1048576"]);
    assert_eq!(first(&a, "SELECT SUBSTRING(C FROM 262139 FOR 6) FROM T WHERE K = 2"), "aa€😀éb");
    assert_eq!(first(&a, "SELECT SUBSTRING(C FROM 262140 FOR 8 USING OCTETS) FROM T WHERE K = 2"), "a€😀");
    assert_eq!(first(&a, "SELECT SUBSTRING(C FROM 262142 FOR 9 USING OCTETS) FROM T WHERE K = 2"), "😀éb");
    assert_eq!(first(&a, "SELECT SUBSTRING(B FROM 262143 FOR 4) FROM T WHERE K = 2"), expected_octets());
    assert_eq!(
        rows(&a, "SELECT CHAR_LENGTH(SUBSTRING(C FROM 600000)), OCTET_LENGTH(SUBSTRING(B FROM 0 FOR 3)) FROM T WHERE K = 2").unwrap(),
        ["61,2"]
    );
    assert_eq!(refused(&a, "SELECT SUBSTRING(C FROM 2 FOR -1) FROM T"), (Code::Data, "22011".to_owned()));
    assert_eq!(code(&a, "SELECT SUBSTRING(B FROM 2 USING OCTETS) FROM T"), Code::SyntaxRule);
}

/// The octets 262142 to 262145 of `long_octets`, as hex.
fn expected_octets() -> String {
    hex(&long_octets()[262_142..262_146])
}

/// `bytes` as hex, two digits an octet.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02X}").unwrap();
        out
    })
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR2, GR2, GR3] POSITION
/// with a CLOB or a BLOB as either operand, found without holding either whole,
/// a needle across a chunk's end, in characters or with USING OCTETS in octets;
/// 1 of an empty needle, 0 if none is found; a CLOB in a BLOB refused (SQLCODE
/// -103).
#[test]
fn position_in_large_objects_6_31() {
    let (_db, a) = filled();
    let sql = "SELECT POSITION('a€😀é' IN C), POSITION('a€😀é' IN C USING OCTETS), POSITION('😀😀' IN C), POSITION('' IN C), \
               POSITION(X'FFEE' IN B), POSITION(X'' IN B), POSITION(X'FFFF' IN B) FROM T WHERE K = 2";
    assert_eq!(rows(&a, sql).unwrap(), ["262140,262140,0,1,262144,1,0"]);
    let tail = "SELECT POSITION(SUBSTRING(C FROM 599990) IN C), POSITION(SUBSTRING(B FROM 262000 FOR 300000) IN B) FROM T WHERE K = 2";
    assert_eq!(rows(&a, tail).unwrap(), ["262146,262000"], "needles of large objects, the first of a periodic text");
    assert_eq!(
        rows(&a, "SELECT POSITION(SUBSTRING(C FROM 4 FOR 1) IN V), POSITION(C IN 'xx  a€😀  '), POSITION(X'01' IN B) FROM T WHERE K = 1")
            .unwrap(),
        ["2,3,2"]
    );
    assert_eq!(rows(&a, "SELECT POSITION(C IN CAST(NULL AS CLOB)) IS NULL FROM T WHERE K = 1").unwrap(), ["TRUE"]);
    assert_eq!(code(&a, "SELECT POSITION(C IN B) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT POSITION(X'01' IN B USING OCTETS) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR14b ii, SR15a ii, GR12,
/// GR13] TRIM, LTRIM, RTRIM and BTRIM of a CLOB are CLOBs, its ends read from
/// each side a chunk at a time: of a trim character not of one character
/// SQLSTATE 22027.
#[test]
fn trims_of_character_large_objects_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW R AS SELECT TRIM(C) AS T1, LTRIM(C, 'a') AS T2 FROM T").unwrap();
    assert_eq!(described(&a, "R"), ["T1,CHARACTER LARGE OBJECT,1048576", "T2,CHARACTER LARGE OBJECT,1048576"]);
    assert_eq!(first(&a, "SELECT TRIM(C) FROM T WHERE K = 1"), "a€😀");
    assert_eq!(first(&a, "SELECT TRIM(LEADING FROM C) FROM T WHERE K = 1"), "a€😀  ");
    assert_eq!(first(&a, "SELECT TRIM(TRAILING ' ' FROM C) FROM T WHERE K = 1"), "  a€😀");
    assert_eq!(first(&a, "SELECT BTRIM(C, ' 😀') FROM T WHERE K = 1"), "a€");
    let long =
        "SELECT CHAR_LENGTH(TRIM(LEADING 'a' FROM C)), CHAR_LENGTH(LTRIM(C, 'a€😀é')), CHAR_LENGTH(RTRIM(C, '😀€éb')) FROM T WHERE K = 2";
    assert_eq!(rows(&a, long).unwrap(), ["337920,337917,262140"]);
    assert_eq!(first(&a, "SELECT SUBSTRING(TRIM(LEADING 'a' FROM C) FROM 1 FOR 4) FROM T WHERE K = 2"), "€😀éb");
    assert_eq!(refused(&a, "SELECT TRIM('ab' FROM C) FROM T"), (Code::Data, "22027".to_owned()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR22, GR18] binary TRIM of
/// BINARY, VARBINARY and BLOB (Feature T022): X'00' trimmed if no trim octet is
/// written, BINARY VARYING of a binary string, a BLOB of a BLOB read from each
/// end; a trim octet not of one octet SQLSTATE 22027; a character trim octet
/// refused (SQLCODE -103).
#[test]
fn binary_trims_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW BT AS SELECT TRIM(X) AS T1, TRIM(LEADING X'01' FROM B) AS T2 FROM T").unwrap();
    assert_eq!(described(&a, "BT"), ["T1,BINARY VARYING,10", "T2,BINARY LARGE OBJECT,2097152"]);
    let short = "SELECT TRIM(X'0000AB00CD00'), TRIM(LEADING FROM X'0000AB00'), TRIM(TRAILING X'AB' FROM X'ABCDABAB'), \
                 TRIM(BOTH X'AB' FROM CAST(X'ABAB' AS BINARY(2))), TRIM(X) FROM T WHERE K = 1";
    assert_eq!(rows(&a, short).unwrap(), ["AB00CD,AB00,ABCD,,01"]);
    assert_eq!(first(&a, "SELECT TRIM(B) FROM T WHERE K = 1"), "0102");
    assert_eq!(
        rows(&a, "SELECT OCTET_LENGTH(TRIM(B)), OCTET_LENGTH(TRIM(TRAILING FROM B)) FROM T WHERE K = 2").unwrap(),
        ["699992,699995"]
    );
    assert_eq!(
        rows(&a, "SELECT TRIM(X'01' FROM CAST(NULL AS BLOB)) IS NULL, TRIM(CAST(NULL AS BINARY(1)) FROM B) IS NULL FROM T WHERE K = 1")
            .unwrap(),
        ["TRUE,TRUE"]
    );
    assert_eq!(refused(&a, "SELECT TRIM(X'0102' FROM X'01') FROM T"), (Code::Data, "22027".to_owned()));
    assert_eq!(refused(&a, "SELECT TRIM(X'' FROM B) FROM T"), (Code::Data, "22027".to_owned()));
    assert_eq!(code(&a, "SELECT TRIM(' ' FROM X'00') FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT LTRIM(B, X'00') FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR9, GR7] UPPER and LOWER
/// of a CLOB are CLOBs of its type, folded a chunk at a time into the
/// database's temporary store; of a BLOB refused (SQLCODE -103).
#[test]
fn folds_of_character_large_objects_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW F AS SELECT UPPER(C) AS U FROM T").unwrap();
    assert_eq!(described(&a, "F"), ["U,CHARACTER LARGE OBJECT,1048576"]);
    let sql = "SELECT CHAR_LENGTH(UPPER(C)), OCTET_LENGTH(LOWER(C)), POSITION('A€😀éB' IN UPPER(C)) FROM T WHERE K = 2";
    assert_eq!(rows(&a, sql).unwrap(), ["600060,1106940,262140"]);
    assert_eq!(first(&a, "SELECT SUBSTRING(UPPER(C) FROM 600055) FROM T WHERE K = 2"), "😀Bé€😀B");
    assert_eq!(first(&a, "SELECT LOWER(UPPER(C)) FROM T WHERE K = 1"), "  a€😀  ");
    assert_eq!(code(&a, "SELECT UPPER(B) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR17, SR23] OVERLAY
/// (Feature T312) of character strings, CLOBs and binary strings: SUBSTRING (CV
/// FROM 1 FOR SP - 1) || RS || SUBSTRING (CV FROM SP + SL), SL the length of RS
/// if not written, in octets with USING OCTETS; its type theirs; of SP less
/// than 1 SQLSTATE 22011; strings of two families, or USING of binary strings,
/// refused (SQLCODE -103).
#[test]
fn overlay_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW O AS SELECT OVERLAY(V PLACING V FROM 1) AS O1, OVERLAY(C PLACING V FROM 2 FOR 1) AS O2, OVERLAY(X PLACING X'01' FROM 1) AS O3 FROM T").unwrap();
    assert_eq!(described(&a, "O"), ["O1,CHARACTER VARYING,60", "O2,CHARACTER LARGE OBJECT,2097172", "O3,BINARY VARYING,21"]);
    let short = "SELECT OVERLAY('abcdef' PLACING 'XY' FROM 3), OVERLAY('abcdef' PLACING 'XY' FROM 3 FOR 0), OVERLAY('abc' PLACING 'XYZ' FROM 2 FOR 9), \
                 OVERLAY('é€😀' PLACING 'x' FROM 3 FOR 3 USING OCTETS), OVERLAY('é€😀' PLACING 'x' FROM 2 USING CHARACTERS), \
                 OVERLAY(X'010203' PLACING X'FF' FROM 2), OVERLAY(X PLACING X'AABB' FROM 3) FROM T WHERE K = 1";
    assert_eq!(rows(&a, short).unwrap(), ["abXYef,abXYcdef,aXYZ,éx😀,éx😀,01FF03,0001AABB"]);
    assert_eq!(first(&a, "SELECT OVERLAY(C PLACING '<>' FROM 3 FOR 4) FROM T WHERE K = 1"), "  <> ");
    let long = "SELECT CHAR_LENGTH(OVERLAY(C PLACING 'Z' FROM 10 FOR 100)), OCTET_LENGTH(OVERLAY(B PLACING X'00' FROM 2 FOR 699998)) FROM T WHERE K = 2";
    assert_eq!(rows(&a, long).unwrap(), ["599961,3"]);
    assert_eq!(first(&a, "SELECT SUBSTRING(OVERLAY(C PLACING 'Z' FROM 262141 FOR 2) FROM 262140 FOR 3) FROM T WHERE K = 2"), "aZé");
    assert_eq!(rows(&a, "SELECT OVERLAY('abc' PLACING CAST(NULL AS CLOB) FROM 1) IS NULL, OVERLAY(C PLACING 'x' FROM CAST(NULL AS INTEGER)) IS NULL FROM T WHERE K = 1").unwrap(), ["TRUE,TRUE"]);
    assert_eq!(refused(&a, "SELECT OVERLAY('abc' PLACING 'x' FROM 0) FROM T"), (Code::Data, "22011".to_owned()));
    assert_eq!(code(&a, "SELECT OVERLAY('abc' PLACING X'01' FROM 1) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT OVERLAY(X'01' PLACING X'02' FROM 1 USING OCTETS) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT OVERLAY('abc' PLACING 'x' FROM 1.5) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR3d] LIKE of a CLOB as its
/// match value, read a chunk at a time, with patterns of `%` and `_`, an escape
/// character, a parameter, and a CLOB as the pattern; a BLOB refused.
#[test]
fn like_of_character_large_objects_8_5() {
    let (_db, a) = filled();
    let matched = |cond: &str| rows(&a, &format!("SELECT K FROM T WHERE {cond} ORDER BY K")).unwrap();
    assert_eq!(matched("C LIKE '%€😀é%'"), ["2"]);
    assert_eq!(matched("C LIKE 'a%'"), ["2"]);
    assert_eq!(matched("C LIKE '%_😀b'"), ["2"]);
    assert_eq!(matched("C NOT LIKE '%😀b'"), ["1"]);
    assert_eq!(matched("C LIKE '  a€😀__'"), ["1"]);
    assert_eq!(matched("C LIKE '%a!%%' ESCAPE '!'"), Vec::<String>::new());
    assert_eq!(matched("C LIKE CAST('%€😀%' AS CLOB)"), ["1", "2"]);
    let p = a.prepare("SELECT K FROM T WHERE C LIKE ? ORDER BY K").unwrap();
    let mut c = a.open_prepared(&p, &[Value::Char("%aa€😀éb%".to_owned())]).unwrap();
    assert_eq!(c.fetch().unwrap().unwrap().first().unwrap().to_string(), "2");
    assert!(c.fetch().unwrap().is_none());
    assert_eq!(code(&a, "SELECT K FROM T WHERE B LIKE '%'"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR16c ii, GR14] (IL046)
/// LPAD and RPAD of a CLOB are CLOBs of 4G characters: padded past a chunk with
/// a padding of characters of two widths, repeated and cut, the source joined
/// after or before it; cut to the total length by characters, across a chunk's
/// end; the empty string of a length less than 1; null of a null length; of a
/// CLOB padding character value, the same.
#[test]
fn pads_of_character_large_objects_6_33() {
    let (_db, a) = filled();
    a.run("CREATE VIEW P AS SELECT LPAD(C, 5) AS L, RPAD(V, 5, C) AS R FROM T").unwrap();
    assert_eq!(described(&a, "P"), ["L,CHARACTER LARGE OBJECT,4294967296", "R,CHARACTER VARYING,32767"]);
    let long = long_text();
    let padded = "SELECT LPAD(C, 700000, 'é-') FROM T WHERE K = 1";
    let expected: String = "é-".chars().cycle().take(700_000 - 7).chain("  a€😀  ".chars()).collect();
    assert_eq!(first(&a, padded), expected);
    let right = first(&a, "SELECT RPAD(C, 600070, CAST('xé' AS CLOB)) FROM T WHERE K = 2");
    assert_eq!(right, format!("{long}xéxéxéxéxé"));
    let cut = first(&a, "SELECT RPAD(C, 262142, 'z') FROM T WHERE K = 2");
    assert_eq!(cut, long.chars().take(262_142).collect::<String>());
    assert_eq!(first(&a, "SELECT CHAR_LENGTH(LPAD(C, 0)) FROM T WHERE K = 2"), "0");
    assert_eq!(first(&a, "SELECT LPAD(C, CAST(NULL AS INTEGER)) IS NULL FROM T WHERE K = 2"), "TRUE");
    assert_eq!(first(&a, "SELECT CHAR_LENGTH(LPAD(C, 9, '')) FROM T WHERE K = 1"), "7");
}
