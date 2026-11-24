//! Savepoints [ISO/IEC 9075-2:2023 §4.43.2, §17.5, §17.6, §17.8]
//! (Features T271, T272): SAVEPOINT establishes one, replacing one of the
//! same name; RELEASE SAVEPOINT destroys it and those after it; ROLLBACK
//! TO SAVEPOINT cancels every change made since, keeping it, while the
//! transaction goes on. The changes go back in every index, in temporary
//! tables and in large objects; cursors are closed and locators given
//! since are invalid. The modules below hold savepoint levels in routines
//! and triggers, and the changes of definitions a savepoint cancels after
//! a crash and on a standby.

use turborel::db::{Database, Session};
use turborel::error::{Result, SqlState};
use turborel::params::Params;
use turborel::value::Value;
use turborel::value::lob::Lob;

use crate::common::{dynamic_host, number, rows, run_with, set_text};

mod durability;
mod levels;

/// Schema A: T (K, V) with a UNIQUE K, rows 1 and 2 committed.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'one'), (2, 'two')").unwrap();
    a.commit().unwrap();
    db
}

/// The SQLSTATE of `r`'s error, or `"ok"`.
fn state<T>(r: Result<T>) -> String {
    r.err().map_or_else(|| "ok".to_owned(), |e| e.sqlstate().as_str().to_owned())
}

/// The values of `query` in `s`, trimmed, each row's joined by commas.
fn values(s: &Session, query: &str) -> Vec<String> {
    rows(s, query).unwrap().iter().map(|r| r.split(',').map(str::trim).collect::<Vec<_>>().join(",")).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5 GR2, GR4, §17.8 GR3d, GR3e]
