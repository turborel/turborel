//! \[EXTENSION\] Languages of full-text search an extension serves
//! (RULES.md D328), served by a real process of the example plug-in:
//! TOYZH cuts Chinese by its dictionary, TOYSTEM stems, each named where
//! a built-in language is.

use turborel::db::Session;
use turborel::error::Code;

use super::{calls, kinds, state};
use crate::common::{exec, rows};

/// The languages, as the manifest declares them. A language is named in
/// the whole process, so each test declares its own.
const LANGUAGES: &str = "LANGUAGE TOYZH SERVES (SEGMENT, STOP) LIKE CHINESE BATCH 64;";

/// The IDs of ZH whose BODY holds `pattern`.
fn found(s: &Session, pattern: &str) -> Vec<String> {
    rows(s, &format!("SELECT ID FROM ZH WHERE CONTAINS(BODY, '{pattern}') = 1 ORDER BY ID")).unwrap()
}

/// Verifies: [EXTENSION] RULES.md D328: a column indexed in a served
/// language is cut by its plug-in, a document a call, its pattern words
/// alike: a dictionary word is one word, so a word inside it is not found
/// where character bigrams would find it; its stop word is the plug-in's;
/// TEXT_MATCHES reads the words in the index's language; a FULLTEXT value
/// and a pattern's language specification name it as a built-in one;
/// DROP EXTENSION is refused while the index names it, and once it is
/// dropped the name is no language.
#[test]
fn a_served_language_cuts_documents_and_patterns() {
    let (_scratch, db) = kinds("plugin-kinds-language", LANGUAGES);
    let s = db.session("U");
    s.run("CREATE TABLE ZH (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(200))").unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX ZH_BODY ON ZH (BODY) USING fulltext WITH (FULLTEXT.LANGUAGE = TOYZH)").unwrap();
    s.commit().unwrap();
    let before = calls(&s);
    s.run("INSERT INTO ZH VALUES (1, '我们喜欢数据库'), (2, '全文搜索引擎'), (3, '中文分词的语言'), (4, '数据')").unwrap();
    s.commit().unwrap();
    let made = calls(&s) - before;
    assert!(made <= 4, "a document a call at most: {made}");
    assert_eq!(found(&s, "\"数据库\""), ["1"]);
    assert_eq!(found(&s, "\"数据\""), ["4"], "数据库 is one word");
    assert!(found(&s, "\"据库\"").is_empty(), "no bigram inside a word");
    assert_eq!(found(&s, "\"中文\" & \"语言\""), ["3"]);
    assert_eq!(found(&s, "\"全文搜索\""), ["2"], "a pattern word cut by the plug-in");
    let matched = "SELECT ID FROM ZH WHERE TEXT_MATCHES(BODY, '数据库 我们') ORDER BY ID";
    assert_eq!(rows(&s, matched).unwrap(), ["1"]);
    assert_eq!(rows(&s, "SELECT ID FROM ZH WHERE TEXT_MATCHES(BODY, '数据') ORDER BY ID").unwrap(), ["4"]);
    assert_eq!(state(&s, "VALUES (CONTAINS(FULLTEXT('的', 'TOYZH'), '\"的\"'))"), "2FF03", "the plug-in's stop word alone");
    assert_eq!(rows(&s, "VALUES (CONTAINS(FULLTEXT('的', 'CHINESE'), '\"的\"'))").unwrap(), ["1"], "no stop word of CHINESE");
    let spec = "VALUES (CONTAINS(FULLTEXT('我们喜欢数据库', 'ENGLISH'), 'TOYZH \"数据库\"'))";
    assert_eq!(rows(&s, spec).unwrap(), ["0"], "the pattern's word is one word, the document's ideographs are each one");
    assert_eq!(rows(&s, "VALUES (CONTAINS(FULLTEXT('我们喜欢数据库', 'TOYZH'), 'TOYZH \"喜欢\"'))").unwrap(), ["1"]);
    s.commit().unwrap();
    assert_eq!(exec(&s, "DROP EXTENSION kinds RESTRICT"), Err(Code::SyntaxRule), "ZH_BODY names TOYZH");
    s.rollback().unwrap();
    s.run("DROP INDEX ZH_BODY").unwrap();
    s.commit().unwrap();
    s.run("DROP EXTENSION kinds RESTRICT").unwrap();
    s.commit().unwrap();
    assert_eq!(state(&s, "VALUES (CONTAINS(FULLTEXT('数据', 'TOYZH'), '\"数据\"'))"), "2FF02", "no extension serves it now");
}

/// Verifies: [EXTENSION] RULES.md D328: a language serving stems alone
/// is cut and its stop words taken as its base, NONE, and stems a batch
/// of words a call; a name no language has is 2FF02.
#[test]
fn a_served_language_may_serve_stems_alone() {
    let (_scratch, db) = kinds("plugin-kinds-language-stems", "LANGUAGE TOYSTEM SERVES (STEM);");
    let s = db.session("U");
    let walked = "VALUES (CONTAINS(FULLTEXT('Walking dogs, walked cats', 'TOYSTEM'), 'FORM OF \"walks\" & FORM OF \"dog\"'))";
    assert_eq!(rows(&s, walked).unwrap(), ["1"]);
    assert_eq!(rows(&s, "VALUES (CONTAINS(FULLTEXT('the dogs', 'TOYSTEM'), '\"the\"'))").unwrap(), ["1"], "NONE stops no word");
    assert_eq!(state(&s, "VALUES (CONTAINS(FULLTEXT('dogs', 'TOYNONE'), '\"dogs\"'))"), "2FF02");
}

/// Verifies: [EXTENSION] RULES.md D328, D170: a language whose process
/// crashes fails the statement with 38T01, and the next call starts
/// another process.
#[test]
fn a_served_language_crashing_is_restarted() {
    let (_scratch, db) = kinds("plugin-kinds-language-crash", "LANGUAGE CRASHZH SERVES (SEGMENT);");
    let s = db.session("U");
    assert_eq!(state(&s, "VALUES (CONTAINS(FULLTEXT('崩溃', 'CRASHZH'), '\"数据\"'))"), "38T01");
    assert_eq!(rows(&s, "VALUES (CONTAINS(FULLTEXT('数据', 'CRASHZH'), '\"数据\"'))").unwrap(), ["1"]);
}
