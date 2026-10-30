//! Direct SQL statements (`Session::direct`), as a server's clients send
//! them: a query gives its rows, any other statement its completion.

use turborel::db::{Completion, Database, Direct};
use turborel::error::Code;

/// The rows of `direct`'s cursor, or its completion's SQLCODE.
fn direct(s: &turborel::db::Session, sql: &str) -> Result<Result<Vec<String>, i32>, Code> {
    match s.direct(sql).map_err(|e| e.code)? {
        Direct::Rows(mut c) => {
            let mut out = Vec::new();
            while let Some(r) = c.fetch().map_err(|e| e.code)? {
                out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
            }
            Ok(Ok(out))
        }
        Direct::Done(done) => Ok(Err(done.sqlcode())),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1, §20.2]: a `<direct
/// select statement: multiple rows>`, begun by SELECT, VALUES, TABLE or a
/// parenthesis, with an ORDER BY or not, gives its rows; other direct
/// statements, data, schema and transaction statements, their completion,
/// SQLCODE 100 for a searched DELETE that finds no row; a statement that
/// does not parse is refused, as `run` refuses it.
#[test]
fn a_query_gives_rows_and_any_other_statement_its_completion() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("X");
    assert_eq!(direct(&s, "INSERT INTO T VALUES (2)"), Ok(Err(0)));
    assert_eq!(direct(&s, "INSERT INTO T VALUES (1)"), Ok(Err(0)));
    assert_eq!(direct(&s, "SELECT K FROM T ORDER BY K"), Ok(Ok(vec!["1".into(), "2".into()])));
    assert_eq!(direct(&s, "(SELECT K FROM T WHERE K = 2)"), Ok(Ok(vec!["2".into()])));
    assert_eq!(direct(&s, "VALUES (7)"), Ok(Ok(vec!["7".into()])));
    assert_eq!(direct(&s, "TABLE T ORDER BY K DESC"), Ok(Ok(vec!["2".into(), "1".into()])));
    assert_eq!(direct(&s, "DELETE FROM T WHERE K = 9"), Ok(Err(Completion::NoData.sqlcode())));
    assert_eq!(direct(&s, "COMMIT"), Ok(Err(0)));
    assert_eq!(direct(&s, "SELECT K FROM T WHERE K = 9"), Ok(Ok(vec![])));
    assert!(direct(&s, "SELEKT 1").is_err());
    assert!(direct(&s, "").is_err());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1, §11.1 AR1]; [DESIGN]: a
/// `<schema definition>` is a direct statement; through a session, whose
/// own its schema must be: another's is refused (SQLCODE -104), nothing of
/// it created; a schema that exists already is refused as
/// `Database::schema` refuses it.
#[test]
fn a_session_creates_its_own_schema_only() {
    let db = Database::new();
    let s = db.session("X");
    assert_eq!(direct(&s, "CREATE SCHEMA AUTHORIZATION Y CREATE TABLE T (K INTEGER)"), Err(Code::Privilege));
    assert_eq!(crate::common::rows(&db.session("Y"), "SELECT K FROM Y.T"), Err(Code::SyntaxRule), "not created");
    assert_eq!(direct(&s, "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)"), Ok(Err(0)));
    assert_eq!(direct(&s, "INSERT INTO T VALUES (1)"), Ok(Err(0)));
    assert_eq!(direct(&s, "SELECT K FROM T"), Ok(Ok(vec!["1".into()])));
    assert_eq!(direct(&s, "CREATE SCHEMA AUTHORIZATION X"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §20.1]: direct SQL declares no
/// cursor: DECLARE CURSOR, not a `<directly executable statement>`, is
/// refused (SQLCODE -102), so OPEN, FETCH, CLOSE and a positioned UPDATE or
/// DELETE name no cursor declared; a query's rows are read by a cursor of
/// its own, fetched a row at a time.
#[test]
fn direct_sql_declares_no_cursor_but_reads_a_query_by_one() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("X");
    s.run("INSERT INTO T VALUES (1), (2)").unwrap();
    assert_eq!(direct(&s, "DECLARE C CURSOR FOR SELECT K FROM T"), Err(Code::Syntax));
    for named in ["OPEN C", "FETCH C INTO P", "CLOSE C", "UPDATE T SET K = 3 WHERE CURRENT OF C", "DELETE FROM T WHERE CURRENT OF C"] {
        assert_eq!(direct(&s, named), Err(Code::SyntaxRule), "{named}");
    }
    let Direct::Rows(mut rows) = s.direct("SELECT K FROM T ORDER BY K").unwrap() else { panic!("a query gives rows") };
    assert_eq!(rows.fetch().unwrap().map(|r| r[0].to_string()), Some("1".into()));
    assert_eq!(rows.fetch().unwrap().map(|r| r[0].to_string()), Some("2".into()));
    assert!(rows.fetch().unwrap().is_none(), "after the last row");
}
