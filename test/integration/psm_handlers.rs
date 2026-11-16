//! SQL/PSM handlers, SIGNAL, RESIGNAL and the diagnostics area stack
//! [ISO/IEC 9075-4:2006 draft §14.2, §17.1 to §17.3] (Features P002, P007).

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::failure;

/// Schema A: T (K, N).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.2 GR3b iii, §14.1 SR17]
/// An UNDO handler undoes the changes of its ATOMIC compound statement
/// before its action runs; the statement that raised is undone alone under
/// a CONTINUE handler, the changes before it kept; UNDO in a compound
/// statement not ATOMIC is refused.
#[test]
fn an_undo_handler_undoes_its_block_14_2() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE P (K INTEGER PRIMARY KEY)").unwrap();
    a.run(
        "BEGIN \
         B: BEGIN ATOMIC DECLARE UNDO HANDLER FOR SQLEXCEPTION INSERT INTO T VALUES (9, 9); \
         INSERT INTO T VALUES (1, 1); INSERT INTO T VALUES (2, 1 / 0); INSERT INTO T VALUES (3, 3); END B; \
         C: BEGIN DECLARE CONTINUE HANDLER FOR SQLEXCEPTION INSERT INTO T VALUES (8, 8); \
         INSERT INTO T VALUES (4, 4); INSERT INTO P VALUES (5), (5); INSERT INTO T VALUES (7, 7); END C; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["4", "7", "8", "9"]);
    assert_eq!(rows(&a, "SELECT K FROM P"), Vec::<String>::new(), "the INSERT that failed after writing a row is undone alone");
    let refused = a.run("BEGIN DECLARE UNDO HANDLER FOR SQLEXCEPTION BEGIN END; END");
    assert_eq!(failure(refused).0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.2 SR6, GR3c, §17.3 GR1, GR3, §17.1]
/// The innermost handler handles a condition, a specific one before a
/// general one; RESIGNAL in a handler's action raises the condition again,
/// its message text replaced, for the enclosing handler; GET STACKED
/// DIAGNOSTICS reads the condition handled; RESIGNAL outside a handler is
/// SQLSTATE 0K000, GET STACKED 0Z002.
#[test]
fn nested_handlers_resignal_14_2() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (M VARCHAR(40), S CHAR(5))").unwrap();
    a.run(
        "CREATE PROCEDURE P () MODIFIES SQL DATA \
         BEGIN DECLARE M VARCHAR(40); DECLARE S CHAR(5); \
         DECLARE EXIT HANDLER FOR SQLEXCEPTION BEGIN \
         GET STACKED DIAGNOSTICS CONDITION 1 M = MESSAGE_TEXT, S = RETURNED_SQLSTATE; INSERT INTO L VALUES (M, S); END; \
         BEGIN DECLARE EXIT HANDLER FOR SQLEXCEPTION INSERT INTO L VALUES ('general', NULL); \
         DECLARE EXIT HANDLER FOR SQLSTATE '75010' RESIGNAL SET MESSAGE_TEXT = 'again'; \
         SIGNAL SQLSTATE '75010' SET MESSAGE_TEXT = 'first'; END; END",
    )
    .unwrap();
    a.run("CALL P()").unwrap();
    assert_eq!(rows(&a, "SELECT M, S FROM L"), ["again,75010"]);
    assert_eq!(failure(a.run("BEGIN RESIGNAL; END")).1, "0K000");
    assert_eq!(failure(a.run("BEGIN DECLARE X INTEGER; GET STACKED DIAGNOSTICS X = NUMBER; END")).1, "0Z002");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §17.1 GR1, GR2; ISO/IEC 9075-2:2023 §23.1]
