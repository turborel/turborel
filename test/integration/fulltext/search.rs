//! FULLTEXT values made, cast and observed, and CONTAINS and RANK, through
//! SQL; their nulls and errors.

use turborel::error::{Code, SqlState};

use turborel::db::Database;

use super::documents;
use crate::common::{rows, state};

/// The rows of `bodies` CONTAINS finds `pattern` in, by ID.
fn found(a: &turborel::db::Session, pattern: &str) -> Vec<String> {
    let pattern = pattern.replace('\'', "''");
    rows(a, &format!("SELECT ID FROM DOCS WHERE CONTAINS(BODY, '{pattern}') = 1 ORDER BY ID")).unwrap()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.9, §5.1.10] a FULLTEXT
/// column is filled by FULLTEXT(string, language), FULLTEXT(string) and
/// CAST from a character string, in ENGLISH; FULLTEXT_TO_CHARACTER, CAST
/// to VARCHAR and LANGUAGE observe it; each gives null of null.
#[test]
fn fulltext_values_are_made_cast_and_observed() {
    let db = Database::new();
    let a = documents(&db, &["FULLTEXT('The quick brown fox', 'english')", "FULLTEXT('Lazy dogs')", "CAST('A cat' AS FULLTEXT)", "NULL"]);
    a.run("INSERT INTO DOCS VALUES (5, FULLTEXT('Nothing stemmed', 'NONE'))").unwrap();
    assert_eq!(
        rows(&a, "SELECT ID, FULLTEXT_TO_CHARACTER(BODY), CAST(BODY AS VARCHAR(40)), BODY.\"LANGUAGE\" FROM DOCS ORDER BY ID").unwrap(),
        [
            "1,The quick brown fox,The quick brown fox,ENGLISH",
            "2,Lazy dogs,Lazy dogs,ENGLISH",
            "3,A cat,A cat,ENGLISH",
            "4,NULL,NULL,NULL",
            "5,Nothing stemmed,Nothing stemmed,NONE",
        ]
    );
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM DOCS WHERE BODY = FULLTEXT('A cat', 'ENGLISH')").unwrap(), ["1"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM DOCS WHERE BODY = FULLTEXT('A cat', 'NONE')").unwrap(), ["0"]);
    assert_eq!(rows(&a, "SELECT FULLTEXT_TO_CHARACTER(FULLTEXT(CAST(NULL AS VARCHAR(3)))) FROM DOCS WHERE ID = 1").unwrap(), ["NULL"]);
    assert_eq!(a.run("INSERT INTO DOCS VALUES (6, 'plain text')").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2] CONTAINS(BODY, pattern) = 1
/// finds the documents holding a word, a conjunction, a disjunction or a
/// stemmed word; a null document gives null, not 1 or 0; FT_PATTERN
/// values and character strings are patterns alike.
#[test]
fn contains_finds_documents() {
    let db = Database::new();
    let a =
        documents(&db, &["FULLTEXT('The quick brown fox jumps')", "FULLTEXT('A lazy dog sleeps')", "FULLTEXT('Foxes and dogs')", "NULL"]);
    assert_eq!(found(&a, "\"fox\""), ["1"]);
    assert_eq!(found(&a, "\"dog\" & \"lazy\""), ["2"]);
    assert_eq!(found(&a, "\"quick\" | \"sleeps\""), ["1", "2"]);
    assert_eq!(found(&a, "STEMMED FORM OF \"fox\""), ["1", "3"]);
    assert_eq!(rows(&a, "SELECT ID, CONTAINS(BODY, '\"fox\"') FROM DOCS ORDER BY ID").unwrap(), ["1,1", "2,0", "3,0", "4,NULL"]);
    assert_eq!(rows(&a, "SELECT CONTAINS(BODY, CAST(NULL AS VARCHAR(5))) FROM DOCS WHERE ID = 1").unwrap(), ["NULL"]);
    let typed = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, CAST('\"jumps\"' AS FT_PATTERN)) = 1";
    assert_eq!(rows(&a, typed).unwrap(), ["1"]);
    assert_eq!(rows(&a, "SELECT RANK(BODY, '\"fox\"') FROM DOCS ORDER BY ID").unwrap(), ["1E0", "0E0", "0E0", "NULL"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §11] the full-text exceptions and
/// their SQLSTATEs: 2FF01 of a pattern that is no `<search expression>`,
/// 2FF02 of an unknown, empty or null language, 2FF03 of a pattern with
/// nothing to look for; and the statement leaves no effect.
#[test]
fn full_text_exceptions_have_their_sqlstates() {
    let db = Database::new();
    let a = documents(&db, &["FULLTEXT('The quick brown fox')"]);
    let query = |sql: &str| state(&a, sql);
    let invalid = SqlState::INVALID_SEARCH_EXPRESSION.as_str();
    assert_eq!(query("SELECT CONTAINS(BODY, '\"fox\" &') FROM DOCS"), invalid);
    assert_eq!(query("SELECT CONTAINS(BODY, 'fox') FROM DOCS"), invalid);
    assert_eq!(query("SELECT RANK(BODY, '(\"fox\"') FROM DOCS"), invalid);
    for language in ["'KLINGON'", "''", "CAST(NULL AS VARCHAR(5))"] {
        let refused = a.run(&format!("INSERT INTO DOCS VALUES (2, FULLTEXT('x', {language}))")).unwrap_err();
        assert_eq!(refused.sqlstate(), SqlState::INVALID_LANGUAGE_SPECIFICATION);
    }
    assert_eq!(query("SELECT CONTAINS(BODY, '\"the\"') FROM DOCS"), SqlState::EFFECTIVELY_EMPTY_SEARCH_EXPRESSION.as_str());
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM DOCS").unwrap(), ["1"]);
}
