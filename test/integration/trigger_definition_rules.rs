//! The Syntax Rules of a `<trigger definition>` [ISO/IEC 9075-2:2023
//! §11.49]: the trigger's name and subject table, its trigger column
//! list, and the statements its triggered action may contain.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N), a log L, a temporary table G and a view CV WITH
/// CHECK OPTION.
fn defined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER)
           CREATE TABLE L (K INTEGER)
           CREATE GLOBAL TEMPORARY TABLE G (K INTEGER) ON COMMIT PRESERVE ROWS
           CREATE VIEW CV AS SELECT K, N FROM T WHERE N > 0 WITH CHECK OPTION",
    )
    .unwrap();
    db
}

/// The SQLCODE `sql` is refused with in `s`, and its message.
fn refused(s: &Session, sql: &str) -> (Code, String) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    (e.code, e.to_string())
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 SR3, SR4, SR5b] a trigger is
/// refused (SQLCODE -103) if its schema has a trigger of its name
/// already, if its subject table does not exist, or if, not INSTEAD OF,
/// its subject table is a temporary table.
#[test]
fn a_trigger_names_a_new_trigger_and_a_base_table() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TRIGGER LOG AFTER INSERT ON T FOR EACH STATEMENT INSERT INTO L VALUES (0)").unwrap();
    let cases = [
        ("CREATE TRIGGER LOG AFTER DELETE ON T FOR EACH STATEMENT INSERT INTO L VALUES (1)", "SR4"),
        ("CREATE TRIGGER NONE AFTER INSERT ON MISSING FOR EACH STATEMENT INSERT INTO L VALUES (1)", "SR3"),
        ("CREATE TRIGGER TEMP AFTER INSERT ON G FOR EACH STATEMENT INSERT INTO L VALUES (1)", "temporary table"),
    ];
    for (sql, rule) in cases {
        let (code, message) = refused(&a, sql);
        assert!(code == Code::SyntaxRule && message.contains(rule), "{sql}: {message}");
    }
    a.run("INSERT INTO T VALUES (1, 1)").unwrap();
    assert!(a.open_cursor("SELECT K FROM L").unwrap().fetch().unwrap().is_some(), "the first trigger kept");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 SR1a] a trigger of a schema
