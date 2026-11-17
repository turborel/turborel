//! The FullText methods written as methods [ISO/IEC 13249-2:2002 §5.1]:
//! `BODY.Contains(p)`, `BODY.Rank(p)`, the LANGUAGE observer, qualified by
//! table or correlation name, and NEW FullText(...); CONTAINS and RANK of
//! character string documents; and patterns given as dynamic parameters.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::value::Value;

use super::documents;
use crate::common::{rows, state};

/// The rows the prepared `sql` gives of `values`, as `rows` gives them.
fn prepared(a: &Session, sql: &str, values: &[Value]) -> Vec<String> {
    let p = a.prepare(sql).unwrap();
    let mut cursor = a.open_prepared(&p, values).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    out
}

/// A character string value of `text`.
fn text(text: &str) -> Value {
    Value::Char(text.into())
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.2, §5.1.3] Contains, Rank
/// and the LANGUAGE observer invoked as methods of a FULLTEXT column, bare
/// or qualified by its table's or a correlation name, give what CONTAINS
/// and RANK give; of a null document, null.
#[test]
fn methods_of_a_fulltext_column_5_1_2() {
    let db = Database::new();
    let a = documents(&db, &["FULLTEXT('The quick brown fox')", "FULLTEXT('Lazy dogs', 'NONE')", "NULL"]);
    let bare = "SELECT ID, BODY.Contains('\"fox\"'), BODY.Rank('\"fox\"'), BODY.\"LANGUAGE\" FROM DOCS ORDER BY ID";
    assert_eq!(rows(&a, bare).unwrap(), ["1,1,1E0,ENGLISH", "2,0,0E0,NONE", "3,NULL,NULL,NULL"]);
    let qualified = "SELECT ID, DOCS.BODY.Contains('\"fox\"'), DOCS.BODY.Rank('\"fox\"') FROM DOCS ORDER BY ID";
    assert_eq!(rows(&a, qualified).unwrap(), ["1,1,1E0", "2,0,0E0", "3,NULL,NULL"]);
    let correlated = "SELECT D.ID FROM DOCS D WHERE D.BODY.Contains('\"lazy\"') = 1";
    assert_eq!(rows(&a, correlated).unwrap(), ["2"]);
    let same = "SELECT COUNT(*) FROM DOCS WHERE BODY.Contains('\"lazy\"') = CONTAINS(BODY, '\"lazy\"')";
    assert_eq!(rows(&a, same).unwrap(), ["2"]);
    let invalid = state(&a, "SELECT BODY.Contains('\"fox\" &') FROM DOCS");
    assert_eq!(invalid, SqlState::INVALID_SEARCH_EXPRESSION.as_str());
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.9] NEW FullText(string) and NEW
/// FullText(string, language) make the value FULLTEXT(...) makes, whose
/// methods may be invoked at once; an unknown language is SQLSTATE 2FF02.
#[test]
fn new_fulltext_5_1_9() {
    let db = Database::new();
    let a = documents(&db, &["NEW FullText('Hello world')", "NEW FULLTEXT('Hello worlds', 'NONE')"]);
    let made = "SELECT ID, FULLTEXT_TO_CHARACTER(BODY), BODY.\"LANGUAGE\" FROM DOCS ORDER BY ID";
    assert_eq!(rows(&a, made).unwrap(), ["1,Hello world,ENGLISH", "2,Hello worlds,NONE"]);
    let equal = "SELECT ID FROM DOCS WHERE BODY = FULLTEXT('Hello world') ORDER BY ID";
    assert_eq!(rows(&a, equal).unwrap(), ["1"]);
    let at_once = "SELECT (NEW FullText('Hello world')).Contains('\"world\"') FROM DOCS WHERE ID = 1";
    assert_eq!(rows(&a, at_once).unwrap(), ["1"]);
    let refused = a.run("INSERT INTO DOCS VALUES (3, NEW FullText('x', 'KLINGON'))").unwrap_err();
    assert_eq!(refused.sqlstate(), SqlState::INVALID_LANGUAGE_SPECIFICATION);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM DOCS").unwrap(), ["2"]);
}

/// Verifies: [EXTENSION] (RULES.md D87) CONTAINS and RANK search a
/// VARCHAR or CLOB document as a FULLTEXT one of the default language;
/// a null document gives null; a character string column has no methods,
/// SQLCODE -103.
#[test]
fn character_string_documents_are_searched() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE V (ID INTEGER, T VARCHAR(100), C CLOB(1000))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO V VALUES (1, 'red apples and pears', 'green apple pie'), (2, NULL, 'Red sky')").unwrap();
    let searched = "SELECT ID, CONTAINS(T, '\"apples\"'), CONTAINS(C, 'STEMMED FORM OF \"apples\"'), RANK(T, '\"red\"'), \
        RANK(C, '\"red\"') FROM V ORDER BY ID";
    assert_eq!(rows(&a, searched).unwrap(), ["1,1,1,1E0,0E0", "2,NULL,0,NULL,1E0"]);
    assert_eq!(rows(&a, "SELECT ID FROM V WHERE CONTAINS(C, '\"red\" & \"sky\"') = 1").unwrap(), ["2"]);
    assert_eq!(rows(&a, "SELECT T.Contains('\"apples\"') FROM V"), Err(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 9075:1992 §17.6, §17.9] [ISO/IEC 13249-2:2002
/// §5.1.2, §5.1.3] a pattern given as a dynamic parameter is read at each
/// execution, so CONTAINS, the Contains method and RANK of one prepared
/// statement find what each value asks; a null pattern gives null, and
/// one not of the grammar is SQLSTATE 2FF01 when executed.
#[test]
fn prepared_patterns_are_read_each_execution() {
    let db = Database::new();
    let a = documents(&db, &["FULLTEXT('The quick brown fox')", "FULLTEXT('Lazy dogs')"]);
    let search = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, ?) = 1 ORDER BY ID";
    assert_eq!(prepared(&a, search, &[text("\"fox\"")]), ["1"]);
    assert_eq!(prepared(&a, search, &[text("\"dogs\" | \"quick\"")]), ["1", "2"]);
    assert_eq!(prepared(&a, search, &[Value::Null]), Vec::<String>::new());
    let method = "SELECT ID, BODY.Contains(?), RANK(BODY, ?) FROM DOCS ORDER BY ID";
    let both = [text("STEMMED FORM OF \"dog\""), text("\"lazy\"")];
    assert_eq!(prepared(&a, method, &both), ["1,0,0E0", "2,1,1E0"]);
    assert_eq!(prepared(&a, method, &[Value::Null, Value::Null]), ["1,NULL,NULL", "2,NULL,NULL"]);
    let p = a.prepare(search).unwrap();
    let failed = a.open_prepared(&p, &[text("\"fox\" &")]).and_then(|mut c| {
        while c.fetch()?.is_some() {}
        Ok(())
    });
    assert_eq!(failed.unwrap_err().sqlstate(), SqlState::INVALID_SEARCH_EXPRESSION);
}
