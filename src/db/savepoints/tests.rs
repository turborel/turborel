//! Unit tests of a session's savepoints and savepoint levels: names at
//! levels, replaced, released and gone back to, the limit, atomic
//! execution contexts, and the statements run directly (RULES.md
//! R-TEST-1, R-TEST-2).

use super::*;
use crate::db::Database;

/// The SQLSTATE of `r`'s error, if it failed.
fn state<T>(r: Result<T>) -> Option<String> {
    r.err().map(|e| e.sqlstate().to_string())
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5 GR2, §17.6 GR2, GR3, §17.8 GR3b,
/// GR3e] a savepoint is named at its level only; SAVEPOINT of a name in
/// use replaces it; RELEASE destroys it and those after; going back
/// destroys those after it, it staying; a name unknown is 3B001.
#[test]
fn savepoints_are_named_at_their_level() {
    let mut levels: Levels<u32> = Levels::default();
    assert_eq!(levels.make_room("A").unwrap(), None);
    levels.establish("A", 1);
    levels.establish("B", 2);
    assert_eq!(levels.make_room("A").unwrap(), Some(1), "replaced");
    levels.establish("A", 3);
    levels.open(Level::Procedure);
    assert_eq!(state(levels.target("A")), Some("3B001".into()), "of another level");
    levels.establish("A", 4);
    levels.establish("C", 5);
    assert_eq!(levels.after("A"), [5]);
    assert_eq!(*levels.target("A").unwrap(), 4);
    assert_eq!(levels.close(), [4]);
    assert_eq!(levels.release("B").unwrap(), [2, 3]);
    assert_eq!(state(levels.release("B")), Some("3B001".into()));
    assert_eq!(state(levels.target("Z")), Some("3B001".into()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3c, §4.41.5] a savepoint
/// established before the latest atomic execution context began is not
/// gone back to (3B001); a procedure's level is no such context, a
/// function's, a trigger's and an ATOMIC block's are. A trigger's level
/// is noted as triggered.
#[test]
fn atomic_contexts_bound_rollback_to() {
    let mut levels: Levels<u32> = Levels::default();
    levels.open(Level::Procedure);
    levels.establish("A", 1);
    levels.open(Level::Atomic);
    levels.close();
    assert_eq!(levels.target("A").copied().ok(), Some(1), "the block ended");
    for level in [Level::Atomic, Level::Function, Level::Trigger] {
        let mut l: Levels<u32> = Levels::default();
        l.establish("A", 1);
        l.open(level);
        l.establish("B", 2);
        assert_eq!(l.target("B").copied().ok(), Some(2), "{level:?}");
        assert_eq!(l.triggered(), level == Level::Trigger);
    }
    assert!(!Level::Procedure.atomic() && Level::Atomic.atomic());
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5 GR3] past `MAX_SAVEPOINTS` a
/// SAVEPOINT is 3B002, but one replacing a name in use is not.
#[test]
fn savepoints_are_at_most_the_limit_17_5() {
    let mut levels: Levels<()> = Levels::default();
    for n in 0..MAX_SAVEPOINTS {
        levels.make_room(&format!("S{n}")).unwrap();
        levels.establish(&format!("S{n}"), ());
    }
    assert_eq!(state(levels.make_room("MORE")), Some("3B002".into()));
    assert!(levels.make_room("S0").is_ok());
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8 GR3d, GR3g; IA168]
/// run directly, SAVEPOINT begins a transaction; ROLLBACK TO cancels the
/// rows written since, those before staying, and closes every cursor;
/// RELEASE then destroys it; without a transaction, RELEASE and ROLLBACK
/// TO are 3B001; COMMIT ends the savepoints.
#[test]
fn savepoints_run_directly() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("A");
    assert_eq!(state(s.run("RELEASE SAVEPOINT S1")), Some("3B001".into()));
    assert_eq!(state(s.run("ROLLBACK TO SAVEPOINT S1")), Some("3B001".into()));
    s.run("SAVEPOINT S0").unwrap();
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.run("SAVEPOINT S1").unwrap();
    s.run("INSERT INTO T VALUES (2)").unwrap();
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM T").unwrap();
    s.run("OPEN C").unwrap();
    assert_eq!(state(s.run("OPEN C")), Some("24000".into()), "open");
    s.run("ROLLBACK TO SAVEPOINT S1").unwrap();
    s.run("OPEN C").unwrap();
    s.run("CLOSE C").unwrap();
    s.run("INSERT INTO T VALUES (3)").unwrap();
    s.run("RELEASE SAVEPOINT S1").unwrap();
    assert_eq!(state(s.run("ROLLBACK TO SAVEPOINT S1")), Some("3B001".into()));
    s.commit().unwrap();
    assert_eq!(state(s.run("ROLLBACK TO SAVEPOINT S0")), Some("3B001".into()));
    let mut rows = s.open_cursor("SELECT K FROM T ORDER BY K").unwrap();
    let mut seen = Vec::new();
    while let Some(row) = rows.fetch().unwrap() {
        seen.push(row[0].to_string());
    }
    assert_eq!(seen, ["1", "3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d] ROLLBACK TO cancels a table
/// created after the savepoint, which is then no longer known, and keeps
/// one created before.
#[test]
fn rollback_to_cancels_definitions_after_it() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("A");
    s.run("CREATE TABLE U (K INTEGER)").unwrap();
    s.run("SAVEPOINT S").unwrap();
    s.run("CREATE TABLE V (K INTEGER)").unwrap();
    s.run("INSERT INTO V VALUES (1)").unwrap();
    s.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert!(s.run("INSERT INTO V VALUES (1)").is_err(), "V cancelled");
    s.run("INSERT INTO U VALUES (1)").unwrap();
    s.commit().unwrap();
    assert!(s.run("SELECT K FROM V").is_err());
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 GR7, §17.8 GR3c, §15.21 GR6e iii]
/// a procedure's body goes back to a savepoint it established, its rows
/// since cancelled; one of NEW SAVEPOINT LEVEL does not see the caller's
/// savepoint (3B001), one of OLD does; an ATOMIC block does not go back
/// past its beginning (3B001); a trigger's action runs no savepoint's
/// statement, nor a procedure it calls (0W000).
#[test]
fn savepoints_in_routines_and_triggers() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE TABLE L (K INTEGER)").unwrap();
    let s = db.session("A");
    s.run("CREATE PROCEDURE P () MODIFIES SQL DATA BEGIN SAVEPOINT S; INSERT INTO T VALUES (1); ROLLBACK TO SAVEPOINT S; INSERT INTO T VALUES (2); END").unwrap();
    s.run("CREATE PROCEDURE KEPT () MODIFIES SQL DATA ROLLBACK TO SAVEPOINT OUTSIDE").unwrap();
    s.run("CREATE PROCEDURE FRESH () MODIFIES SQL DATA NEW SAVEPOINT LEVEL ROLLBACK TO SAVEPOINT OUTSIDE").unwrap();
    s.run("CREATE PROCEDURE BLOCKED () MODIFIES SQL DATA BEGIN SAVEPOINT B; BEGIN ATOMIC ROLLBACK TO SAVEPOINT B; END; END").unwrap();
    s.run("CREATE PROCEDURE SP () MODIFIES SQL DATA SAVEPOINT X").unwrap();
    s.run("CREATE TRIGGER G AFTER INSERT ON L FOR EACH ROW CALL SP()").unwrap();
    s.commit().unwrap();
    s.run("CALL P()").unwrap();
    s.run("SAVEPOINT OUTSIDE").unwrap();
    s.run("INSERT INTO T VALUES (3)").unwrap();
    assert_eq!(state(s.run("CALL FRESH()")), Some("3B001".into()));
    s.run("CALL KEPT()").unwrap();
    assert_eq!(state(s.run("CALL BLOCKED()")), Some("3B001".into()));
    // [§15.21 GR6e iii] the 0W000 the action raises fails it,
    // as a triggered action exception.
    let triggered = s.run("INSERT INTO L VALUES (1)").unwrap_err();
    assert_eq!(triggered.sqlstate().to_string(), "09000", "{triggered:?}");
    assert!(triggered.msg.contains("GR6e iii"), "{triggered:?}");
    s.commit().unwrap();
    let mut rows = s.open_cursor("SELECT K FROM T ORDER BY K").unwrap();
    let mut seen = Vec::new();
    while let Some(row) = rows.fetch().unwrap() {
        seen.push(row[0].to_string());
    }
    assert_eq!(seen, ["2"]);
}