/// savepoints nest: going back to an inner one cancels what came after
/// it alone, and to an outer one what came after that, destroying the
/// inner; SAVEPOINT of a name in use destroys the one it named and
/// establishes it here, so going back to it keeps what was done between.
#[test]
fn savepoints_nest_and_a_name_is_replaced_17_5() {
    let db = defined();
    let a = db.session("A");
    a.run("SAVEPOINT OUTER_SP").unwrap();
    a.run("INSERT INTO T VALUES (3, 'tri')").unwrap();
    a.run("SAVEPOINT INNER_SP").unwrap();
    a.run("INSERT INTO T VALUES (4, 'four')").unwrap();
    a.run("ROLLBACK TO SAVEPOINT INNER_SP").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2", "3"]);
    a.run("ROLLBACK TO SAVEPOINT OUTER_SP").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2"]);
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT INNER_SP")), "3B001", "destroyed with those after OUTER_SP");

    a.run("SAVEPOINT S").unwrap();
    a.run("UPDATE T SET V = 'uno' WHERE K = 1").unwrap();
    a.run("SAVEPOINT S").unwrap();
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(values(&a, "SELECT K, V FROM T ORDER BY K"), ["1,uno", "2,two"], "the second S kept the UPDATE");
    a.run("RELEASE SAVEPOINT S").unwrap();
    assert_eq!(state(a.run("RELEASE SAVEPOINT S")), "3B001", "one S only: the first was replaced");
    a.commit().unwrap();
    assert_eq!(values(&db.session("A"), "SELECT K, V FROM T ORDER BY K"), ["1,uno", "2,two"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.6 GR2, GR3] RELEASE SAVEPOINT
/// destroys the savepoint and every one established after it, but not
/// those before; what was changed since each stays, and commits.
#[test]
fn release_destroys_it_and_those_after_17_6() {
    let db = defined();
    let a = db.session("A");
    a.run("SAVEPOINT S1").unwrap();
    a.run("INSERT INTO T VALUES (3, 'tri')").unwrap();
    a.run("SAVEPOINT S2").unwrap();
    a.run("INSERT INTO T VALUES (4, 'four')").unwrap();
    a.run("SAVEPOINT S3").unwrap();
    a.run("RELEASE SAVEPOINT S2").unwrap();
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT S2")), "3B001");
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT S3")), "3B001", "established after S2");
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2", "3", "4"], "released, nothing undone");
    a.run("ROLLBACK TO SAVEPOINT S1").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2"], "S1 stays");
    a.run("INSERT INTO T VALUES (5, 'five')").unwrap();
    a.run("RELEASE SAVEPOINT S1").unwrap();
    a.commit().unwrap();
    assert_eq!(values(&db.session("A"), "SELECT K FROM T ORDER BY K"), ["1", "2", "5"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d, GR3e, NOTE 743] ROLLBACK TO
/// SAVEPOINT keeps the savepoint, so it is gone back to again; the
/// transaction goes on, ROLLBACK WORK and AND NO CHAIN spelled out too,
/// and a COMMIT after commits what was done before it and after it.
#[test]
fn rollback_to_keeps_the_savepoint_17_8() {
    let db = defined();
    let a = db.session("A");
    a.run("DELETE FROM T WHERE K = 1").unwrap();
    let tx = a.transaction();
    a.run("SAVEPOINT S").unwrap();
    for k in 3..6 {
        a.run(&format!("INSERT INTO T VALUES ({k}, 'n')")).unwrap();
        a.run("ROLLBACK WORK AND NO CHAIN TO SAVEPOINT S").unwrap();
    }
    a.run("UPDATE T SET V = 'dos' WHERE K = 2").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(a.transaction(), tx, "the transaction goes on");
    a.run("INSERT INTO T VALUES (6, 'six')").unwrap();
    a.commit().unwrap();
    assert_eq!(values(&db.session("A"), "SELECT K, V FROM T ORDER BY K"), ["2,two", "6,six"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 SR1, §4.41.2.4, §17.7 GR7, §17.8
/// GR2c, Table 41] the savepoint exceptions: RELEASE and ROLLBACK TO of a
/// name none has is 3B001, with no transaction active too; SAVEPOINT
/// initiates a transaction; COMMIT and ROLLBACK destroy every savepoint;
/// AND CHAIN with TO SAVEPOINT is a syntax error (42000); 3B001 is of
/// the class 3B000, and an SQLEXCEPTION handler takes it, its
/// RETURNED_SQLSTATE 3B001.
#[test]
fn savepoint_exceptions_17_8() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(state(a.run("RELEASE SAVEPOINT NONE")), "3B001");
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT NONE")), "3B001");
    assert_eq!(a.transaction(), None, "neither initiates one");
    a.run("SAVEPOINT S").unwrap();
    assert!(a.transaction().is_some(), "SAVEPOINT initiates one");
    a.commit().unwrap();
    assert_eq!(state(a.run("ROLLBACK TO SAVEPOINT S")), "3B001", "COMMIT destroyed it");
    a.run("SAVEPOINT S").unwrap();
    a.rollback().unwrap();
    assert_eq!(state(a.run("RELEASE SAVEPOINT S")), "3B001", "ROLLBACK destroyed it");
    a.run("SAVEPOINT S").unwrap();
    assert_eq!(state(a.run("ROLLBACK AND CHAIN TO SAVEPOINT S")), "42000");
    assert_eq!(state(a.run("SAVEPOINT")), "42000");
    let raised = a.run("RELEASE SAVEPOINT NONE").unwrap_err().sqlstate();
    assert_eq!(raised.class(), SqlState::SAVEPOINT_EXCEPTION.class());
    a.run(
        "CREATE PROCEDURE H (OUT C CHAR(5)) MODIFIES SQL DATA BEGIN \
         DECLARE EXIT HANDLER FOR SQLEXCEPTION GET DIAGNOSTICS CONDITION 1 C = RETURNED_SQLSTATE; \
         ROLLBACK TO SAVEPOINT NONE; END",
    )
    .unwrap();
    a.commit().unwrap();
    let mut p = dynamic_host();
    run_with(&a, "CALL H(TXT)", &mut p).unwrap();
    assert_eq!(p.value("TXT").to_string().trim(), "3B001");
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5 GR3] (IL208) a transaction has at
/// most 1024 savepoints: the 1025th is 3B002, the statement failing
/// alone; replacing a name in use is no more, and once one is released
/// there is room again.
#[test]
fn at_most_1024_savepoints_17_5() {
    let db = defined();
    let a = db.session("A");
    for n in 0..1024 {
        a.run(&format!("SAVEPOINT S{n}")).unwrap();
    }
    assert_eq!(state(a.run("SAVEPOINT MORE")), "3B002");
    a.run("SAVEPOINT S5").unwrap();
    a.run("RELEASE SAVEPOINT S1023").unwrap();
    a.run("SAVEPOINT MORE").unwrap();
    a.run("INSERT INTO T VALUES (3, 'tri')").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S0").unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2"]);
}

/// Schema A: T with a btree index on V, a hash index on W and an rtree
/// index on (X, Y); rows 1 and 2 committed.
fn indexed() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V CHAR(4), W INTEGER, \
         X DOUBLE PRECISION, Y DOUBLE PRECISION)",
    )
    .unwrap();
    db.create_index("T_V", "A.T", &["V"], "btree").unwrap();
    db.create_index("T_W", "A.T", &["W"], "hash").unwrap();
    db.create_index("T_XY", "A.T", &["X", "Y"], "rtree").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'one', 10, 1, 1), (2, 'two', 20, 2, 2)").unwrap();
    a.commit().unwrap();
    db
}

