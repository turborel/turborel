//! Each form of search pattern [ISO/IEC 13249-2:2002 §5.3.1] through SQL,
//! over a table with no full-text index and again with one. The index
//! changes how the rows are found, never which.

use turborel::db::{Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;

use super::documents;
use crate::common::{rows, state};

/// The documents searched: two sentences; names alike in sound; stemmed
/// forms; a connector and two paragraphs, apart by U+2029; a word a mask without its
/// escape also matches; and a null.
const BODIES: [&str; 6] = [
    "FULLTEXT('The quick brown fox jumps. Over the lazy dog.')",
    "FULLTEXT('Smith and Smyth went home')",
    "FULLTEXT('Jumping foxes are quick')",
    "FULLTEXT('A test_case for brown dogs\u{2029}A new paragraph about a fox')",
    "FULLTEXT('Nothing testacase here')",
    "NULL",
];

/// The first ID of the filler rows, which no pattern but a NOT finds.
const FILLER: u32 = 100;

/// Schema A's DOCS of `BODIES` and 2000 filler rows from ID `FILLER`, so a
/// lookup of an index costs less than a scan.
fn filled(db: &Database) -> Session {
    let a = documents(db, &BODIES);
    let filler: Vec<String> = (FILLER..FILLER + 2000).map(|k| format!("({k}, FULLTEXT('filler row number {k}'))")).collect();
    for chunk in filler.chunks(200) {
        a.run(&format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    a.run("COMMIT WORK").unwrap();
    a
}

/// The query of the documents CONTAINS finds `pattern` in.
fn search(pattern: &str) -> String {
    format!("SELECT ID FROM DOCS WHERE CONTAINS(BODY, '{}') = 1 ORDER BY ID", pattern.replace('\'', "''"))
}

/// The IDs below `FILLER` of the documents CONTAINS finds `pattern` in.
fn found(a: &Session, pattern: &str) -> Vec<String> {
    let ids = rows(a, &search(pattern)).unwrap();
    ids.into_iter().filter(|id| id.parse::<u32>().is_ok_and(|id| id < FILLER)).collect()
}

/// Checks that each of `cases`, a pattern and the IDs it finds, finds its
/// IDs, first with no index on DOCS and then with a full-text index.
fn indexed_alike(cases: &[(&str, &[&str])]) {
    let db = Database::new();
    let a = filled(&db);
    let check = |a: &Session, how: &str| {
        for (pattern, want) in cases {
            assert_eq!(found(a, pattern), *want, "{pattern} {how}");
        }
    };
    check(&a, "unindexed");
    a.run("COMMIT WORK").unwrap();
    a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    check(&a, "indexed");
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2] [EXTENSION] (RULES.md D87) a
/// search by CONTAINS(BODY, pattern) = 1 of a column with a full-text
/// index, among enough rows that a lookup costs less than a scan, reads
/// the index, written either way round.
#[test]
fn contains_reads_the_index() {
    let db = Database::new();
    let a = filled(&db);
    a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    for sql in [search("\"fox\""), "SELECT ID FROM DOCS WHERE 1 = BODY.Contains('\"fox\"')".to_owned()] {
        let plan = a.physical_plan(&sql, &Params::new()).unwrap();
        assert!(plan.contains("Scan A.DOCS using DOCS_BODY"), "{plan}");
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §6.4, §6.6,
/// §6.19, §6.20, §6.21] words, phrases (in their order only), masks with
/// `%` and `_`, a mask's ESCAPE, `&`, `|`, NOT and parentheses find the
/// same documents with and without a full-text index; a null document is
/// found by none, NOT included.
#[test]
fn words_phrases_masks_and_connectives_6_4() {
    indexed_alike(&[
        ("\"fox\"", &["1", "4"]),
        ("\"FOX\"", &["1", "4"]),
        ("\"brown fox\"", &["1"]),
        ("\"fox brown\"", &[]),
        ("\"jum%\"", &["1", "3"]),
        ("\"j_mps\"", &["1"]),
        ("\"test_case\"", &["4", "5"]),
        ("\"test!_case\" ESCAPE \"!\"", &["4"]),
        ("\"quick\" & \"brown\"", &["1"]),
        ("\"smith\" | \"foxes\"", &["2", "3"]),
        ("NOT \"fox\"", &["2", "3", "5"]),
        ("\"fox\" & NOT \"dog\"", &["4"]),
        ("(\"smith\" | \"quick\") & NOT \"brown\"", &["2", "3"]),
        ("NONE \"jumps\"", &["1"]),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §6.5, §6.7]
/// STEMMED FORM OF a word or phrase, and FORM OF, find each form of it,
/// with and without an index; the key words are of any case.
#[test]
fn stemmed_forms_6_5() {
    indexed_alike(&[
        ("STEMMED FORM OF \"fox\"", &["1", "3", "4"]),
        ("stemmed form of \"jumping\"", &["1", "3"]),
        ("FORM OF \"dog\"", &["1", "4"]),
        ("STEMMED FORM OF \"jumping foxes\"", &["3"]),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3f, §6.8, §6.22]
/// NEAR within a number of words (positions apart), characters, sentences
/// or paragraphs, IN ORDER or ANY ORDER, of words and of lists, with and
/// without an index; at the distance found, not one less.
#[test]
fn proximity_6_8() {
    indexed_alike(&[
        ("\"quick\" NEAR \"fox\" WITHIN 2 WORDS IN ORDER", &["1"]),
        ("\"quick\" NEAR \"fox\" WITHIN 1 WORDS IN ORDER", &[]),
        ("\"fox\" NEAR \"quick\" WITHIN 2 WORDS IN ORDER", &[]),
        ("\"fox\" NEAR \"quick\" WITHIN 2 WORDS ANY ORDER", &["1"]),
        ("(\"fox\", \"dog\") NEAR \"lazy\" WITHIN 5 WORDS ANY ORDER", &["1"]),
        ("\"fox\" NEAR \"quick\" WITHIN 10 CHARACTERS ANY ORDER", &["1"]),
        ("\"fox\" NEAR \"quick\" WITHIN 3 CHARACTERS ANY ORDER", &[]),
        ("\"fox\" NEAR \"lazy\" WITHIN 1 SENTENCES ANY ORDER", &["1"]),
        ("\"fox\" NEAR \"lazy\" WITHIN 0 SENTENCE ANY ORDER", &[]),
        ("\"dogs\" NEAR \"fox\" WITHIN 1 PARAGRAPHS ANY ORDER", &["4"]),
        ("\"dogs\" NEAR \"fox\" WITHIN 0 PARAGRAPH ANY ORDER", &[]),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §6.18] IN SAME
/// SENTENCE and IN SAME PARAGRAPH AS find words in one unit, with and
/// without an index.
#[test]
fn contexts_6_18() {
    indexed_alike(&[
        ("\"fox\" IN SAME SENTENCE AS \"quick\"", &["1"]),
        ("\"fox\" IN SAME SENTENCE AS \"lazy\"", &[]),
        ("\"fox\" IN SAME PARAGRAPH AS \"lazy\"", &["1"]),
        ("\"brown\" IN SAME PARAGRAPH AS \"fox\"", &["1"]),
        ("\"brown\" IN SAME SENTENCE AS \"fox\" AND \"quick\"", &["1"]),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.9, §6.10, §6.17] SOUNDS LIKE finds
/// words of the same sound, FUZZY FORM OF words spelled alike, and IS
/// ABOUT documents about a word, with and without an index.
#[test]
fn sounds_fuzzy_and_about_6_9() {
    indexed_alike(&[
        ("SOUNDS LIKE \"smith\"", &["2"]),
        ("SOUNDS LIKE \"smoth\"", &["2"]),
        ("FUZZY FORM OF \"quik\"", &["1", "3"]),
        ("FUZZY FORM OF \"qxyz\"", &[]),
        ("IS ABOUT \"fox\"", &["1", "4"]),
        ("IS ABOUT \"zebra\"", &[]),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §11] patterns
/// the grammar refuses are SQLSTATE 2FF01: a context in WORDS, a phrase
/// in a token list, a mask after SOUNDS LIKE or in a stemmed word, an
/// escape of two characters, NEAR with no order; a language unknown is
/// 2FF02.
#[test]
fn malformed_patterns_are_2ff01() {
    let db = Database::new();
    let a = documents(&db, &BODIES);
    let invalid = SqlState::INVALID_SEARCH_EXPRESSION.as_str();
    for pattern in [
        "\"fox\" IN SAME WORDS AS \"quick\"",
        "\"brown fox\" NEAR \"lazy\" WITHIN 3 WORDS ANY ORDER",
        "SOUNDS LIKE \"sm%\"",
        "STEMMED FORM OF \"jum%\"",
        "\"a\" ESCAPE \"!!\"",
        "\"fox\" NEAR \"dog\" WITHIN 3 WORDS",
        "\"fox\" NEAR \"dog\" WITHIN WORDS ANY ORDER",
    ] {
        let pattern = pattern.replace('\'', "''");
        assert_eq!(state(&a, &format!("SELECT CONTAINS(BODY, '{pattern}') FROM DOCS")), invalid, "{pattern}");
    }
    let unknown = state(&a, "SELECT CONTAINS(BODY, 'KLINGON \"fox\"') FROM DOCS");
    assert_eq!(unknown, SqlState::INVALID_LANGUAGE_SPECIFICATION.as_str());
}
