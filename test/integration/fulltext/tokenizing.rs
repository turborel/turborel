//! The FullText methods that tokenize [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §5.1.4-§5.1.8] through SQL: called as methods of a FULLTEXT
//! column, their arrays read by CARDINALITY, element reference and
//! UNNEST; their nulls and refused calls.

use turborel::db::{Database, Session};
use turborel::error::Code;

use super::documents;
use crate::common::rows;

/// DOCS of three rows: an English sentence pair, a value in NONE, NULL.
fn three(db: &Database) -> Session {
    documents(
        db,
        &["FULLTEXT('The quick fox jumps. The dogs sleep!\u{2029}A new day.', 'ENGLISH')", "FULLTEXT('The foxes', 'NONE')", "NULL"],
    )
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.4, §5.1.7] BODY.Tokenize() and
/// BODY.TokenizeAndStem() give arrays of the normalized words, stop words
/// left out, and of their stems; CARDINALITY counts them; a null BODY
/// gives null.
#[test]
fn tokenize_and_stem_as_methods_5_1_4() {
    let db = Database::new();
    let a = three(&db);
    let words = "SELECT ID, CARDINALITY(BODY.Tokenize()), BODY.Tokenize()[1], BODY.TokenizeAndStem()[2] FROM DOCS ORDER BY ID";
    assert_eq!(rows(&a, words).unwrap(), ["1,7,quick,fox", "2,2,the,foxes", "3,NULL,NULL,NULL"]);
    let all = "SELECT W, N FROM DOCS, UNNEST(DOCS.BODY.TokenizeAndStem()) WITH ORDINALITY AS U (W, N) WHERE ID = 1 ORDER BY N";
    assert_eq!(rows(&a, all).unwrap(), ["quick,1", "fox,2", "jump,3", "dog,4", "sleep,5", "new,6", "day,7"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5, §5.1.8, §5.2.1]
/// BODY.TokenizePosition(unit) places each word in WORDS, SENTENCES,
/// PARAGRAPHS or CHARACTERS, its stop words counted; its rows' fields are
/// read by name, of an element or as UNNEST's columns;
/// BODY.TokenizePositionAndStem() gives stems at their places in words.
#[test]
fn token_positions_as_methods_5_1_5() {
    let db = Database::new();
    let a = three(&db);
    let place = |unit: &str, n: u32| {
        let q = format!(
            "SELECT T, P, C FROM DOCS, UNNEST(DOCS.BODY.TokenizePosition('{unit}')) WITH ORDINALITY AS U (T, P, C, N) WHERE ID = 1 AND N = {n}"
        );
        rows(&a, &q).unwrap()
    };
    assert_eq!(place("WORDS", 1), ["quick,2,0"]);
    assert_eq!(place("WORDS", 4), ["dogs,6,0"]);
    assert_eq!(place("SENTENCES", 4), ["dogs,2,0"]);
    assert_eq!(place("PARAGRAPH", 7), ["day,2,0"]);
    assert_eq!(place("CHARACTERS", 2), ["fox,11,3"]);
    let stems = "SELECT U.\"TOKEN\", U.\"POSITION\" FROM DOCS, UNNEST(DOCS.BODY.TokenizePositionAndStem()) AS U WHERE ID = 1";
    assert_eq!(rows(&a, stems).unwrap()[..3], ["quick,2", "fox,3", "jump,4"]);
    let element = "SELECT BODY.TokenizePosition('WORDS')[2].\"TOKEN\", BODY.TokenizePosition('WORDS')[2].CORRVAL FROM DOCS WHERE ID = 1";
    assert_eq!(rows(&a, element).unwrap(), ["fox,0"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.6] BODY.Segmentize('SENTENCE')
/// and BODY.Segmentize('PARAGRAPH') give FullText values of the sentences
/// and paragraphs, in order and in BODY's language.
#[test]
fn segmentize_as_a_method_5_1_6() {
    let db = Database::new();
    let a = three(&db);
    let sentences = "SELECT FULLTEXT_TO_CHARACTER(S), S.\"LANGUAGE\" FROM DOCS, UNNEST(DOCS.BODY.Segmentize('SENTENCE')) WITH ORDINALITY AS U (S, N) WHERE ID = 1 ORDER BY N";
    assert_eq!(rows(&a, sentences).unwrap(), ["The quick fox jumps.,ENGLISH", "The dogs sleep!,ENGLISH", "A new day.,ENGLISH"]);
    let paragraphs =
        "SELECT CARDINALITY(BODY.Segmentize('PARAGRAPH')), FULLTEXT_TO_CHARACTER(BODY.Segmentize('PARAGRAPH')[1]) FROM DOCS ORDER BY ID";
    assert_eq!(rows(&a, paragraphs).unwrap(), ["2,The quick fox jumps. The dogs sleep!", "1,The foxes", "NULL,NULL"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 Description 5, §5.1.6
/// Description 2-3] a unit not taken or null gives null, an empty value an
/// empty array; a method not of a FullText value, or of a unit it does not
/// take, is refused.
#[test]
fn units_nulls_and_refusals_5_1_5() {
    let db = Database::new();
    let a = three(&db);
    let q = "SELECT CARDINALITY(BODY.TokenizePosition('WORD')), CARDINALITY(BODY.Segmentize('SENTENCES')), CARDINALITY(BODY.TokenizePosition(CAST(NULL AS VARCHAR(5)))) FROM DOCS WHERE ID = 1";
    assert_eq!(rows(&a, q).unwrap(), ["NULL,NULL,NULL"]);
    let empty = "SELECT CARDINALITY(TOKENIZE(FULLTEXT(''))), CARDINALITY(SEGMENTIZE(FULLTEXT(''), 'SENTENCE')) FROM DOCS WHERE ID = 1";
    assert_eq!(rows(&a, empty).unwrap(), ["0,0"]);
    assert_eq!(rows(&a, "SELECT TOKENIZE('plain') FROM DOCS").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT BODY.Tokenize('WORDS') FROM DOCS").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT BODY.TokenizePosition() FROM DOCS").unwrap_err(), Code::SyntaxRule);
}
