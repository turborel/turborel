//! The options of a full-text index (RULES.md D41, D86): listed, checked,
//! changed in place or by building the index again, and kept as it is
//! built again; and the columns it refuses.

use turborel::db::Database;
use turborel::error::Code;

use super::{TEXTS, documents, found, probed};
use crate::common::{exec, rows};

/// The options of DOCS_BODY, as INDEX_OPTIONS lists them: name, value,
/// source.
fn listed(db: &Database) -> Vec<String> {
    let sql = "SELECT OPTION_NAME, OPTION_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
               WHERE INDEX_NAME = 'DOCS_BODY' ORDER BY OPTION_NAME";
    rows(&db.session("S"), sql).unwrap().into_iter().map(|r| r.replace(' ', "")).collect()
}

/// Verifies: [EXTENSION] RULES.md D41, D86: a full-text index has the
/// options FULLTEXT.B, FULLTEXT.K1, FULLTEXT.LANGUAGE and
/// FULLTEXT.STOP_WORDS, of their defaults unless WITH gives them; ALTER
/// INDEX changes them; a value out of range, a language not known or an
/// option of another method is refused (SQLCODE -103).
#[test]
fn its_options_are_listed_set_and_checked() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    let defaults = [
        "FULLTEXT.B,7.5E-1,DEFAULT",
        "FULLTEXT.K1,1.2E0,DEFAULT",
        "FULLTEXT.LANGUAGE,ENGLISH,DEFAULT",
        "FULLTEXT.STOP_WORDS,DEFAULT,DEFAULT",
    ];
    assert_eq!(listed(&db), defaults);
    exec(&s, "ALTER INDEX DOCS_BODY SET (FULLTEXT.K1 = 1.5, FULLTEXT.B = 0.5)").unwrap();
    let changed = listed(&db);
    assert!(
        changed.contains(&"FULLTEXT.K1,1.5E0,INDEX".to_owned()) && changed.contains(&"FULLTEXT.B,5E-1,INDEX".to_owned()),
        "{changed:?}"
    );
    let refused = [
        "ALTER INDEX DOCS_BODY SET (FULLTEXT.B = 2)",
        "ALTER INDEX DOCS_BODY SET (FULLTEXT.K1 = -1)",
        "ALTER INDEX DOCS_BODY SET (FULLTEXT.LANGUAGE = KLINGON)",
        "CREATE INDEX DOCS_ID ON DOCS (BODY) USING btree WITH (FULLTEXT.K1 = 1)",
    ];
    for sql in refused {
        assert_eq!(exec(&s, sql).err(), Some(Code::SyntaxRule), "{sql}");
    }
    assert_eq!(found(&s, "lazy"), ["1", "2"]);
}

/// Verifies: [EXTENSION] RULES.md D41, D86: FULLTEXT.STOP_WORDS and
/// FULLTEXT.LANGUAGE build the index again, as its entries follow them: a
/// list of the user's, NONE and the language NONE each give the rows a
/// scan gives, whatever the index posts; a word the index takes for a
/// stop word it holds no places of, so a search of it reads the table.
#[test]
fn stop_words_and_language_build_it_again() {
    let db = Database::new();
    let s = documents(&db, &TEXTS);
    for setting in ["'lazy, garden'", "NONE", "DEFAULT"] {
        exec(&s, "COMMIT WORK").unwrap();
        exec(&s, &format!("ALTER INDEX DOCS_BODY SET (FULLTEXT.STOP_WORDS = {setting})")).unwrap();
        assert!(probed(&s, "foxes"), "{setting}");
        assert_eq!(probed(&s, "garden"), !setting.contains("garden"), "{setting}");
        assert_eq!(found(&s, "lazy garden"), ["2"], "{setting}");
        assert_eq!(found(&s, "the"), ["1", "2", "3", "4", "5"], "{setting}");
    }
    exec(&s, "COMMIT WORK").unwrap();
    exec(&s, "ALTER INDEX DOCS_BODY SET (FULLTEXT.LANGUAGE = NONE)").unwrap();
    assert_eq!(found(&s, "foxes"), ["3", "5"]);
    let listed = listed(&db);
    assert!(listed.contains(&"FULLTEXT.LANGUAGE,NONE,INDEX".to_owned()), "{listed:?}");
}

/// Verifies: [EXTENSION] RULES.md D86: a full-text index is of one
/// character string column, not of a number, two columns, or a collated
/// column, whose key is its collation's, not its text (SQLCODE -103 or
/// -900).
#[test]
fn it_indexes_one_character_string_column() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (N INTEGER, A VARCHAR(10), B VARCHAR(10) COLLATE CASE_INSENSITIVE)").unwrap();
    let s = db.session("S");
    for sql in
        ["CREATE INDEX X ON T (N) USING fulltext", "CREATE INDEX X ON T (A, A) USING fulltext", "CREATE INDEX X ON T (B) USING fulltext"]
    {
        let code = exec(&s, sql).err();
        assert!(matches!(code, Some(Code::SyntaxRule | Code::Unsupported)), "{sql}: {code:?}");
    }
    exec(&s, "CREATE INDEX X ON T (A) USING fulltext").unwrap();
}
