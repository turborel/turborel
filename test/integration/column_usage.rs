//! The columns the views of usage show of what routines, triggers and
//! modules reference, each as its base table's Description 1 counts them
//! [ISO/IEC 9075-11:2006 draft §6.35, §6.54; ISO/IEC 9075-4:2006 draft
//! §19.1]: an INSERT's column list's and an UPDATE's object columns among
//! them; and the triggered action column set they are of, which DROP
//! COLUMN RESTRICT reads [ISO/IEC 9075-2:2023 §11.49 GR5i, §11.23 SR7].

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{exec, rows};

/// Schema A: tables T (K, N, G) and LOG (WHO, WHAT); procedure BUMP, an
/// UPDATE of T's N from a subquery of its K where G > 1; procedure FILL,
/// an INSERT of LOG's WHAT from T's G; function TOP, a query of T's N
/// alone; trigger TR on UPDATE of T's N, WHEN its old K > 0, inserting
/// into LOG's WHO T's G where N is the old N; module SHOP, whose RECORD
/// inserts into LOG's WHAT T's G.
fn referenced() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER, G INTEGER) CREATE TABLE LOG (WHO INTEGER, WHAT INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE PROCEDURE BUMP () MODIFIES SQL DATA UPDATE T SET N = (SELECT MAX(K) FROM T) WHERE G > 1",
        "CREATE PROCEDURE FILL () MODIFIES SQL DATA INSERT INTO LOG (WHAT) SELECT G FROM T",
        "CREATE FUNCTION TOP () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(N) FROM T)",
        "CREATE TRIGGER TR AFTER UPDATE OF N ON T REFERENCING OLD ROW AS O FOR EACH ROW WHEN (O.K > 0) \
         INSERT INTO LOG (WHO) SELECT G FROM T WHERE N = O.N",
        "CREATE MODULE SHOP PROCEDURE RECORD () MODIFIES SQL DATA INSERT INTO LOG (WHAT) SELECT G FROM T; END MODULE",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    a.commit().unwrap();
    db
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.35 Description 1, §5.44]
/// ROUTINE_COLUMN_USAGE shows of a routine's body the columns of a
/// searched UPDATE's search condition [1a] and of its update source [1b],
/// those an INSERT's column list names [1c] and an UPDATE's object columns
/// [1d]; not those a query's select list alone references, as FILL's T.G
/// and TOP's T.N.
#[test]
fn routine_column_usage_6_35() {
    let db = referenced();
    let a = db.session("A");
    let sql = "SELECT ROUTINE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.ROUTINE_COLUMN_USAGE \
               WHERE ROUTINE_SCHEMA = 'A' ORDER BY 1, 2, 3";
    assert_eq!(query(&a, sql), ["BUMP,T,G", "BUMP,T,K", "BUMP,T,N", "FILL,LOG,WHAT", "RECORD,LOG,WHAT"]);
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.54 Description 1, §5.65]
/// TRIGGER_COLUMN_USAGE shows of a trigger's definition the columns of
/// each `<search condition>`, its WHEN's and a query's WHERE's, by their
/// transition variables too [1a], and those an INSERT's column list names
/// [1d]; not those a query's select list alone references, as T.G.
#[test]
fn trigger_column_usage_6_54() {
    let db = referenced();
    let a = db.session("A");
    let sql = "SELECT TRIGGER_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.TRIGGER_COLUMN_USAGE ORDER BY 2, 3";
    assert_eq!(query(&a, sql), ["TR,LOG,WHO", "TR,T,K", "TR,T,N"]);
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §19.1 Description 2, §18.1]
/// MODULE_COLUMN_USAGE shows each column a module's routines reference,
/// a query's select list's and an INSERT's column list's alike.
#[test]
fn module_column_usage_19_1() {
    let db = referenced();
    let a = db.session("A");
    let sql = "SELECT MODULE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.MODULE_COLUMN_USAGE ORDER BY 2, 3";
    assert_eq!(query(&a, sql), ["SHOP,LOG,WHAT", "SHOP,T,G"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 GR5i, §11.23 SR7] the triggered
/// action column set holds the columns an INSERT's column list names and
/// a select list references, so DROP COLUMN RESTRICT of LOG's WHO or of
/// T's G, which TR's action names, is refused (SQLCODE -103).
#[test]
fn the_triggered_action_column_set_restricts_11_23() {
    let db = referenced();
    let a = db.session("A");
    assert_eq!(exec(&a, "ALTER TABLE LOG DROP COLUMN WHO RESTRICT").err(), Some(Code::SyntaxRule), "TR names WHO");
    assert_eq!(exec(&a, "ALTER TABLE T DROP COLUMN G RESTRICT").err(), Some(Code::SyntaxRule), "TR's query selects G");
}
