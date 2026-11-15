//! Triggers executed through every path that changes data, their state
//! across reopening, and the bounds of their nesting [ISO/IEC 9075-2:2023
//! §4.46, §9.17, §14.12, §15.17 to §15.21].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::Scratch;

/// Schema A: P (K) and C (K, P) whose rows go with P's by ON DELETE
/// CASCADE; T (K, N); L, a log.
fn related(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY)
           CREATE TABLE C (K INTEGER, P INTEGER REFERENCES P ON DELETE CASCADE)
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER)
           CREATE TABLE L (WHAT VARCHAR(20), K INTEGER)",
    )
    .unwrap();
}

/// A database of `related`'s schema.
fn fresh() -> Database {
    let db = Database::new();
    related(&db);
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.17 GR, §15.18, §9.17 GR17b] the rows
/// a referential action deletes are a state change of their own table,
/// whose AFTER triggers execute once the statement's actions are done.
#[test]
fn a_referential_action_executes_triggers() {
    let db = fresh();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES (1), (2)").unwrap();
    a.run("INSERT INTO C VALUES (10, 1), (11, 1), (20, 2)").unwrap();
    a.run("CREATE TRIGGER A_C AFTER DELETE ON C REFERENCING OLD TABLE AS GONE FOR EACH STATEMENT INSERT INTO L SELECT 'GONE', COUNT(*) FROM GONE")
        .unwrap();
    a.run("CREATE TRIGGER R_C AFTER DELETE ON C REFERENCING OLD AS O FOR EACH ROW INSERT INTO L VALUES ('ROW', O.K)").unwrap();
    a.run("DELETE FROM P WHERE K = 1").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L ORDER BY WHAT, K"), ["GONE,2", "ROW,10", "ROW,11"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.14 GR5, §15.18] the rows a
/// referential action updates go through their table's BEFORE triggers,
/// which may change the new rows before they are written.
#[test]
fn a_referential_update_executes_before_triggers() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY)
           CREATE TABLE D (K INTEGER, P INTEGER REFERENCES P ON UPDATE CASCADE)
           CREATE TRIGGER B_D BEFORE UPDATE ON D REFERENCING NEW AS NR FOR EACH ROW
             SELECT NR.K + 100 INTO NR.K FROM (VALUES (1)) AS O (X)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES (1), (2)").unwrap();
    a.run("INSERT INTO D VALUES (10, 1), (20, 2)").unwrap();
    a.run("UPDATE P SET K = 3 WHERE K = 1").unwrap();
    assert_eq!(rows(&a, "SELECT K, P FROM D ORDER BY K"), ["20,2", "110,3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.13 GR, §15.14 GR5, §15.8 GR2,
/// §15.19, §15.20] a positioned UPDATE and DELETE are state changes of
/// the cursor's table, the row under the cursor given to its BEFORE and
/// AFTER triggers.
#[test]
fn positioned_statements_execute_triggers() {
    let db = fresh();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("CREATE TRIGGER B_T BEFORE UPDATE ON T REFERENCING NEW AS NR FOR EACH ROW SELECT NR.N + 1 INTO NR.N FROM (VALUES (1)) AS O (X)")
        .unwrap();
    a.run("CREATE TRIGGER G_T AFTER DELETE ON T REFERENCING OLD AS O FOR EACH ROW INSERT INTO L VALUES ('GONE', O.K)").unwrap();
    a.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM T WHERE K = 1").unwrap();
    let mut p = Params::new().declare("K_OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    a.execute("OPEN C", &mut p).unwrap();
    a.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    a.execute("UPDATE T SET N = 100 WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,101", "2,20"]);
    a.execute("DELETE FROM T WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L"), ["GONE,1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a, §15.19, §15.20] MERGE
/// executes the BEFORE triggers of each clause's state change on that
/// clause's rows, and its AFTER triggers once it is done, a statement
/// trigger of each event once.
#[test]
fn a_merge_executes_triggers_by_clause() {
    let db = fresh();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    let before = |name: &str, event: &str, add: u32| {
        format!(
            "CREATE TRIGGER {name} BEFORE {event} ON T REFERENCING NEW AS NR FOR EACH ROW SELECT NR.N + {add} INTO NR.N FROM (VALUES (1)) AS X (Y)"
        )
    };
    a.run(&before("BU", "UPDATE", 1000)).unwrap();
    a.run(&before("BI", "INSERT", 100)).unwrap();
    for (name, event) in [("AU", "UPDATE"), ("AI", "INSERT")] {
        a.run(&format!("CREATE TRIGGER {name} AFTER {event} ON T FOR EACH STATEMENT INSERT INTO L SELECT '{name}', COUNT(*) FROM T"))
            .unwrap();
    }
    a.run("MERGE INTO T USING (VALUES (1, 11), (2, 20)) AS S (K, N) ON T.K = S.K WHEN MATCHED THEN UPDATE SET N = S.N WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.N)")
        .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,1011", "2,120"]);
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L ORDER BY WHAT"), ["AI,2", "AU,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.46.2] (RULES.md D38) a transition
/// table larger than MEMORY.WORK_BYTES is spilled and read whole.
#[test]
fn a_large_transition_table_is_read_whole() {
    let db = fresh();
    let a = db.session("A");
    a.run("SET MEMORY.WORK_BYTES = 65536").unwrap();
    a.run("CREATE TRIGGER MANY AFTER INSERT ON T REFERENCING NEW TABLE AS ADDED FOR EACH STATEMENT INSERT INTO L SELECT 'ADDED', SUM(N) FROM ADDED")
        .unwrap();
    let values: Vec<String> = (1..=5000).map(|k| format!("({k}, {k})")).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L"), ["ADDED,12502500"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 GR] TRUNCATE TABLE deletes its
/// rows without executing any trigger.
#[test]
fn truncate_executes_no_trigger() {
    let db = fresh();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    a.run("CREATE TRIGGER GONE AFTER DELETE ON T FOR EACH STATEMENT INSERT INTO L VALUES ('GONE', 0)").unwrap();
    a.run("TRUNCATE TABLE T").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T"), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT WHAT FROM L"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.17 GR5, §6.32] a triggered action's
/// datetime value functions give its triggering statement's instant.
#[test]
fn an_action_shares_its_statement_instant() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, TS TIMESTAMP) CREATE TABLE L (TS TIMESTAMP)").unwrap();
    let a = db.session("A");
    a.run("CREATE TRIGGER WHEN_T AFTER INSERT ON T FOR EACH ROW INSERT INTO L VALUES (CURRENT_TIMESTAMP)").unwrap();
    let values: Vec<String> = (1..=200).map(|k| format!("({k}, CURRENT_TIMESTAMP)")).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT TS) FROM L"), ["1"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T, L WHERE T.TS = L.TS"), ["40000"]);
}

