//! Thesaurus expansions of a search pattern [ISO/IEC 13249-2:2002
//! §5.3.1 Description 5k–5p, §6.11–§6.16] over thesauri held in the
//! tables of schema FT_THESAURUS [§8], through SQL.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::value::Value;

use super::documents;
use crate::common::{rows, state};

/// The thesaurus tables of clause 8 [§8.2–§8.6], SELECT on each granted to
/// PUBLIC. A TERMID is DEFAULT 0 NOT NULL, as SQL orders a column's
/// default before its constraints.
pub const TABLES: &str = "CREATE SCHEMA FT_THESAURUS AUTHORIZATION FT_THESAURUS \
    CREATE TABLE TERM_DICTIONARY (TERMID INTEGER DEFAULT 0 NOT NULL, EXPR VARCHAR(1024), THNAME_DIC VARCHAR(128) NOT NULL, \
        PRIMARY KEY (TERMID, THNAME_DIC)) \
    CREATE TABLE TERM_HIERARCHY (TERMID INTEGER NOT NULL, NARROWER_TERMID INTEGER NOT NULL, THNAME_HRR VARCHAR(128) NOT NULL, \
        PRIMARY KEY (TERMID, NARROWER_TERMID, THNAME_HRR)) \
    CREATE TABLE TERM_SYNONYM (TERMID INTEGER NOT NULL, SYNONYM_TERMID INTEGER NOT NULL, PREFERRED_TERMID INTEGER, \
        THNAME_SYN VARCHAR(128) NOT NULL, PRIMARY KEY (TERMID, SYNONYM_TERMID, THNAME_SYN)) \
    CREATE TABLE TERM_RELATED (TERMID INTEGER NOT NULL, RELATED_TERMID INTEGER NOT NULL, THNAME_REL VARCHAR(128) NOT NULL, \
        PRIMARY KEY (TERMID, RELATED_TERMID, THNAME_REL)) \
    GRANT SELECT ON TERM_DICTIONARY TO PUBLIC GRANT SELECT ON TERM_HIERARCHY TO PUBLIC \
    GRANT SELECT ON TERM_SYNONYM TO PUBLIC GRANT SELECT ON TERM_RELATED TO PUBLIC";

/// Thesaurus ZOO: animal > mammal > dog > puppy and mammal > cat; dog,
/// hound and canine synonyms, canine preferred; dog related to leash.
/// Thesaurus PETS, of the same TERMIDs: dog and cat synonyms.
const ZOO: [&str; 5] = [
    "INSERT INTO TERM_DICTIONARY VALUES (1, 'animal', 'ZOO'), (2, 'mammal', 'ZOO'), (3, 'dog', 'ZOO'), (4, 'puppy', 'ZOO'), \
        (5, 'hound', 'ZOO'), (6, 'cat', 'ZOO'), (7, 'leash', 'ZOO'), (8, 'canine', 'ZOO'), (3, 'dog', 'PETS'), (6, 'cat', 'PETS')",
    "INSERT INTO TERM_HIERARCHY VALUES (1, 2, 'ZOO'), (2, 3, 'ZOO'), (3, 4, 'ZOO'), (2, 6, 'ZOO')",
    "INSERT INTO TERM_SYNONYM VALUES (3, 5, 8, 'ZOO'), (5, 8, NULL, 'ZOO'), (3, 6, NULL, 'PETS')",
    "INSERT INTO TERM_RELATED VALUES (3, 7, 'ZOO')",
    "COMMIT WORK",
];

/// Schema A's DOCS of one word each, ID 1 to 9: animal, mammal, dog,
/// puppy, hound, cat, leash, canine, zebra, readable by PUBLIC; the empty
/// thesaurus tables; and a session of A and one of FT_THESAURUS.
fn zoo(db: &Database) -> (Session, Session) {
    let words = ["animal", "mammal", "dog", "puppy", "hound", "cat", "leash", "canine", "zebra"];
    let bodies: Vec<String> = words.iter().map(|w| format!("FULLTEXT('{w}')")).collect();
    let a = documents(db, &bodies.iter().map(String::as_str).collect::<Vec<_>>());
    a.run("GRANT SELECT ON DOCS TO PUBLIC").unwrap();
    a.run("COMMIT WORK").unwrap();
    db.schema(TABLES).unwrap();
    (a, db.session("FT_THESAURUS"))
}

