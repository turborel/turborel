//! SQL/PSM function bodies run where a query evaluates them [ISO/IEC
//! 9075-4:2006 draft §11.2]: savepoints, atomic compounds and local
//! cursors fetched by position, and the cursor states a body refuses.

use turborel::db::Database;

use crate::common::{rows, state};

/// Schema A: T (K, N), rows (1, 10), (2, 20), (3, 30).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §12.3] FETCH ABSOLUTE and
/// RELATIVE move a function's local cursor, which is scrollable, to the
/// row they name, its rows as it is opened.
#[test]
fn local_cursors_fetch_by_position_12_3() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE FUNCTION PICK (P INTEGER) RETURNS INTEGER READS SQL DATA \
         BEGIN DECLARE V, W INTEGER DEFAULT 0; DECLARE C SCROLL CURSOR FOR SELECT N FROM T WHERE N > P ORDER BY N; \
         OPEN C; FETCH ABSOLUTE 2 FROM C INTO V; FETCH RELATIVE -1 FROM C INTO W; CLOSE C; RETURN V * 100 + W; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT PICK(N - 10) FROM T WHERE K < 3 ORDER BY K").unwrap(), ["2010", "3020"]);
}

/// Verifies: [X3.135-1986 §8.8 GR1, §8.6 GR1, §8.1 GR1] in a function's
/// body, OPEN of a cursor already open, and FETCH and CLOSE of one not
/// open, raise SQLSTATE 24000 from the query.
#[test]
fn cursor_states_are_checked_8_8() {
    let db = defined();
    let a = db.session("A");
    for (n, statements) in ["OPEN C; OPEN C;", "FETCH C INTO V;", "CLOSE C;"].into_iter().enumerate() {
        a.run(&format!(
            "CREATE FUNCTION F{n} () RETURNS INTEGER READS SQL DATA \
             BEGIN DECLARE V INTEGER DEFAULT 0; DECLARE C CURSOR FOR SELECT N FROM T; {statements} RETURN V; END"
        ))
        .unwrap();
        assert_eq!(state(&a, &format!("SELECT F{n}() FROM T")), "24000", "{statements}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.5, §17.6 GR2, §17.8; ISO/IEC
/// 9075-4:2006 draft §14.1] a function's body establishes, releases and
/// rolls back to savepoints, and runs an atomic compound statement, its
/// value as without them; a savepoint not established cannot be
/// released, SQLSTATE 3B001.
#[test]
fn savepoints_and_atomic_compounds_in_a_function_17_5() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE FUNCTION KEPT (X INTEGER) RETURNS INTEGER CONTAINS SQL \
         BEGIN DECLARE R INTEGER DEFAULT 0; SAVEPOINT S1; SET R = X; SAVEPOINT S2; ROLLBACK TO SAVEPOINT S2; \
         RELEASE SAVEPOINT S1; B1: BEGIN ATOMIC SET R = R + 1; END B1; RETURN R; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT KEPT(N) FROM T ORDER BY K").unwrap(), ["11", "21", "31"]);
    a.run("CREATE FUNCTION NOT_HELD () RETURNS INTEGER CONTAINS SQL BEGIN RELEASE SAVEPOINT S9; RETURN 1; END").unwrap();
    assert_eq!(state(&a, "SELECT NOT_HELD() FROM T"), "3B001");
}