/// Verifies: [DESIGN] a prepared statement executed after a trigger is
/// defined executes it: its plans are made again.
#[test]
fn a_prepared_statement_executes_a_new_trigger() {
    let db = fresh();
    let a = db.session("A");
    let p = a.prepare("INSERT INTO T VALUES (?, 0)").unwrap();
    a.execute_prepared(&p, &[Value::Exact(Decimal::new(1, 0))]).unwrap();
    a.run("CREATE TRIGGER NEW_T AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES ('NEW', NR.K)").unwrap();
    a.execute_prepared(&p, &[Value::Exact(Decimal::new(2, 0))]).unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L"), ["NEW,2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1 GR4g, §15.21 GR6m] after a
/// triggered action fails, the condition names the trigger and its table,
/// and MORE says the condition that caused it is left out (IA079).
#[test]
fn a_failure_names_the_trigger_in_the_diagnostics_area() {
    let db = fresh();
    let a = db.session("A");
    a.run("CREATE TRIGGER BAD AFTER INSERT ON L FOR EACH STATEMENT INSERT INTO T VALUES (NULL, 0)").unwrap();
    let e = a.run("INSERT INTO L VALUES ('X', 1)").unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::TRIGGERED_ACTION_EXCEPTION, "{e}");
    let item = |name: &str| {
        let mut p = Params::new().declare("TXT", DataType::character(128), Value::Null);
        a.execute(&format!("GET DIAGNOSTICS CONDITION 1 TXT = {name}"), &mut p).unwrap();
        p.value("TXT").to_string().trim_end().to_owned()
    };
    assert_eq!([item("TRIGGER_SCHEMA"), item("TRIGGER_NAME")], ["A", "BAD"]);
    assert_eq!([item("SCHEMA_NAME"), item("TABLE_NAME"), item("RETURNED_SQLSTATE")], ["A", "L", "09000"]);
    let mut p = Params::new().declare("TXT", DataType::character(128), Value::Null);
    a.execute("GET DIAGNOSTICS TXT = MORE", &mut p).unwrap();
    assert_eq!(p.value("TXT").to_string().trim_end(), "Y", "the cause is left out");
    assert_eq!(rows(&a, "SELECT K FROM L"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 GR5, §4.46.1] a trigger is kept
/// with the definitions: after stopping without a checkpoint, by its
/// logged definition, and after a checkpoint, by the snapshot.
#[test]
fn triggers_are_kept_across_reopening() {
    let scratch = Scratch::new("triggers-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        related(&db);
        let a = db.session("A");
        a.run("CREATE TRIGGER LOGGED AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES ('LOGGED', NR.K)").unwrap();
        a.commit().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1, 0)").unwrap();
        a.run("CREATE TRIGGER KEPT AFTER INSERT ON T FOR EACH STATEMENT INSERT INTO L VALUES ('KEPT', 0)").unwrap();
        a.commit().unwrap();
        db.checkpoint().unwrap();
        drop(a);
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (2, 0)").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K FROM L ORDER BY K, WHAT"), ["KEPT,0", "LOGGED,1", "LOGGED,2"]);
    a.commit().unwrap();
    drop(a);
    db.close().unwrap();
}