/// The IDs of A.DOCS, as `s` sees them, that `THESAURUS "name" EXPAND
/// expansion` finds.
fn expanded(s: &Session, name: &str, expansion: &str) -> Vec<String> {
    let pattern = format!("THESAURUS \"{name}\" EXPAND {expansion}");
    rows(s, &format!("SELECT ID FROM A.DOCS WHERE CONTAINS(BODY, '{pattern}') = 1 ORDER BY ID")).unwrap()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5k–5p, NOTE 14,
/// §6.11.5–§6.16.5] each expansion finds the term and the terms related to
/// it by its relation: BROADER and NARROWER all the way, or n steps of FOR
/// n LEVELS, 0 the term alone; SYNONYM the synonyms of synonyms too;
/// RELATED the related ones; PREFERRED the preferred term and TOP the top
/// of the hierarchy, either without the term itself when there is one.
#[test]
fn each_expansion_follows_its_relation_6_11() {
    let db = Database::new();
    let (a, t) = zoo(&db);
    for sql in ZOO {
        t.run(sql).unwrap();
    }
    let cases: [(&str, &[&str]); 12] = [
        ("BROADER TERM OF \"dog\"", &["1", "2", "3"]),
        ("BROADER TERM OF \"dog\" FOR 1 LEVELS", &["2", "3"]),
        ("BROADER TERM OF \"dog\" FOR 0 LEVELS", &["3"]),
        ("NARROWER TERM OF \"mammal\"", &["2", "3", "4", "6"]),
        ("NARROWER TERM OF \"animal\" FOR 1 LEVEL", &["1", "2"]),
        ("NARROWER TERM OF \"animal\" FOR 2 LEVELS", &["1", "2", "3", "6"]),
        ("SYNONYM TERM OF \"dog\"", &["3", "5", "8"]),
        ("PREFERRED TERM OF \"dog\"", &["8"]),
        ("RELATED TERM OF \"dog\"", &["3", "7"]),
        ("TOP TERM OF \"puppy\"", &["1"]),
        ("narrower term of \"ANIMAL\" for 1 level", &["1", "2"]),
        ("SYNONYM TERM OF \"Dog\"", &["3", "5", "8"]),
    ];
    for (expansion, want) in cases {
        assert_eq!(expanded(&a, "ZOO", expansion), want, "{expansion}");
    }
    assert_eq!(expanded(&a, "PETS", "SYNONYM TERM OF \"dog\""), ["3", "6"]);
    assert_eq!(expanded(&a, "  ZOO ", "RELATED TERM OF \"dog\""), ["3", "7"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5k, §6.11.5,
/// §6.14.5 Description 5, §6.16.5] a thesaurus or term the tables do not
/// hold relates the term to nothing, so the term alone is found; so too
/// PREFERRED and TOP of a term with no preferred or top term.
#[test]
fn unknown_thesauri_and_terms_expand_to_nothing() {
    let db = Database::new();
    let (a, t) = zoo(&db);
    for sql in ZOO {
        t.run(sql).unwrap();
    }
    assert_eq!(expanded(&a, "NOPE", "SYNONYM TERM OF \"dog\""), ["3"]);
    assert_eq!(expanded(&a, "NOPE", "NARROWER TERM OF \"animal\""), ["1"]);
    assert_eq!(expanded(&a, "ZOO", "SYNONYM TERM OF \"zebra\""), ["9"]);
    assert_eq!(expanded(&a, "ZOO", "BROADER TERM OF \"zebra\""), ["9"]);
    assert_eq!(expanded(&a, "ZOO", "TOP TERM OF \"zebra\""), ["9"]);
    assert_eq!(expanded(&a, "ZOO", "PREFERRED TERM OF \"zebra\""), ["9"]);
    assert_eq!(expanded(&a, "ZOO", "PREFERRED TERM OF \"hound\""), ["5"]);
    assert_eq!(expanded(&a, "ZOO", "TOP TERM OF \"animal\""), ["1"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1, §8] [EXTENSION] (RULES.md
/// D87) a statement reads the thesaurus in its own snapshot, so rows a
/// transaction inserted and has not committed expand its own patterns and
/// no other session's, until it commits; rows rolled back expand none.
#[test]
fn uncommitted_thesaurus_rows_are_seen_by_their_transaction_only() {
    let db = Database::new();
    let (a, t) = zoo(&db);
    for sql in &ZOO[..4] {
        t.run(sql).unwrap();
    }
    assert_eq!(expanded(&t, "ZOO", "RELATED TERM OF \"dog\""), ["3", "7"]);
    assert_eq!(expanded(&a, "ZOO", "RELATED TERM OF \"dog\""), ["3"]);
    t.run("COMMIT WORK").unwrap();
    assert_eq!(expanded(&a, "ZOO", "RELATED TERM OF \"dog\""), ["3", "7"]);
    t.run("INSERT INTO TERM_RELATED VALUES (3, 6, 'ZOO')").unwrap();
    assert_eq!(expanded(&t, "ZOO", "RELATED TERM OF \"dog\""), ["3", "6", "7"]);
    t.run("ROLLBACK WORK").unwrap();
    assert_eq!(expanded(&t, "ZOO", "RELATED TERM OF \"dog\""), ["3", "7"]);
    assert_eq!(expanded(&a, "ZOO", "RELATED TERM OF \"dog\""), ["3", "7"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2, §5.1.3, §5.3.1] an expansion
/// stands wherever a primary does: joined by `|`, in RANK and the Contains
/// method, and in a pattern given as a dynamic parameter.
#[test]
fn expansions_combine_and_are_given_by_parameter() {
    let db = Database::new();
    let (a, t) = zoo(&db);
    for sql in ZOO {
        t.run(sql).unwrap();
    }
    let either = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"zebra\" | THESAURUS \"ZOO\" EXPAND RELATED TERM OF \"dog\"') = 1 ORDER BY ID";
    assert_eq!(rows(&a, either).unwrap(), ["3", "7", "9"]);
    let ranked = "SELECT ID, RANK(BODY, 'THESAURUS \"ZOO\" EXPAND TOP TERM OF \"cat\"') FROM DOCS WHERE ID < 3 ORDER BY ID";
    assert_eq!(rows(&a, ranked).unwrap(), ["1,1E0", "2,0E0"]);
    let method = "SELECT ID FROM DOCS WHERE BODY.Contains('THESAURUS \"ZOO\" EXPAND SYNONYM TERM OF \"hound\"') = 1 ORDER BY ID";
    assert_eq!(rows(&a, method).unwrap(), ["5", "8"]);
    let p = a.prepare("SELECT ID FROM DOCS WHERE CONTAINS(BODY, ?) = 1 ORDER BY ID").unwrap();
    let pattern = Value::Char("THESAURUS \"ZOO\" EXPAND NARROWER TERM OF \"dog\"".into());
    let mut cursor = a.open_prepared(&p, &[pattern]).unwrap();
    let mut ids = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        ids.push(row[0].to_string());
    }
    assert_eq!(ids, ["3", "4"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, 3d, 4] an
/// expansion of a thesaurus of no name, of a mask or a stemmed word, or
/// FOR n LEVELS of a relation that counts none, is SQLSTATE 2FF01.
#[test]
fn malformed_expansions_are_2ff01() {
    let db = Database::new();
    let (a, _) = zoo(&db);
    for pattern in [
        "THESAURUS \"\" EXPAND RELATED TERM OF \"dog\"",
        "THESAURUS \"ZOO\" EXPAND RELATED TERM OF \"d%\"",
        "THESAURUS \"ZOO\" EXPAND RELATED TERM OF STEMMED FORM OF \"dogs\"",
        "THESAURUS \"ZOO\" EXPAND SYNONYM TERM OF \"dog\" FOR 2 LEVELS",
        "THESAURUS \"ZOO\" EXPAND TOP TERM OF \"dog\" FOR 1 LEVEL",
        "THESAURUS \"ZOO\" EXPAND COUSIN TERM OF \"dog\"",
        "THESAURUS \"ZOO\" EXPAND BROADER TERM OF \"dog\" FOR 1",
    ] {
        let sql = format!("SELECT CONTAINS(BODY, '{pattern}') FROM DOCS");
        assert_eq!(state(&a, &sql), SqlState::INVALID_SEARCH_EXPRESSION.as_str(), "{pattern}");
    }
}
