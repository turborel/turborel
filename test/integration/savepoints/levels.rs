//! Savepoint levels [ISO/IEC 9075-2:2023 §4.43.2] (Feature T272): a
//! function, a procedure of NEW SAVEPOINT LEVEL, an ATOMIC compound
//! statement and a triggered action each run at a level of their own,
//! destroyed with its savepoints as they end; a procedure of OLD
//! SAVEPOINT LEVEL runs at its caller's. A statement names only the
//! savepoints of the current level, and goes back past no atomic
//! execution context's beginning.

use turborel::db::Database;

use super::{state, values};
use crate::common::{dynamic_host, run_with};

/// Schema A: T (K) and L (K), empty, and the routines `routines` define,
/// committed.
fn with(routines: &[&str]) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE L (K INTEGER)").unwrap();
    let a = db.session("A");
    for r in routines {
        a.run(r).unwrap();
    }
    a.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 GR3k, §9.18 GR7, GR15, §17.5 GR4]
/// a procedure of OLD SAVEPOINT LEVEL, or of neither, establishes its
/// savepoints at its caller's level: they outlive the CALL, so the caller
/// goes back to one, and its body goes back to the caller's.
#[test]
fn an_old_level_procedure_shares_its_callers_level_9_18() {
    let db = with(&[
        "CREATE PROCEDURE MARK () MODIFIES SQL DATA OLD SAVEPOINT LEVEL BEGIN INSERT INTO T VALUES (1); SAVEPOINT INSIDE; END",
        "CREATE PROCEDURE BACK () MODIFIES SQL DATA ROLLBACK TO SAVEPOINT OUTSIDE",
    ]);
    let a = db.session("A");
    a.run("CALL MARK()").unwrap();
    a.run("INSERT INTO T VALUES (2)").unwrap();
    a.run("ROLLBACK TO SAVEPOINT INSIDE").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T"), ["1"]);
    a.run("SAVEPOINT OUTSIDE").unwrap();
    a.run("INSERT INTO T VALUES (3)").unwrap();
    a.run("CALL BACK()").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T"), ["1"], "the body went back to the caller's savepoint");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 GR3k, §9.18 GR7, GR15, §17.6
/// GR2, §17.8 GR3b] a procedure of NEW SAVEPOINT LEVEL names none of its
/// caller's savepoints (3B001), its own of the same name apart; they are
/// destroyed as it returns, what it changed staying.
#[test]
fn a_new_level_procedure_has_its_own_savepoints_9_18() {
    let db = with(&[
        "CREATE PROCEDURE OWN () MODIFIES SQL DATA NEW SAVEPOINT LEVEL BEGIN SAVEPOINT S; INSERT INTO T VALUES (2); \
         ROLLBACK TO SAVEPOINT S; INSERT INTO T VALUES (3); SAVEPOINT KEPT; END",
        "CREATE PROCEDURE REACH () MODIFIES SQL DATA NEW SAVEPOINT LEVEL ROLLBACK TO SAVEPOINT S",
        "CREATE PROCEDURE FREE_IT () MODIFIES SQL DATA NEW SAVEPOINT LEVEL RELEASE SAVEPOINT S",
    ]);
    let a = db.session("A");
    a.run("SAVEPOINT S").unwrap();
    a.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(state(a.run("CALL REACH()")), "3B001");
    assert_eq!(state(a.run("CALL FREE_IT()")), "3B001");
    a.run("CALL OWN()").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "3"]);
    assert_eq!(state(a.run("RELEASE SAVEPOINT KEPT")), "3B001", "destroyed with its level");
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T"), Vec::<String>::new(), "the caller's S, apart from the procedure's");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 GR7, GR15, §17.8 GR3b, GR3c,
/// §4.41.5] a function runs at a level of its own, an atomic execution
/// context: its body goes back to a savepoint of its own, names none of
/// its caller's (3B001), and leaves none behind.
#[test]
fn a_function_runs_at_a_level_of_its_own_9_18() {
    let db = with(&[
        "CREATE FUNCTION OWN () RETURNS INTEGER MODIFIES SQL DATA BEGIN SAVEPOINT F; INSERT INTO L VALUES (1); \
         ROLLBACK TO SAVEPOINT F; INSERT INTO L VALUES (2); RETURN 7; END",
        "CREATE FUNCTION REACH () RETURNS INTEGER MODIFIES SQL DATA BEGIN ROLLBACK TO SAVEPOINT S; RETURN 0; END",
        "CREATE PROCEDURE USE_OWN (OUT N INTEGER) MODIFIES SQL DATA SET N = OWN()",
        "CREATE PROCEDURE USE_REACH (OUT N INTEGER) MODIFIES SQL DATA SET N = REACH()",
    ]);
    let a = db.session("A");
    let mut p = dynamic_host();
    a.run("SAVEPOINT S").unwrap();
    run_with(&a, "CALL USE_OWN(N)", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "7");
    assert_eq!(values(&a, "SELECT K FROM L"), ["2"]);
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT F")), "3B001", "destroyed as it returned");
    assert_eq!(state(a.execute("CALL USE_REACH(N)", &mut p)), "3B001");
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(values(&a, "SELECT K FROM L"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §14.1 GR1, GR6; ISO/IEC
/// 9075-2:2023 §17.8 GR3c] an ATOMIC compound statement runs at a level of
/// its own: it goes back to a savepoint it established, none established
/// before it began (3B001), and its savepoints end with it; one NOT ATOMIC
/// shares the level it runs at.
#[test]
fn an_atomic_block_runs_at_a_level_of_its_own_14_1() {
    let db = with(&[]);
    let a = db.session("A");
    a.run("SAVEPOINT S").unwrap();
    a.run("BEGIN ATOMIC SAVEPOINT B; INSERT INTO T VALUES (1); ROLLBACK TO SAVEPOINT B; INSERT INTO T VALUES (2); END").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T"), ["2"]);
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT B")), "3B001", "ended with the block");
    assert_eq!(state(a.run("BEGIN ATOMIC INSERT INTO T VALUES (3); ROLLBACK TO SAVEPOINT S; END")), "3B001");
    assert_eq!(values(&a, "SELECT K FROM T"), ["2"], "the block failed whole");
    a.run("BEGIN NOT ATOMIC INSERT INTO T VALUES (4); SAVEPOINT N; INSERT INTO T VALUES (5); END").unwrap();
    a.run("ROLLBACK TO SAVEPOINT N").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["2", "4"]);
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §14.13 SR9, §14.2] a FOR holds no
/// ROLLBACK TO SAVEPOINT (2A000); a handler takes 3B001 raised in a
/// procedure's body, which goes on.
#[test]
fn a_for_refuses_rollback_to_and_a_handler_takes_3b001_14_13() {
    let db = with(&["CREATE PROCEDURE HANDLED (OUT C CHAR(5)) MODIFIES SQL DATA BEGIN \
         DECLARE CONTINUE HANDLER FOR SQLSTATE '3B001' SET C = 'taken'; RELEASE SAVEPOINT NONE; INSERT INTO T VALUES (1); END"]);
    let a = db.session("A");
    let refused = a.run("CREATE PROCEDURE LOOPED () MODIFIES SQL DATA FOR R AS SELECT K FROM T DO ROLLBACK TO SAVEPOINT S; END FOR");
    assert_eq!(state(refused), "2A000");
    let mut p = dynamic_host();
    run_with(&a, "CALL HANDLED(TXT)", &mut p).unwrap();
    assert_eq!(p.value("TXT").to_string().trim(), "taken");
    assert_eq!(values(&a, "SELECT K FROM T"), ["1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.21 GR6c, GR6e iii, GR6j] (IA046) a
/// triggered action runs at a level of its own, and executes no
/// savepoint's statement: a trigger whose action is one is refused as it
/// is defined (2A000), and one calling a procedure that executes one
/// fails as it fires with 0W000, raised as a triggered action exception
/// (09000), the triggering statement undone.
#[test]
fn a_triggered_action_executes_no_savepoint_statement_15_21() {
    let db =
        with(&["CREATE PROCEDURE MARK () MODIFIES SQL DATA SAVEPOINT X", "CREATE TRIGGER G AFTER INSERT ON L FOR EACH ROW CALL MARK()"]);
    let a = db.session("A");
    assert_eq!(state(a.run("CREATE TRIGGER H AFTER INSERT ON T FOR EACH ROW SAVEPOINT X")), "2A000");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    let fired = a.run("INSERT INTO L VALUES (1)").unwrap_err();
    assert_eq!(fired.sqlstate().as_str(), "09000", "{fired:?}");
    assert!(fired.msg.contains("GR6e iii"), "{fired:?}");
    assert_eq!(values(&a, "SELECT K FROM L"), Vec::<String>::new());
    assert_eq!(values(&a, "SELECT K FROM T"), ["1"], "the transaction goes on");
}

/// Verifies: [DESIGN] RULES.md D88. ROLLBACK TO SAVEPOINT in a routine's
/// body is refused (SQLCODE -900) when it would cancel a change of
/// definitions made before the statement that invoked the routine began,
/// as that statement runs on them; run directly it cancels the change.
#[test]
fn a_body_cancels_no_definition_its_caller_runs_on() {
    let db = with(&["CREATE PROCEDURE BACK () MODIFIES SQL DATA ROLLBACK TO SAVEPOINT S"]);
    let a = db.session("A");
    a.run("SAVEPOINT S").unwrap();
    a.run("CREATE TABLE U (K INTEGER)").unwrap();
    let refused = a.run("CALL BACK()").unwrap_err();
    assert_eq!(refused.code, turborel::error::Code::Unsupported, "{refused:?}");
    a.run("INSERT INTO U VALUES (1)").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert!(a.run("SELECT K FROM U").is_err(), "U cancelled");
}
