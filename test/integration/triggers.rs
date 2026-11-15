//! Triggers [ISO/IEC 9075-2:2023 §4.46, §11.49, §15.19 to §15.21]
//! (Features T200, T211, T212, T214 to T216, T218): BEFORE and AFTER, row
//! and statement, for each event, their transition variables and tables,
//! the order they run in, and what a failure of their action does.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

/// Schema A: T (K, N), U (K, N) where triggers write, and L, a log of
/// what they saw.
fn logged() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL PRIMARY KEY, N INTEGER)
           CREATE TABLE L (WHAT VARCHAR(40), K INTEGER, N INTEGER)",
    )
    .unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.21 GR4a, GR5a ii] an AFTER INSERT
/// row trigger runs once for each row inserted, its new transition
/// variable that row, in the order the rows were inserted (US048).
#[test]
fn an_after_row_trigger_sees_each_row_inserted() {
    let db = logged();
    let a = db.session("A");
    a.run("CREATE TRIGGER LOG_T AFTER INSERT ON T REFERENCING NEW ROW AS NR FOR EACH ROW INSERT INTO L VALUES ('INS', NR.K, NR.N)")
        .unwrap();
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, N FROM L ORDER BY K"), ["INS,1,10", "INS,2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR7, §15.21 GR5b iii] a BEFORE row
/// trigger's SELECT INTO a new transition variable column changes the row
/// that is inserted.
#[test]
fn a_before_row_trigger_changes_the_new_row() {
    let db = logged();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER TWICE BEFORE INSERT ON T REFERENCING NEW AS NR FOR EACH ROW SELECT NR.N * 2 INTO NR.N FROM (VALUES (1)) AS O (X)",
    )
    .unwrap();
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T"), ["1,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.46.2, §15.21 GR4b] a statement-level
/// trigger runs once for a statement that changes no row, its transition
/// table empty.
#[test]
fn a_statement_trigger_runs_for_no_rows() {
    let db = logged();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER COUNTED AFTER DELETE ON T REFERENCING OLD TABLE AS GONE FOR EACH STATEMENT
           INSERT INTO L SELECT 'DEL', COUNT(*), 0 FROM GONE",
    )
    .unwrap();
    a.run("DELETE FROM T WHERE K = 99").unwrap();
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("DELETE FROM T").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L ORDER BY K"), ["DEL,0", "DEL,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.46.2] an UPDATE OF trigger runs only
/// for an update of a column of its trigger column list; its old and new
/// transition variables are the row before and after.
#[test]
fn an_update_of_trigger_runs_only_for_its_columns() {
    let db = logged();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    a.run(
        "CREATE TRIGGER ON_N AFTER UPDATE OF N ON T REFERENCING OLD AS O NEW AS NW FOR EACH ROW
           INSERT INTO L VALUES ('UPD', O.N, NW.N)",
    )
    .unwrap();
    a.run("UPDATE T SET K = 5").unwrap();
    a.run("UPDATE T SET N = 11").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, N FROM L"), ["UPD,10,11"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.21 GR5b] a row trigger's WHEN
/// condition decides, for each row, whether its action runs.
#[test]
fn a_when_condition_picks_the_rows() {
    let db = logged();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER BIG AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW WHEN (NR.N > 15)
           INSERT INTO L VALUES ('BIG', NR.K, NR.N)",
    )
    .unwrap();
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L"), ["BIG,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.46.1] (Feature T218) triggers of one
/// event and time run in the order they were created.
#[test]
fn triggers_run_in_the_order_created() {
    let db = logged();
    let a = db.session("A");
    for name in ["Z", "M", "B"] {
        a.run(&format!("CREATE TRIGGER {name} AFTER INSERT ON T FOR EACH STATEMENT INSERT INTO L SELECT '{name}', COUNT(*), 0 FROM L"))
            .unwrap();
    }
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L ORDER BY K"), ["Z,0", "M,1", "B,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.21 GR6m, §23.1 GR4g, §4.46.2] an
/// action that fails fails the statement with SQLSTATE 09000, naming the
/// trigger, and nothing the statement did stays.
#[test]
fn a_failed_action_undoes_the_statement() {
    let db = logged();
    let a = db.session("A");
    a.run("INSERT INTO U VALUES (1, 0)").unwrap();
    a.run("CREATE TRIGGER CLASH AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO U VALUES (NR.K, NR.N)").unwrap();
    let e = a.run("INSERT INTO T VALUES (2, 20), (1, 10)").unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::TRIGGERED_ACTION_EXCEPTION, "{e}");
    assert_eq!(e.code, Code::Constraint, "{e}");
    assert!(e.to_string().contains("A.CLASH"), "{e}");
    assert_eq!(rows(&a, "SELECT K FROM T"), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT K FROM U"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR14c, §15.11 GR6a] a BEFORE
/// trigger's action references no generated column of the new row, read
/// or assigned; a generated column is computed again from the row the
/// trigger left.
#[test]
fn a_before_trigger_leaves_generated_columns_alone() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE G (K INTEGER, N INTEGER, D INTEGER GENERATED ALWAYS AS (N * 2))").unwrap();
    let a = db.session("A");
    let refused = |sql: &str| a.run(sql).unwrap_err();
    let read = "CREATE TRIGGER R BEFORE INSERT ON G REFERENCING NEW AS NR FOR EACH ROW SELECT NR.D INTO NR.N FROM (VALUES (1)) AS O (X)";
    assert_eq!(refused(read).code, Code::SyntaxRule);
    let set = "CREATE TRIGGER R BEFORE INSERT ON G REFERENCING NEW AS NR FOR EACH ROW SELECT 1 INTO NR.D FROM (VALUES (1)) AS O (X)";
    assert_eq!(refused(set).code, Code::SyntaxRule);
    a.run("CREATE TRIGGER R BEFORE INSERT ON G REFERENCING NEW AS NR FOR EACH ROW SELECT NR.N + 1 INTO NR.N FROM (VALUES (1)) AS O (X)")
        .unwrap();
    a.run("INSERT INTO G (K, N) VALUES (1, 10)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N, D FROM G"), ["1,11,22"]);
}
