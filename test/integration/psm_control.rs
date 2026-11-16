//! SQL/PSM control statements, their labels and assignments, functions
//! that possibly modify SQL-data where Part 2 places them, and routine
//! bodies across reopening [ISO/IEC 9075-4:2006 draft §14.5 to §14.12]
//! (Features P002, P004, P005, P006, P008).

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Scratch, failure};

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

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.6 GR2, GR3, §14.7]
/// A simple CASE statement compares row operands (P004) and lists of
/// operands (P008); a searched one and IF with ELSEIF choose the first
/// true; a CASE with no ELSE whose WHEN all fail raises SQLSTATE 20000.
#[test]
fn case_and_if_statements_14_6() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "BEGIN DECLARE X INTEGER DEFAULT 3; \
         CASE (X, 1) WHEN (2, 1) THEN INSERT INTO T VALUES (1, 0); WHEN (3, 1) THEN INSERT INTO T VALUES (1, 1); END CASE; \
         CASE X WHEN 1, 2 THEN INSERT INTO T VALUES (2, 0); WHEN 4, 3 THEN INSERT INTO T VALUES (2, 1); ELSE INSERT INTO T VALUES (2, 2); END CASE; \
         CASE WHEN X > 5 THEN INSERT INTO T VALUES (3, 0); ELSE INSERT INTO T VALUES (3, 1); END CASE; \
         IF X = 1 THEN INSERT INTO T VALUES (4, 0); ELSEIF X = 3 THEN INSERT INTO T VALUES (4, 1); ELSE INSERT INTO T VALUES (4, 2); END IF; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,1", "2,1", "3,1", "4,1"]);
    assert_eq!(failure(a.run("BEGIN CASE 1 WHEN 2 THEN SET 1 = 1; END CASE; END")).0, Code::Syntax);
    assert_eq!(failure(a.run("BEGIN DECLARE X INTEGER DEFAULT 1; CASE X WHEN 2 THEN SET X = 3; END CASE; END")).1, "20000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.8 to §14.12, §14.1 SR3]
/// LEAVE leaves the labelled statement it names, through a compound
/// statement in it too, ITERATE begins its iteration again, REPEAT runs
/// its statements before testing its condition each time; an ending
/// label other than the beginning one is refused, as LEAVE of no label in
/// scope and ITERATE of a compound statement are.
#[test]
fn labels_leave_and_iterate_14_9() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "BEGIN DECLARE I INTEGER DEFAULT 0; \
         ODD: LOOP SET I = I + 1; IF I IN (2, 4, 6, 8) THEN ITERATE ODD; END IF; \
         IF I > 7 THEN LEAVE ODD; END IF; INSERT INTO T VALUES (I, 0); END LOOP ODD; \
         REPEAT SET I = I + 1; INSERT INTO T VALUES (100 + I, 0); UNTIL I >= 11 END REPEAT; \
         O: BEGIN BEGIN LEAVE O; END; INSERT INTO T VALUES (200, 0); END O; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,0", "3,0", "5,0", "7,0", "110,0", "111,0"]);
    for refused in ["BEGIN L: LOOP LEAVE L; END LOOP M; END", "BEGIN LEAVE NOWHERE; END", "B: BEGIN ITERATE B; END B"] {
        assert_eq!(failure(a.run(refused)).0, Code::SyntaxRule, "{refused}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §6.2 SR3 to SR7, §14.5 GR1, §14.4]
/// An inner block's variable hides an outer one's of its name, which its
/// label still names (P005); a multiple assignment assigns the fields of a
/// row, of a subquery of one row too (P006), its targets null of no row
/// and SQLSTATE 21000 of more than one.
#[test]
fn scopes_and_multiple_assignment_14_5() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run(
        "BEGIN DECLARE P, Q INTEGER; \
         O: BEGIN DECLARE X INTEGER DEFAULT 1; \
         I: BEGIN DECLARE X INTEGER DEFAULT 2; SET O.X = O.X + X * 10; END I; \
         SET (P, Q) = (X, X + 1); INSERT INTO T VALUES (P, Q); \
         SET (P, Q) = (SELECT K, N FROM T WHERE K = 2); INSERT INTO T VALUES (P + 100, Q); \
         SET (P, Q) = (SELECT K, N FROM T WHERE K = 0); INSERT INTO T VALUES (300, COALESCE(P, Q, -1)); END O; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T WHERE K > 2 ORDER BY K"), ["21,22", "102,20", "300,-1"]);
    assert_eq!(failure(a.run("BEGIN DECLARE P, Q INTEGER; SET (P, Q) = (SELECT K, N FROM T); END")).1, "21000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.9 SR6, §11.49 SR11, §14.12 SR13, §4.35.2]
/// A function that possibly modifies SQL-data is invoked in a MERGE's SET
/// and VALUES; refused in a CHECK, a trigger's WHEN and a MERGE's ON.
#[test]
fn a_modifying_function_in_merge_and_conditions_4_35_2() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE L (V INTEGER)").unwrap();
    a.run("CREATE FUNCTION LOGGED (D INTEGER) RETURNS INTEGER MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (D); RETURN D; END").unwrap();
    a.run("INSERT INTO T VALUES (1, 1)").unwrap();
    a.run(
        "MERGE INTO T USING (VALUES (1), (2)) AS S (K) ON T.K = S.K \
         WHEN MATCHED THEN UPDATE SET N = LOGGED(S.K * 10) WHEN NOT MATCHED THEN INSERT VALUES (S.K, LOGGED(S.K * 100))",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,10", "2,200"]);
    assert_eq!(rows(&a, "SELECT V FROM L ORDER BY V"), ["10", "200"]);
    for refused in [
        "CREATE TABLE C (V INTEGER CHECK (LOGGED(V) > 0))",
        "CREATE TRIGGER G AFTER INSERT ON T REFERENCING NEW ROW AS R FOR EACH ROW WHEN (LOGGED(R.K) > 0) INSERT INTO L VALUES (0)",
        "MERGE INTO T USING (VALUES (1)) AS S (K) ON LOGGED(S.K) = T.K WHEN MATCHED THEN DELETE",
    ] {
        assert_eq!(failure(a.run(refused)).0, Code::SyntaxRule, "{refused}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 GR6, §15.19; RULES.md D71]
/// A procedure whose compound body inserts into a table whose trigger
/// calls it again nests until the nesting bound refuses the next CALL,
/// every change of the statement undone with it.
#[test]
fn nesting_is_bounded_9_18() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE R (IN D INTEGER) MODIFIES SQL DATA BEGIN DECLARE E INTEGER; SET E = D; INSERT INTO T VALUES (E, 0); END")
        .unwrap();
    a.run(
        "CREATE TRIGGER G AFTER INSERT ON T REFERENCING NEW ROW AS NR FOR EACH ROW WHEN (NR.K > 0) \
         BEGIN ATOMIC CALL R(NR.K - 1); END",
    )
    .unwrap();
    a.run("CALL R(3)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["0", "1", "2", "3"]);
    assert_eq!(failure(a.run("CALL R(1000)")).0, Code::Unsupported);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T"), ["4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 GR1; ISO/IEC 9075-4:2006 draft §14.1]
/// A routine and a trigger whose bodies are compound statements are kept
/// across reopening, by the logged definition and by the snapshot.
#[test]
fn compound_bodies_are_kept_across_reopening_14_1() {
    let scratch = Scratch::new("psm-reopen");
    let dir = scratch.path("db");
    let check = |db: &Database| {
        let a = db.session("A");
        a.run("CALL ADD2(5)").unwrap();
        assert_eq!(rows(&a, "SELECT K, N FROM T WHERE K = 5"), ["5,3"]);
        a.run("DELETE FROM T").unwrap();
        a.commit().unwrap();
    };
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
        let a = db.session("A");
        a.run("CREATE PROCEDURE ADD2 (IN X INTEGER) MODIFIES SQL DATA BEGIN DECLARE Y INTEGER DEFAULT 1; INSERT INTO T VALUES (X, Y); END")
            .unwrap();
        a.run(
            "CREATE TRIGGER G BEFORE INSERT ON T REFERENCING NEW ROW AS NR FOR EACH ROW \
             BEGIN ATOMIC DECLARE D INTEGER DEFAULT 1; WHILE D < 3 DO SET NR.N = NR.N + 1; SET D = D + 1; END WHILE; END",
        )
        .unwrap();
        a.commit().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        check(&db);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    check(&db);
    db.close().unwrap();
}

/// Verifies: [DESIGN] RULES.md D44; [ISO/IEC 9075-4:2006 draft §14.11]
/// A WHILE loop that never ends is cancelled as a statement is: it fails
/// with SQLCODE -920, its changes undone, and the session goes on.
#[test]
fn an_endless_loop_is_cancelled_14_11() {
    let db = defined();
    let a = db.session("A");
    let canceller = a.canceller();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    // Cancelled again and again, so a cancel before the loop begins is
    // not the only one.
    let cancelling = std::thread::spawn(move || {
        while finished.recv_timeout(std::time::Duration::from_millis(10)).is_err() {
            canceller.cancel();
        }
    });
    let e = a.run("BEGIN ATOMIC DECLARE X INTEGER DEFAULT 0; INSERT INTO T VALUES (1, 1); WHILE TRUE DO SET X = 0; END WHILE; END");
    done.send(()).unwrap();
    cancelling.join().unwrap();
    assert_eq!(e.unwrap_err().sqlcode(), -920);
    assert_eq!(rows(&a, "SELECT K FROM T"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 GR4, GR5, §12.2]
/// A compound statement's cursors are destroyed as it ends, so each
/// iteration of a loop around it declares and opens its cursor anew.
#[test]
fn a_blocks_cursors_end_with_it_14_1() {
    let db = defined();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    a.run(
        "BEGIN DECLARE I, X INTEGER DEFAULT 0; \
         WHILE I < 3 DO BEGIN DECLARE C CURSOR FOR SELECT N FROM T WHERE K = 1; OPEN C; FETCH C INTO X; END; \
         SET I = I + 1; INSERT INTO T VALUES (I + 1, X + I); END WHILE; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T ORDER BY K"), ["1,10", "2,11", "3,12", "4,13"]);
}