/// definition named in another schema is refused (SQLCODE -103), and the
/// schema with it.
#[test]
fn a_schema_trigger_is_named_in_its_schema() {
    let db = Database::new();
    let e = db
        .schema(
            "CREATE SCHEMA AUTHORIZATION B
               CREATE TABLE T (K INTEGER)
               CREATE TRIGGER OTHER.LOG AFTER INSERT ON T FOR EACH STATEMENT DELETE FROM T",
        )
        .unwrap_err();
    assert!(e.code == Code::SyntaxRule && e.to_string().contains("SR1a"), "{e}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 SR6a, SR6b] a trigger column
/// list names each column once, each a column of the subject table, or
/// the trigger is refused (SQLCODE -103).
#[test]
fn a_trigger_column_list_names_columns_of_its_table_once() {
    let db = defined();
    let a = db.session("A");
    let cases = [
        ("CREATE TRIGGER U AFTER UPDATE OF N, N ON T FOR EACH STATEMENT INSERT INTO L VALUES (1)", "SR6a"),
        ("CREATE TRIGGER U AFTER UPDATE OF N, Z ON T FOR EACH STATEMENT INSERT INTO L VALUES (1)", "SR6b"),
    ];
    for (sql, rule) in cases {
        let (code, message) = refused(&a, sql);
        assert!(code == Code::SyntaxRule && message.contains(rule), "{sql}: {message}");
    }
    a.run("CREATE TRIGGER U AFTER UPDATE OF N, K ON T FOR EACH STATEMENT INSERT INTO L VALUES (1)").unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 SR13] a triggered action
/// contains no SQL session statement or schema statement (IA046): one
/// that does is refused with SQLCODE -103.
#[test]
fn a_triggered_action_contains_no_session_statement() {
    let db = defined();
    let a = db.session("A");
    let cases = [
        "CREATE TRIGGER S AFTER INSERT ON T FOR EACH STATEMENT SET SCHEMA 'A'",
        "CREATE TRIGGER S AFTER INSERT ON T FOR EACH STATEMENT BEGIN ATOMIC SET CONSTRAINTS ALL DEFERRED; END",
    ];
    for sql in cases {
        let (code, message) = refused(&a, sql);
        assert!(code == Code::SyntaxRule && message.contains("SR13"), "{sql}: {message}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.49 SR15c] an INSTEAD OF trigger is
/// on a view; on a view WITH CHECK OPTION it is refused (SQLCODE -103).
#[test]
fn an_instead_of_trigger_is_not_on_a_checked_view() {
    let db = defined();
    let a = db.session("A");
    let (code, message) = refused(&a, "CREATE TRIGGER I INSTEAD OF INSERT ON CV FOR EACH ROW INSERT INTO L VALUES (1)");
    assert!(code == Code::SyntaxRule && message.contains("WITH CHECK OPTION"), "{message}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.8 SR1] [ISO/IEC 9075-4:2006
/// draft §12.6] a positioned DELETE names a cursor declared in its scope:
/// in a triggered action, which declares none, it is refused (SQLCODE
/// -103); in a procedure's compound statement it deletes the row its
/// cursor is on as the procedure runs, and the procedure names the table.
#[test]
fn a_positioned_delete_is_of_a_cursor_in_scope() {
    let db = defined();
    let a = db.session("A");
    let (code, message) = refused(
        &a,
        "CREATE TRIGGER P AFTER INSERT ON T FOR EACH STATEMENT BEGIN ATOMIC \
           DECLARE X INTEGER; DECLARE C CURSOR FOR SELECT K FROM L FOR UPDATE; \
           OPEN C; FETCH C INTO X; DELETE FROM L WHERE CURRENT OF C; CLOSE C; END",
    );
    assert!(code == Code::SyntaxRule && message.contains("§14.8 SR1"), "{message}");
    a.run("INSERT INTO L VALUES (5), (6)").unwrap();
    a.run(
        "CREATE PROCEDURE P () MODIFIES SQL DATA BEGIN \
           DECLARE X INTEGER; \
           DECLARE C CURSOR FOR SELECT K FROM L WHERE K = 5 FOR UPDATE; \
           OPEN C; FETCH C INTO X; DELETE FROM L WHERE CURRENT OF C; CLOSE C; \
         END",
    )
    .unwrap();
    a.run("CALL P()").unwrap();
    let mut c = a.open_cursor("SELECT K FROM L").unwrap();
    let mut left = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        left.push(row[0].to_string());
    }
    assert_eq!(left, ["6"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.31 SR7g] a table a triggered action
/// names only in a subquery is one it references, wherever the subquery
/// is written: a MERGE's inserted row or its update's source, a simple
/// CASE's WHEN operand, a CASE statement's WHEN operand, an element of a
/// multiple variable assignment; DROP
/// TABLE of it RESTRICT is refused (SQLCODE -103) while the trigger is
/// defined.
#[test]
fn a_table_named_deep_in_an_action_is_referenced() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE S (K INTEGER)").unwrap();
    a.commit().unwrap();
    let actions = [
        "MERGE INTO L USING (VALUES (1)) AS Q (X) ON L.K = Q.X WHEN NOT MATCHED THEN INSERT VALUES ((SELECT MAX(K) FROM S))",
        "MERGE INTO L USING (VALUES (1)) AS Q (X) ON L.K = Q.X WHEN MATCHED THEN UPDATE SET K = (SELECT MAX(K) FROM S)",
        "UPDATE T SET N = CASE K WHEN (SELECT MAX(K) FROM S) THEN 1 ELSE 2 END",
        "BEGIN ATOMIC DECLARE X, Y INTEGER; SET (X, Y) = ((SELECT MAX(K) FROM S), 1); END",
        "BEGIN ATOMIC DECLARE X INTEGER; CASE X WHEN (SELECT MAX(K) FROM S) THEN SET X = NULL; ELSE SET X = 1; END CASE; END",
    ];
    for action in actions {
        a.run(&format!("CREATE TRIGGER X AFTER DELETE ON T FOR EACH STATEMENT {action}")).unwrap_or_else(|e| panic!("{action}: {e}"));
        a.commit().unwrap();
        let (code, message) = refused(&a, "DROP TABLE S RESTRICT");
        assert!(code == Code::SyntaxRule && message.contains("§11.31 SR7g"), "{action}: {message}");
        a.rollback().unwrap();
        a.run("DROP TRIGGER X").unwrap();
        a.commit().unwrap();
    }
    a.run("CREATE TRIGGER X AFTER DELETE ON T FOR EACH STATEMENT BEGIN ATOMIC DECLARE X INTEGER; SET X = NULL; END").unwrap();
    a.commit().unwrap();
    a.run("DROP TABLE S RESTRICT").unwrap();
}