/// In a handler's action GET CURRENT DIAGNOSTICS reads the area of the
/// action's own last statement, GET STACKED the condition handled (P007).
#[test]
fn current_and_stacked_diagnostics_17_1() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 1), (2, 2)").unwrap();
    a.run(
        "BEGIN DECLARE C, S INTEGER; DECLARE R CHAR(5); \
         DECLARE CONTINUE HANDLER FOR SQLSTATE '22012' BEGIN \
         UPDATE T SET N = 0; GET CURRENT DIAGNOSTICS C = ROW_COUNT; \
         GET STACKED DIAGNOSTICS CONDITION 1 R = RETURNED_SQLSTATE; END; \
         SET S = 1 / 0; INSERT INTO T VALUES (C, 0); INSERT INTO T VALUES (CAST(R AS INTEGER), 0); END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T WHERE K > 2 ORDER BY K"), ["22012"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T WHERE K = 2"), ["2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §17.2 GR3, §8.2 SR1, SR2]
/// SIGNAL of a declared condition with no SQLSTATE raises 45000; SIGNAL
/// of class 01 raises a completion condition, a warning, that no handler
/// handles, so the statements after it run (§17.3 GR6a iii); one of
/// class 00, or not five digits and capital letters, is refused.
#[test]
fn signal_of_a_condition_17_2() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(failure(a.run("BEGIN DECLARE OOPS CONDITION; SIGNAL OOPS; END")).1, "45000");
    let done = a.run("BEGIN SIGNAL SQLSTATE '01H01'; INSERT INTO T VALUES (1, 1); SIGNAL SQLSTATE '02H01'; END").unwrap();
    assert_eq!((done.sqlstate().as_str(), done.sqlcode()), ("02H01", 100));
    assert_eq!(rows(&a, "SELECT K FROM T"), ["1"]);
    for bad in ["00000", "7500", "75a01"] {
        assert_eq!(failure(a.run(&format!("BEGIN SIGNAL SQLSTATE '{bad}'; END"))).0, Code::SyntaxRule, "{bad}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.6, §17.2; ISO/IEC 9075-2:2023 §20.7]
/// A prepared SIGNAL raises its condition each time it is executed; a
/// compound statement is not preparable.
#[test]
fn a_prepared_signal_is_raised_each_execution_17_2() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(failure(a.prepare("BEGIN SIGNAL SQLSTATE '75020'; END")).0, Code::SyntaxRule);
    let p = a.prepare("SIGNAL SQLSTATE '75020' SET MESSAGE_TEXT = 'p'").unwrap();
    for _ in 0..2 {
        let e = a.execute_prepared(&p, &[]).unwrap_err();
        assert_eq!((e.sqlstate().as_str(), e.msg.as_str()), ("75020", "p"));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §17.3 GR4, GR5]
/// RESIGNAL of a `<signal value>` raises its condition ahead of the one
/// its handler handles, which the area, of one condition, leaves out:
/// MORE is Y, NUMBER 1, the new SQLSTATE and message read. RESIGNAL of
/// none raises the condition handled itself, MORE N.
#[test]
fn resignal_of_a_signal_value_stacks_the_condition_handled_17_3() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (S CHAR(5), MSG VARCHAR(40), MORE CHAR(1), N INTEGER)").unwrap();
    for (resignal, expected) in [
        ("RESIGNAL SQLSTATE '75009' SET MESSAGE_TEXT = 'bad input'", "75009,bad input,Y,1"),
        ("RESIGNAL SET MESSAGE_TEXT = 'again'", "22012,again,N,1"),
    ] {
        a.run("DELETE FROM L").unwrap();
        a.run(&format!(
            "BEGIN DECLARE S CHAR(5); DECLARE MSG VARCHAR(40); DECLARE MORE CHAR(1); DECLARE N INTEGER; \
             DECLARE EXIT HANDLER FOR SQLEXCEPTION BEGIN \
             GET STACKED DIAGNOSTICS MORE = MORE, N = NUMBER; \
             GET STACKED DIAGNOSTICS CONDITION 1 S = RETURNED_SQLSTATE, MSG = MESSAGE_TEXT; \
             INSERT INTO L VALUES (S, MSG, MORE, N); END; \
             BEGIN DECLARE Z INTEGER DEFAULT 0; DECLARE EXIT HANDLER FOR SQLSTATE '22012' {resignal}; \
             SET Z = 1 / Z; END; END"
        ))
        .unwrap();
        assert_eq!(rows(&a, "SELECT S, MSG, MORE, N FROM L"), [expected], "{resignal}");
    }
}
