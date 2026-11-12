//! The regular expression functions [ISO/IEC 9075-2:2023 §6.31, §6.33,
//! §9.28, §9.29; RULES.md D61]: OCCURRENCES_REGEX, POSITION_REGEX,
//! SUBSTRING_REGEX and TRANSLATE_REGEX, in characters or octets.

use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{code, exec, expected, rows, setup};

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR3, GR18; §9.28 GR11]
/// OCCURRENCES_REGEX counts the matches from the start position, 1 if
/// none is written, each beginning after the one before begins and not
/// before it ends, empty ones too; -1 if the start position is less than
/// 1 or past the string; null if an operand is.
#[test]
fn occurrences_6_31() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (OCCURRENCES_REGEX('a' IN 'banana'), OCCURRENCES_REGEX('A' FLAG 'i' IN 'banana' FROM 3), \
             OCCURRENCES_REGEX('ana' IN 'banana'), OCCURRENCES_REGEX('x*' IN 'ab'), OCCURRENCES_REGEX('a|' IN 'ab'))";
    assert_eq!(rows(&s, q), expected(&["3,2,1,3,3"]));
    let q = "VALUES (OCCURRENCES_REGEX('a' IN 'banana' FROM 0), OCCURRENCES_REGEX('a' IN 'banana' FROM 7), \
             OCCURRENCES_REGEX('a' IN 'banana' FROM 6), OCCURRENCES_REGEX('x*' IN CAST('' AS VARCHAR(1))))";
    assert_eq!(rows(&s, q), expected(&["-1,-1,1,-1"]));
    let null = "CAST(NULL AS VARCHAR(1))";
    let q = format!(
        "VALUES (OCCURRENCES_REGEX({null} IN 'a'), OCCURRENCES_REGEX('a' FLAG {null} IN 'a'), OCCURRENCES_REGEX('a' IN {null}), \
         OCCURRENCES_REGEX('a' IN 'a' FROM CAST(NULL AS INTEGER)))"
    );
    assert_eq!(rows(&s, &q), expected(&["NULL,NULL,NULL,NULL"]));
    assert_eq!(rows(&s, "VALUES OCCURRENCES_REGEX('(' IN 'a' FROM 5)"), expected(&["-1"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR4, GR19] POSITION_REGEX
/// is where the occurrence-th match or its group begins, or under AFTER
/// the position after it ends; 0 if there is no such match, the group
/// matched nothing, the occurrence is less than 1, the group is none of
/// the pattern's, or the start position is not of the string.
#[test]
fn position_6_31() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (POSITION_REGEX('an' IN 'banana'), POSITION_REGEX(START 'an' IN 'banana' OCCURRENCE 2), \
             POSITION_REGEX(AFTER 'an' IN 'banana' OCCURRENCE 2), POSITION_REGEX('(a)(n)' IN 'banana' FROM 3 GROUP 2), \
             POSITION_REGEX(AFTER 'x*' IN 'ab' OCCURRENCE 3))";
    assert_eq!(rows(&s, q), expected(&["2,4,6,5,3"]));
    let q = "VALUES (POSITION_REGEX('z' IN 'banana'), POSITION_REGEX('(a)|(b)' IN 'a' GROUP 2), \
             POSITION_REGEX('a' IN 'banana' OCCURRENCE 0), POSITION_REGEX('(a)' IN 'banana' GROUP 2), \
             POSITION_REGEX('a' IN 'banana' GROUP -1), POSITION_REGEX('a' IN 'banana' FROM 9), \
             POSITION_REGEX('(' IN 'a' OCCURRENCE 0))";
    assert_eq!(rows(&s, q), expected(&["0,0,0,0,0,0,0"]));
    let null = "CAST(NULL AS INTEGER)";
    let q = format!("VALUES (POSITION_REGEX('a' IN 'a' OCCURRENCE {null}), POSITION_REGEX('a' IN 'a' GROUP {null}))");
    assert_eq!(rows(&s, &q), expected(&["NULL,NULL"]));
    exec(&s, "CREATE TABLE SIDES (START VARCHAR(5), AFTER VARCHAR(5))").unwrap();
    exec(&s, "INSERT INTO SIDES VALUES ('b', 'n')").unwrap();
    let q = "SELECT POSITION_REGEX(START IN 'banana'), POSITION_REGEX(AFTER FLAG 'i' IN 'banana'), \
             POSITION_REGEX(AFTER START IN 'banana') FROM SIDES";
    assert_eq!(rows(&s, q), expected(&["1,3,2"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR8, GR4] SUBSTRING_REGEX
/// is what the occurrence-th match or its group matched; null if there is
/// no such match, the group matched nothing, the occurrence is less than
/// 1, the group is none of the pattern's, the start position is not of
/// the string, or an operand is null.
#[test]
fn substring_6_33() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (SUBSTRING_REGEX('[0-9]+' IN 'ab 12 cd 345'), SUBSTRING_REGEX('[0-9]+' IN 'ab 12 cd 345' OCCURRENCE 2), \
             SUBSTRING_REGEX('(\\w+)@(\\w+)' IN 'me@host' GROUP 2), SUBSTRING_REGEX('b+' IN 'abbcbb' FROM 4), \
             SUBSTRING_REGEX('x*' IN 'ab') || '-')";
    assert_eq!(rows(&s, q), expected(&["12,345,host,bb,-"]));
    let q = "VALUES (SUBSTRING_REGEX('z' IN 'ab'), SUBSTRING_REGEX('(a)|(b)' IN 'a' GROUP 2), \
             SUBSTRING_REGEX('a' IN 'a' OCCURRENCE 0), SUBSTRING_REGEX('a' IN 'a' GROUP 1), \
             SUBSTRING_REGEX('a' IN 'a' FROM 2), SUBSTRING_REGEX('a' IN CAST(NULL AS VARCHAR(1))))";
    assert_eq!(rows(&s, q), expected(&["NULL,NULL,NULL,NULL,NULL,NULL"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR12, GR9; §9.29]
/// TRANSLATE_REGEX replaces every match, or the occurrence-th, as
/// fn:replace replaces it, `$N` a group; the string as it is if there is
/// no such occurrence; the replacement the empty string if none is
/// written; null if an operand is or the start position is not of the
/// string.
#[test]
fn translate_6_33() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (TRANSLATE_REGEX('a' IN 'banana' WITH 'o'), TRANSLATE_REGEX('a' IN 'banana' WITH 'o' OCCURRENCE 2), \
             TRANSLATE_REGEX('a' IN 'banana' WITH 'o' OCCURRENCE ALL), TRANSLATE_REGEX('(\\w+) (\\w+)' IN 'hello world' WITH '$2 $1'), \
             TRANSLATE_REGEX('a' IN 'banana'), TRANSLATE_REGEX('a' IN 'banana' WITH '<$0>' FROM 4))";
    assert_eq!(rows(&s, q), expected(&["bonono,banona,bonono,world hello,bnn,ban<a>n<a>"]));
    let q = "VALUES (TRANSLATE_REGEX('a' IN 'banana' WITH 'o' OCCURRENCE 4), TRANSLATE_REGEX('a' IN 'banana' WITH 'o' OCCURRENCE 0), \
             TRANSLATE_REGEX('^a|b$' IN 'ab' WITH '-'), TRANSLATE_REGEX('.' FLAG 'q' IN 'a.b' WITH '$'))";
    assert_eq!(rows(&s, q), expected(&["banana,banana,--,a$b"]));
    let null = "CAST(NULL AS VARCHAR(1))";
    let q = format!(
        "VALUES (TRANSLATE_REGEX('a' IN 'a' WITH {null}), TRANSLATE_REGEX('a' IN 'a' OCCURRENCE CAST(NULL AS INTEGER)), \
         TRANSLATE_REGEX('a' IN 'a' FROM 2), TRANSLATE_REGEX('a' IN 'a' WITH '' OCCURRENCE ALL))"
    );
    assert_eq!(rows(&s, &q), expected(&["NULL,NULL,NULL,"]));
}

/// The SQLSTATE of the first row `sql` fetches, if it fails.
fn state(s: &turborel::db::Session, sql: &str) -> Result<(), String> {
    s.open_cursor(sql).and_then(|mut c| c.fetch()).map(|_| ()).map_err(|e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.28 GR4, GR5; §9.29 GR3,
/// GR5; §6.33 SR12f] a pattern no XQuery regular expression is 2201S, a
/// flag none 2201T; a match of no characters to replace 2201U, and a
/// replacement no XQuery replacement string 2201V, only once a match is
/// replaced; a result longer than the longest string 22001.
#[test]
fn errors_9_29() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(state(&s, "VALUES SUBSTRING_REGEX('(' IN 'a')"), Err("2201S".into()));
    assert_eq!(state(&s, "VALUES OCCURRENCES_REGEX('a' FLAG 'z' IN 'a')"), Err("2201T".into()));
    assert_eq!(state(&s, "VALUES TRANSLATE_REGEX('a*' IN 'baa' WITH 'x')"), Err("2201U".into()));
    assert_eq!(state(&s, "VALUES TRANSLATE_REGEX('a+' FLAG 'm' IN 'aa' WITH 'x' OCCURRENCE 1)"), Ok(()));
    assert_eq!(state(&s, "VALUES TRANSLATE_REGEX('a|$' IN 'aa' WITH 'x' OCCURRENCE 1)"), Err("2201U".into()));
    assert_eq!(state(&s, "VALUES TRANSLATE_REGEX('a' IN 'baa' WITH '$')"), Err("2201V".into()));
    assert_eq!(state(&s, "VALUES TRANSLATE_REGEX('a' IN 'baa' WITH '\\n')"), Err("2201V".into()));
    assert_eq!(rows(&s, "VALUES TRANSLATE_REGEX('z' IN 'baa' WITH '$')"), expected(&["baa"]));
    exec(&s, "CREATE TABLE LONG (L VARCHAR(20000))").unwrap();
    let p = s.prepare("INSERT INTO LONG VALUES (?)").unwrap();
    s.execute_prepared(&p, &[Value::Char("a".repeat(20_000))]).unwrap();
    assert_eq!(state(&s, "SELECT TRANSLATE_REGEX('a' IN L WITH 'bb') FROM LONG"), Err("22001".into()));
    assert_eq!(rows(&s, "SELECT CHAR_LENGTH(TRANSLATE_REGEX('a' IN L WITH 'b')) FROM LONG"), expected(&["20000"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR18, GR19; §6.33 GR4,
/// GR9; Feature F846] USING OCTETS counts positions, lengths and the start
/// position in octets of the string as UTF-8, a start position within a
/// character naming the next character; USING CHARACTERS as with none.
#[test]
fn octets_f846() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (POSITION_REGEX('é' IN 'aébé' USING OCTETS OCCURRENCE 2), POSITION_REGEX(AFTER 'é' IN 'aébé' USING OCTETS), \
             POSITION_REGEX('é' IN 'aébé' USING CHARACTERS OCCURRENCE 2), OCCURRENCES_REGEX('b' IN 'aébé' FROM 3 USING OCTETS), \
             OCCURRENCES_REGEX('é' IN 'aébé' FROM 6 USING OCTETS), OCCURRENCES_REGEX('é' IN 'aébé' FROM 7 USING OCTETS))";
    assert_eq!(rows(&s, q), expected(&["5,4,4,1,0,-1"]));
    let q = "VALUES (SUBSTRING_REGEX('b.' IN 'aébé' FROM 2 USING OCTETS), TRANSLATE_REGEX('é' IN 'aébé' WITH 'e' FROM 4 USING OCTETS))";
    assert_eq!(rows(&s, q), expected(&["bé,aébe"]));
    assert_eq!(code(&s, "VALUES OCCURRENCES_REGEX('a' IN 'a' USING BITS)"), Code::Syntax);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR1, SR3, SR4; §6.33
/// SR3, SR8f, SR12f; §20.7 GR5a xxxvi, xxxvii] the strings are character
/// strings and the numbers exact of scale 0; a count or position is
/// NUMERIC(18), SUBSTRING_REGEX varying as long as its subject, and
/// TRANSLATE_REGEX too with no replacement or the empty string, else of
/// the longest string; a `?` string CHARACTER VARYING beside a string not
/// a `?`, a `?` number NUMERIC; the names identifiers but before "(".
#[test]
fn operands_and_types_6_31() {
    let db = setup();
    let s = db.session("APP");
    for bad in [
        "VALUES OCCURRENCES_REGEX(1 IN 'a')",
        "VALUES POSITION_REGEX('a' FLAG 1 IN 'a')",
        "VALUES SUBSTRING_REGEX('a' IN 1)",
        "VALUES TRANSLATE_REGEX('a' IN 'a' WITH 1)",
        "VALUES OCCURRENCES_REGEX('a' IN 'a' FROM 1.0)",
        "VALUES POSITION_REGEX('a' IN 'a' OCCURRENCE '1')",
        "VALUES SUBSTRING_REGEX('a' IN 'a' GROUP 1E0)",
        "VALUES TRANSLATE_REGEX('a' IN X'61')",
    ] {
        assert_eq!(code(&s, bad), Code::SyntaxRule, "{bad}");
    }
    exec(&s, "CREATE TABLE W (N VARCHAR(12), C CHAR(4))").unwrap();
    let view = "CREATE VIEW V AS SELECT SUBSTRING_REGEX('a' IN N) AS S1, SUBSTRING_REGEX('a' IN C) AS S2, TRANSLATE_REGEX('a' IN N) AS T1, \
                TRANSLATE_REGEX('a' IN N WITH '') AS T2, TRANSLATE_REGEX('a' IN N WITH 'b') AS T3, POSITION_REGEX('a' IN N) AS P FROM W";
    exec(&s, view).unwrap();
    let q = "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_NAME = 'V' ORDER BY COLUMN_NAME";
    let types = [
        "P,NUMERIC,NULL,18",
        "S1,CHARACTER VARYING,12,NULL",
        "S2,CHARACTER VARYING,4,NULL",
        "T1,CHARACTER VARYING,12,NULL",
        "T2,CHARACTER VARYING,12,NULL",
        "T3,CHARACTER VARYING,32767,NULL",
    ];
    assert_eq!(rows(&s, q), expected(&types));
    let p = s.prepare("VALUES SUBSTRING_REGEX(? IN ? FROM ? OCCURRENCE ?)").unwrap();
    let five = Value::Exact(Decimal::new(5, 0));
    let mut c =
        s.open_prepared(&p, &[Value::Char("b.".into()), Value::Char("abcabd".into()), Value::Exact(Decimal::new(3, 0)), five]).unwrap();
    assert_eq!(c.fetch().unwrap().map(|r| r[0].to_string()), Some("NULL".into()));
    assert!(s.prepare("VALUES TRANSLATE_REGEX(? FLAG ? IN ? WITH ?)").is_err());
    exec(&s, "CREATE TABLE NAMES_OF (OCCURRENCES_REGEX INTEGER, SUBSTRING_REGEX INTEGER)").unwrap();
    exec(&s, "INSERT INTO NAMES_OF VALUES (1, 2)").unwrap();
    assert_eq!(rows(&s, "SELECT OCCURRENCES_REGEX + SUBSTRING_REGEX FROM NAMES_OF"), expected(&["3"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.33 SR8f ii, SR12f ii; F&O
/// 3.1 §5.6.3] SUBSTRING_REGEX and TRANSLATE_REGEX are of their subject's
/// collation, whatever the pattern's, so compare under it; matching
/// itself takes no account of a collation.
#[test]
fn collations_6_33() {
    let db = setup();
    let s = db.session("APP");
    exec(&s, "CREATE TABLE PEOPLE (N VARCHAR(10) COLLATE CASE_INSENSITIVE)").unwrap();
    exec(&s, "INSERT INTO PEOPLE VALUES ('Bob Smith')").unwrap();
    let q = "SELECT SUBSTRING_REGEX('\\w+' IN N) = 'BOB', TRANSLATE_REGEX(' .*' IN N) = 'bob', OCCURRENCES_REGEX('b' IN N) FROM PEOPLE";
    assert_eq!(rows(&s, q), expected(&["TRUE,TRUE,1"]));
    exec(&s, "CREATE TABLE PATS (P VARCHAR(10))").unwrap();
    exec(&s, "INSERT INTO PATS VALUES ('\\w+')").unwrap();
    let q = "SELECT SUBSTRING_REGEX(P IN N) = 'BOB', TRANSLATE_REGEX(P IN N WITH 'x') = 'X X' FROM PEOPLE, PATS";
    assert_eq!(rows(&s, q), expected(&["TRUE,TRUE"]));
}
