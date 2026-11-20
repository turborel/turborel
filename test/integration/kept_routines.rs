//! \[DESIGN\] A procedure's body kept between the CALLs a session makes
//! of it (RULES.md D406): its text read once while it stays the same, its
//! program bound once and kept with what it was bound with, and each
//! INSERT, searched UPDATE and searched DELETE in it keeping its plan.

use turborel::db::{Database, Session};

/// Schema A: S (W, I, Q), stock by warehouse and item, and H (W, I, WHEN_AT),
/// a history; and P, a procedure that takes `n` of an item from stock,
/// writes the history, and counts the warehouse's low items into L.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE S (W INTEGER NOT NULL, I INTEGER NOT NULL, Q INTEGER, PRIMARY KEY (W, I))
           CREATE TABLE H (W INTEGER, I INTEGER, WHEN_AT TIMESTAMP(6))
           CREATE TABLE L (W INTEGER, LOW INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let stock: Vec<String> = (1..=50_i64).flat_map(|i| (1..=2_i64).map(move |w| format!("({w}, {i}, 20)"))).collect();
    a.run(&format!("INSERT INTO S VALUES {}", stock.join(", "))).unwrap();
    a.run(PROCEDURE).unwrap();
    a.commit().unwrap();
    (db, a)
}

/// P's definition.
const PROCEDURE: &str = "CREATE PROCEDURE P (IN WH INTEGER, IN ITEM INTEGER, IN N INTEGER) MODIFIES SQL DATA
     BEGIN
       DECLARE LOW INTEGER;
       UPDATE S SET Q = Q - N WHERE W = WH AND I = ITEM;
       INSERT INTO H VALUES (WH, ITEM, CURRENT_TIMESTAMP(6));
       SET LOW = (SELECT COUNT(*) FROM S WHERE W = WH AND Q < 15);
       DELETE FROM L WHERE W = WH;
       INSERT INTO L VALUES (WH, LOW);
     END";

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Verifies: [DESIGN] RULES.md D406, a procedure CALLed again and again
/// with other arguments runs with each CALL's values: its kept program's
/// statements change the rows those values name, and its value read by a
/// query counts what they changed.
#[test]
fn a_kept_body_runs_with_each_calls_arguments() {
    let (_db, a) = defined();
    for (w, i, n) in [(1, 3, 10), (2, 3, 1), (1, 4, 6), (1, 3, 2), (2, 7, 9)] {
        a.run(&format!("CALL P({w}, {i}, {n})")).unwrap();
    }
    a.commit().unwrap();
    let changed = "SELECT W, I, Q FROM S WHERE Q <> 20 ORDER BY W, I";
    assert_eq!(rows(&a, changed), ["1,3,8", "1,4,14", "2,3,19", "2,7,11"]);
    assert_eq!(rows(&a, "SELECT W, LOW FROM L ORDER BY W"), ["1,2", "2,1"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM H"), ["5"]);
}

/// Verifies: [DESIGN] RULES.md D406, a body whose binding reads the
/// statement's instant is not kept: each CALL writes the instant of its
/// own.
#[test]
fn a_body_that_reads_the_instant_takes_each_calls() {
    let (db, a) = defined();
    a.run(
        "CREATE PROCEDURE STAMP () MODIFIES SQL DATA
         BEGIN
           DECLARE T TIMESTAMP(6);
           SET T = CURRENT_TIMESTAMP(6);
           INSERT INTO H VALUES (0, 0, T);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let b = db.session("A");
    for _ in 0..3 {
        b.run("CALL STAMP()").unwrap();
        b.commit().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT WHEN_AT) FROM H WHERE W = 0"), ["3"]);
}

/// Verifies: [DESIGN] RULES.md D406, a body kept is bound again once a
/// definition it was bound with changes: its table dropped, the procedure
/// with it, and both made again, the table's columns in another order and
/// the procedure of the same text, its CALL reads and writes the columns
/// of the table made again by their names.
#[test]
fn a_kept_body_is_bound_again_as_definitions_change() {
    let (_db, a) = defined();
    a.run("CALL P(1, 3, 10)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT W, LOW FROM L"), ["1,1"]);
    a.run("DROP TABLE S CASCADE").unwrap();
    a.run("CREATE TABLE S (Q INTEGER, I INTEGER NOT NULL, W INTEGER NOT NULL, PRIMARY KEY (W, I))").unwrap();
    a.run("INSERT INTO S VALUES (1, 1, 1), (2, 2, 1), (30, 3, 1)").unwrap();
    a.run(PROCEDURE).unwrap();
    a.commit().unwrap();
    a.run("CALL P(1, 3, 1)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT W, LOW FROM L"), ["1,2"]);
    assert_eq!(rows(&a, "SELECT Q FROM S WHERE I = 3"), ["29"]);
}