/// What each index of `indexed` finds of `a`'s rows, and how many entries
/// each holds.
fn by_index(db: &Database, a: &Session) -> (Vec<String>, Vec<usize>) {
    let found = [
        "SELECT K FROM T WHERE V = 'tri' ORDER BY K",
        "SELECT K FROM T WHERE W = 30 ORDER BY K",
        "SELECT K FROM T WHERE X BETWEEN 2.5 AND 3.5 AND Y BETWEEN 2.5 AND 3.5 ORDER BY K",
        "SELECT K FROM T WHERE V = 'one' ORDER BY K",
        "SELECT K FROM T WHERE K = 3 ORDER BY K",
    ]
    .iter()
    .map(|q| values(a, q).join(" "))
    .collect();
    (found, ["T_V", "T_W", "T_XY"].iter().map(|ix| db.index_entries(ix).unwrap()).collect())
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d] the rows inserted, updated
/// and deleted after a savepoint are cancelled in the table and in each
/// of its indexes, btree, hash and rtree, and its primary key's: queries
/// by each find what was there at the savepoint, a key freed again takes
/// a new row, and the indexes hold what they held.
#[test]
fn rollback_to_cancels_changes_in_every_index_17_8() {
    let db = indexed();
    let a = db.session("A");
    let before = by_index(&db, &a);
    a.run("SAVEPOINT S").unwrap();
    a.run("INSERT INTO T VALUES (3, 'tri', 30, 3, 3)").unwrap();
    a.run("UPDATE T SET V = 'tri', W = 30, X = 3, Y = 3 WHERE K = 1").unwrap();
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    assert_eq!(by_index(&db, &a).0, ["1 3", "1 3", "1 3", "", "3"]);
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(by_index(&db, &a), before);
    a.run("INSERT INTO T VALUES (3, 'tri', 30, 3, 3)").unwrap();
    a.commit().unwrap();
    assert_eq!(by_index(&db, &db.session("A")).0, ["3", "3", "3", "1", "3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d] a temporary table's rows
/// written after a savepoint are cancelled, those before staying; a
/// large object written after one is gone with its row, one before it
/// reads whole.
#[test]
fn rollback_to_cancels_temporary_rows_and_large_objects_17_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE B (K INTEGER, C CLOB) \
         CREATE GLOBAL TEMPORARY TABLE G (K INTEGER NOT NULL UNIQUE) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let a = db.session("A");
    let big: String = "savepoint ".repeat(70_000);
    let insert = a.prepare("INSERT INTO B VALUES (?, ?)").unwrap();
    a.execute_prepared(&insert, &[number(1), Value::Lob(Lob::of_text(&big))]).unwrap();
    a.run("INSERT INTO G VALUES (1)").unwrap();
    a.run("SAVEPOINT S").unwrap();
    a.execute_prepared(&insert, &[number(2), Value::Lob(Lob::of_text(&big))]).unwrap();
    a.run("UPDATE B SET C = 'short' WHERE K = 1").unwrap();
    a.run("INSERT INTO G VALUES (2)").unwrap();
    a.run("DELETE FROM G WHERE K = 1").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(values(&a, "SELECT K FROM G"), ["1"]);
    a.run("INSERT INTO G VALUES (2)").unwrap();
    a.commit().unwrap();
    assert_eq!(values(&a, "SELECT K, CHAR_LENGTH(C) FROM B"), ["1,700000"]);
    assert_eq!(values(&a, "SELECT K FROM G ORDER BY K"), ["1", "2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3f, GR3g, GR3h] (IA168) ROLLBACK
/// TO SAVEPOINT closes every open cursor, one opened before the savepoint
/// too, so a FETCH is 24000 and the cursor opens again; a locator given
/// after the savepoint is invalid (0F001), one given before stays valid.
#[test]
fn rollback_to_closes_cursors_and_invalidates_later_locators_17_8() {
    let db = defined();
    let a = db.session("A");
    a.declare_cursor("DECLARE BEFORE_SP CURSOR FOR SELECT K FROM T").unwrap();
    a.declare_cursor("DECLARE AFTER_SP CURSOR FOR SELECT K FROM T").unwrap();
    a.run("OPEN BEFORE_SP").unwrap();
    let kept = a.locator_of(Lob::of_text("kept"));
    a.run("SAVEPOINT S").unwrap();
    a.run("OPEN AFTER_SP").unwrap();
    let given = a.locator_of(Lob::of_text("given"));
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    let mut p = dynamic_host();
    assert_eq!(state(a.execute("FETCH BEFORE_SP INTO N", &mut p)), "24000");
    assert_eq!(state(a.execute("FETCH AFTER_SP INTO N", &mut p)), "24000");
    a.run("OPEN BEFORE_SP").unwrap();
    assert_eq!(state(a.read_lob(given, (0, 5), false)), "0F001");
    assert_eq!(a.read_lob(kept, (0, 4), false).unwrap(), b"kept");
}

/// The command codes GET DIAGNOSTICS gives of the statement `s` ran last.
fn command(s: &Session) -> String {
    let mut p = dynamic_host();
    s.execute("GET DIAGNOSTICS TXT = COMMAND_FUNCTION, N = COMMAND_FUNCTION_CODE", &mut p).unwrap();
    format!("{} {}", p.value("TXT").to_string().trim(), p.value("N"))
}

/// Verifies: [ISO/IEC 9075-2:2023 §23.1 GR2c, Table 39] the command codes
/// of the savepoint statements: SAVEPOINT 63, RELEASE SAVEPOINT 57, and
/// ROLLBACK WORK 62 of ROLLBACK TO SAVEPOINT; a failed one is coded too.
#[test]
fn savepoint_statements_are_coded_23_1() {
    let db = defined();
    let a = db.session("A");
    a.run("SAVEPOINT S").unwrap();
    assert_eq!(command(&a), "SAVEPOINT 63");
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(command(&a), "ROLLBACK WORK 62");
    a.run("RELEASE SAVEPOINT S").unwrap();
    assert_eq!(command(&a), "RELEASE SAVEPOINT 57");
    drop(a.run("RELEASE SAVEPOINT S"));
    assert_eq!(command(&a), "RELEASE SAVEPOINT 57");
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8 GR3i, GR3j] (IA167)
/// the savepoint statements prepared, by the program's interface and by
/// PREPARE, and by EXECUTE IMMEDIATE, run as written directly; ROLLBACK
/// TO deallocates no prepared statement, one prepared after the
/// savepoint executing still.
#[test]
fn savepoint_statements_prepared_17_8() {
    let db = defined();
    let a = db.session("A");
    let establish = a.prepare("SAVEPOINT P").unwrap();
    let back = a.prepare("ROLLBACK TO SAVEPOINT P").unwrap();
    let release = a.prepare("RELEASE SAVEPOINT P").unwrap();
    a.execute_prepared(&establish, &[]).unwrap();
    let insert = a.prepare("INSERT INTO T VALUES (?, 'p')").unwrap();
    a.execute_prepared(&insert, &[number(3)]).unwrap();
    a.execute_prepared(&back, &[]).unwrap();
    a.execute_prepared(&insert, &[number(4)]).unwrap();
    a.execute_prepared(&release, &[]).unwrap();
    assert_eq!(state(a.execute_prepared(&back, &[])), "3B001");

    let mut p = dynamic_host();
    set_text(&mut p, "SAVEPOINT D");
    run_with(&a, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run_with(&a, "EXECUTE Q", &mut p).unwrap();
    run_with(&a, "INSERT INTO T VALUES (5, 'd')", &mut Params::new()).unwrap();
    run_with(&a, "EXECUTE IMMEDIATE 'ROLLBACK TO SAVEPOINT D'", &mut p).unwrap();
    a.commit().unwrap();
    assert_eq!(values(&a, "SELECT K FROM T ORDER BY K"), ["1", "2", "4"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d, NOTE 743; §4.43.4] going
/// back to a savepoint cancels the changes but keeps the transaction and
/// what it holds: a table changed after the savepoint stays locked
/// against another transaction's change of its definition (SQLCODE
/// -911) until the transaction ends.
#[test]
fn locks_taken_after_a_savepoint_stay_held_17_8() {
    let db = defined();
    let a = db.session("A");
    let b = db.session("A");
    a.run("SAVEPOINT S").unwrap();
    a.run("ALTER TABLE T ADD COLUMN W INTEGER").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert_eq!(state(b.run("ALTER TABLE T ADD COLUMN Z INTEGER")), "TB911", "still locked");
    b.rollback().unwrap();
    a.commit().unwrap();
    b.run("ALTER TABLE T ADD COLUMN Z INTEGER").unwrap();
    b.commit().unwrap();
}

/// Verifies: [ISO/IEC 9075-11:2006 draft §6.40 Description 21, §5.49]
/// INFORMATION_SCHEMA.ROUTINES.NEW_SAVEPOINT_LEVEL: YES of a function and
/// of a procedure of NEW SAVEPOINT LEVEL, NO of one of OLD SAVEPOINT
/// LEVEL or of neither.
#[test]
fn routines_show_their_savepoint_level() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    a.run("CREATE PROCEDURE P_NEW () MODIFIES SQL DATA NEW SAVEPOINT LEVEL DELETE FROM T").unwrap();
    a.run("CREATE PROCEDURE P_OLD () MODIFIES SQL DATA OLD SAVEPOINT LEVEL DELETE FROM T").unwrap();
    a.run("CREATE PROCEDURE P_NONE () MODIFIES SQL DATA DELETE FROM T").unwrap();
    a.commit().unwrap();
    let shown = values(
        &a,
        "SELECT ROUTINE_NAME, NEW_SAVEPOINT_LEVEL FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME",
    );
    assert_eq!(shown, ["F,YES", "P_NEW,YES", "P_NONE,NO", "P_OLD,NO"]);
}