/// Half the default stack of a thread.
const HALF_DEFAULT_STACK: usize = 1024 * 1024;

/// Verifies: [DESIGN] a trigger whose action executes it again recurses
/// until its WHEN condition stops it, and no deeper than the limit of
/// nesting, beyond which its statement fails with SQLSTATE 09000, its
/// cause SQLCODE -900, never a stack overflow: on half the default stack.
#[test]
fn a_recursive_trigger_stops_at_the_limit() {
    let outcome = std::thread::Builder::new()
        .name("triggers".to_owned())
        .stack_size(HALF_DEFAULT_STACK)
        .spawn(|| {
            let db = fresh();
            let a = db.session("A");
            let again = |limit: u32| format!("CREATE TRIGGER AGAIN AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW WHEN (NR.K < {limit}) INSERT INTO T VALUES (NR.K + 1, 0)");
            a.run(&again(10)).unwrap();
            a.run("INSERT INTO T VALUES (1, 0)").unwrap();
            let shallow = rows(&a, "SELECT COUNT(*) FROM T");
            a.run("DELETE FROM T").unwrap();
            a.run("DROP TRIGGER AGAIN").unwrap();
            a.run(&again(1000)).unwrap();
            let e = a.run("INSERT INTO T VALUES (1, 0)").unwrap_err();
            (shallow, e.sqlstate(), e.code, rows(&a, "SELECT COUNT(*) FROM T"))
        })
        .expect("the thread starts")
        .join()
        .expect("the statements do not panic");
    assert_eq!(outcome, (vec!["10".to_owned()], SqlState::TRIGGERED_ACTION_EXCEPTION, Code::Unsupported, vec!["0".to_owned()]));
}
