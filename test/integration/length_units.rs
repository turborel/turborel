//! Character length units [ISO/IEC 9075-2:2023 §6.1, §6.31, §6.33;
//! Feature T062]: CHAR_LENGTH, POSITION and SUBSTRING counting in
//! characters or octets, and character string types whose length is in
//! octets.

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, setup};

/// The SQLSTATE `sql` fails with, or none if it is taken.
fn state(s: &Session, sql: &str) -> Option<String> {
    s.open_cursor(sql).and_then(|mut c| c.fetch()).err().map(|e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR2, GR5; §6.33 GR3]
/// CHAR_LENGTH, POSITION and SUBSTRING count in characters, or with USING
/// OCTETS in octets of the string's UTF-8: a substring takes the whole
/// characters within its octets (UV074); a negative length is 22011, a
/// null operand null; USING CHARACTERS is the default.
#[test]
fn functions_count_in_units_6_31() {
    let db = setup();
    let s = db.session("APP");
    let q = "VALUES (CHAR_LENGTH('café' USING OCTETS), CHARACTER_LENGTH('café' USING CHARACTERS), CHAR_LENGTH('café'), \
             POSITION('cr' IN 'café crème' USING OCTETS), POSITION('cr' IN 'café crème' USING CHARACTERS), \
             POSITION('' IN 'é' USING OCTETS), POSITION('z' IN 'é' USING OCTETS))";
    assert_eq!(rows(&s, q), Ok(vec!["5,4,4,7,6,1,0".to_owned()]));
    let q = "VALUES ('[' || SUBSTRING('café crème' FROM 4 FOR 3 USING OCTETS) || ']', SUBSTRING('café' FROM 5 USING OCTETS), \
             '[' || SUBSTRING('café' FROM 4 FOR 1 USING OCTETS) || ']', SUBSTRING('café' FROM 2 FOR 2 USING CHARACTERS), \
             SUBSTRING('café' FROM 0 FOR 3 USING OCTETS))";
    assert_eq!(rows(&s, q), Ok(vec!["[é ],,[],af,ca".to_owned()]));
    let null = "CAST(NULL AS VARCHAR(1))";
    let q =
        format!("VALUES (CHAR_LENGTH({null} USING OCTETS), POSITION('a' IN {null} USING OCTETS), SUBSTRING({null} FROM 1 USING OCTETS))");
    assert_eq!(rows(&s, &q), Ok(vec!["NULL,NULL,NULL".to_owned()]));
    assert_eq!(state(&s, "VALUES SUBSTRING('abc' FROM 2 FOR -1 USING OCTETS)").as_deref(), Some("22011"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31, §6.33] USING is of a
/// character string's CHAR_LENGTH, POSITION and SUBSTRING alone: of a
/// binary string's SQLCODE -103, of another function or without its units
/// a syntax error.
#[test]
fn units_refused_6_31() {
    let db = setup();
    let s = db.session("APP");
    for sql in ["VALUES SUBSTRING(X'0102' FROM 1 USING OCTETS)", "VALUES POSITION(X'01' IN X'0102' USING CHARACTERS)"] {
        assert_eq!(code(&s, sql), Code::SyntaxRule, "{sql}");
    }
    assert_eq!(code(&s, "VALUES CHAR_LENGTH(X'01' USING OCTETS)"), Code::SyntaxRule);
    for sql in ["VALUES UPPER('a' USING OCTETS)", "VALUES CHAR_LENGTH('a' USING BYTES)", "VALUES OCTET_LENGTH('a' USING OCTETS)"] {
        assert_eq!(code(&s, sql), Code::Syntax, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR9, SR16; §9.2 GR3;
/// §6.13 GR5c, GR6c] a character string type's length in octets bounds
/// its values' UTF-8: a longer value is 22001 unless only spaces are past
/// it, a fixed one is padded to so many octets, CAST cuts whole characters
/// with the warning 01004; CHARACTERS is the default; the type is shown
/// with its units, its octet length its length.
#[test]
fn octet_lengths_6_1() {
    let db = setup();
    let s = db.session("APP");
    s.run("CREATE TABLE OCTETS_T (V VARCHAR(5 OCTETS), C CHAR(6 OCTETS), W VARCHAR(5 CHARACTERS))").unwrap();
    s.run("INSERT INTO OCTETS_T VALUES ('abcde', 'é', 'ééééé')").unwrap();
    assert_eq!(exec(&s, "INSERT INTO OCTETS_T VALUES ('ééé', 'a', 'a')").err(), Some(Code::Data), "6 octets");
    s.run("INSERT INTO OCTETS_T VALUES ('éé  ', 'ééé', 'a')").unwrap();
    let q = "SELECT '[' || V || ']', '[' || C || ']', OCTET_LENGTH(C), CHAR_LENGTH(C) FROM OCTETS_T ORDER BY 4 DESC";
    assert_eq!(rows(&s, q), Ok(vec!["[abcde],[é    ],6,5".to_owned(), "[éé ],[ééé],6,3".to_owned()]));
    let q = "SELECT CAST('ééé' AS VARCHAR(5 OCTETS)), '[' || CAST('ééé' AS CHAR(5 OCTETS)) || ']' FROM OCTETS_T WHERE V = 'abcde'";
    assert_eq!(rows(&s, q), Ok(vec!["éé,[éé ]".to_owned()]));
    assert_eq!(state(&s, "VALUES CAST('ééé' AS VARCHAR(5 OCTETS))"), None, "a warning only");
    let q = "SELECT COLUMN_NAME, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_NAME = 'OCTETS_T' ORDER BY COLUMN_NAME";
    assert_eq!(rows(&s, q), Ok(vec!["C,6,6".to_owned(), "V,5,5".to_owned(), "W,5,20".to_owned()]));
    s.commit().unwrap();
    s.run("CREATE INDEX TC ON OCTETS_T (C)").unwrap();
    assert_eq!(rows(&s, "SELECT V FROM OCTETS_T WHERE C = 'ééé'"), Ok(vec!["éé ".to_owned()]), "probed by its key");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR16; §9.5 SR3a; §6.33
/// SR3a] the result of strings brought together is in octets if all are,
/// else in characters, which a length in octets bounds; a concatenation
/// in octets if both are; a domain keeps its units.
#[test]
fn octet_lengths_brought_together_9_5() {
    let db = setup();
    let s = db.session("APP");
    s.run("CREATE TABLE OCTETS_U (A VARCHAR(4 OCTETS), B CHAR(6 OCTETS), C VARCHAR(3))").unwrap();
    s.run("CREATE VIEW OCTETS_W (AB, AC, JOINED, MIXED) AS SELECT CASE WHEN A = 'x' THEN A ELSE B END, COALESCE(A, C), A || B, A || C FROM OCTETS_U")
        .unwrap();
    let q = "SELECT COLUMN_NAME, CHARACTER_MAXIMUM_LENGTH, CHARACTER_OCTET_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_NAME = 'OCTETS_W' ORDER BY COLUMN_NAME";
    assert_eq!(rows(&s, q), Ok(vec!["AB,6,6".to_owned(), "AC,4,16".to_owned(), "JOINED,10,10".to_owned(), "MIXED,7,28".to_owned()]));
    s.run("CREATE DOMAIN SHORT AS VARCHAR(3 OCTETS)").unwrap();
    s.run("CREATE TABLE OCTETS_D (S SHORT)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO OCTETS_D VALUES ('éé')").err(), Some(Code::Data));
    s.run("INSERT INTO OCTETS_D VALUES ('aé')").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR16] RULES.md D25: a
/// length in octets is kept, of a row store's columns and a column
/// store's, after a crash and after a clean close.
#[test]
fn octet_lengths_are_kept() {
    let scratch = Scratch::new("length_units");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (C CHAR(4 OCTETS), V VARCHAR(4 OCTETS)) \
         CREATE TABLE K (N INTEGER, C CHAR(4 OCTETS)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO R VALUES ('é', 'éé')").unwrap();
    a.run("INSERT INTO K VALUES (1, 'é'), (2, 'ab')").unwrap();
    for _ in 0..11 {
        a.run("INSERT INTO K SELECT N + 2, C FROM K").unwrap();
    }
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT '[' || C || ']', V FROM R"), Ok(vec!["[é  ],éé".to_owned()]));
        assert_eq!(exec(&a, "INSERT INTO R (V) VALUES ('ééé')").err(), Some(Code::Data));
        let q = "SELECT COUNT(*), MIN('[' || C || ']'), MAX('[' || C || ']') FROM K WHERE C = 'é' OR C = 'ab'";
        assert_eq!(rows(&a, q), Ok(vec!["4096,[ab  ],[é  ]".to_owned()]));
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.1 GR3; §6.1 SR16b] a
/// value retrieved into a host variable whose length is in octets is cut
/// to whole characters of no more octets, a fixed one padded to them,
/// with the warning 01004 and its length in octets told.
#[test]
fn octet_lengths_are_retrieved_9_1() {
    use turborel::db::Completion;
    use turborel::error::SqlState;
    use turborel::params::Params;
    use turborel::value::{CharUnits, DataType, Spelled, Value};
    let db = setup();
    let s = db.session("APP");
    s.run("CREATE TABLE OCTETS_ONE (X INTEGER)").unwrap();
    s.run("INSERT INTO OCTETS_ONE VALUES (1)").unwrap();
    let declared = |out: DataType| {
        Params::new().declare("OUT", out, Value::Null).declare(
            "IND",
            DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
            Value::Null,
        )
    };
    let mut p = declared(DataType::Varying { length: 3, units: CharUnits::Octets });
    assert_eq!(s.execute("SELECT 'aé' INTO OUT IND FROM OCTETS_ONE", &mut p).unwrap(), Completion::Success);
    assert_eq!(p.value("OUT").to_string(), "aé");
    let mut p = declared(DataType::Character { length: 3, units: CharUnits::Octets });
    let done = s.execute("SELECT 'aéé' INTO OUT IND FROM OCTETS_ONE", &mut p).unwrap();
    assert_eq!(done, Completion::Warning(SqlState::STRING_TRUNCATED));
    assert_eq!((p.value("OUT").to_string(), p.value("IND").to_string()), ("aé".to_owned(), "5".to_owned()), "cut and told in octets");
}
