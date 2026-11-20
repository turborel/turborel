//! \[DESIGN\] A procedure's body kept between the CALLs a session makes
//! of it (RULES.md D406): its text read once while it stays the same, its
//! program bound once and kept with what it was bound with, and each
//! INSERT, searched UPDATE and searched DELETE in it keeping its plan.

use turborel::db::{Database, Session};
use turborel::value::{Decimal, Value};

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

/// Verifies: [DESIGN] RULES.md D408, a query of a kept body that probes a
/// whole primary key with the procedure's parameters keeps its plan, and
/// each CALL reads the row its own values name: the warehouse and the
/// item each by its own parameter, a null item naming none.
#[test]
fn a_kept_leaf_reads_the_row_each_calls_values_name() {
    let (_db, a) = defined();
    // Rows enough that a probe of the key is chosen unpriced.
    let more: Vec<String> = (51..=600_i64).flat_map(|i| (1..=2_i64).map(move |w| format!("({w}, {i}, 0)"))).collect();
    a.run(&format!("INSERT INTO S VALUES {}", more.join(", "))).unwrap();
    a.run("UPDATE S SET Q = W * 1000 + I").unwrap();
    a.run(
        "CREATE PROCEDURE READ_ONE (IN WH INTEGER, IN ITEM INTEGER) MODIFIES SQL DATA
         BEGIN
           DECLARE GOT INTEGER;
           SET GOT = (SELECT Q FROM S WHERE W = WH AND I = ITEM);
           INSERT INTO L VALUES (WH, GOT);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    for (w, i) in [("1", "3"), ("2", "7"), ("1", "40"), ("2", "CAST(NULL AS INTEGER)")] {
        a.run(&format!("CALL READ_ONE({w}, {i})")).unwrap();
    }
    a.commit().unwrap();
    let mut read = rows(&a, "SELECT W, LOW FROM L");
    read.sort();
    assert_eq!(read, ["1,1003", "1,1040", "2,2007", "2,NULL"]);
}

/// Verifies: [DESIGN] RULES.md D414, a query of a kept body that reads a
/// range of rows learns its plan with slots over its first CALLs and
/// keeps it, and every CALL, before and after, reads the rows its own
/// values name: a count below a bound, and the least item at or above it.
#[test]
fn a_kept_leaf_learns_its_plan_and_reads_each_calls_rows() {
    let (_db, a) = defined();
    let more: Vec<String> = (51..=600_i64).flat_map(|i| (1..=2_i64).map(move |w| format!("({w}, {i}, 0)"))).collect();
    a.run(&format!("INSERT INTO S VALUES {}", more.join(", "))).unwrap();
    a.run("UPDATE S SET Q = MOD(I * 7, 100) + W").unwrap();
    a.run(
        "CREATE PROCEDURE RANGE_OF (IN WH INTEGER, IN BELOW INTEGER) MODIFIES SQL DATA
         BEGIN
           DECLARE N INTEGER;
           DECLARE LEAST INTEGER;
           SET N = (SELECT COUNT(*) FROM S WHERE W = WH AND Q < BELOW);
           SELECT MIN(I) INTO LEAST FROM S WHERE W = WH AND Q >= BELOW;
           INSERT INTO L VALUES (N, LEAST);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let calls: Vec<(i64, i64)> = (0..14_i64).map(|k| (1 + k % 2, (k * 37) % 103)).collect();
    let mut expected = Vec::new();
    for (w, below) in &calls {
        let n = rows(&a, &format!("SELECT COUNT(*) FROM S WHERE W = {w} AND Q < {below}"));
        let least = rows(&a, &format!("SELECT MIN(I) FROM S WHERE W = {w} AND Q >= {below}"));
        expected.push(format!("{},{}", n.join(""), least.join("")));
        a.run(&format!("CALL RANGE_OF({w}, {below})")).unwrap();
    }
    a.commit().unwrap();
    let mut read = rows(&a, "SELECT W, LOW FROM L");
    read.sort();
    expected.sort();
    assert_eq!(read, expected);
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §12.2, §14.13]; [DESIGN] RULES.md
/// D416, a local cursor and a FOR statement's cursor of a kept body, each
/// opened from the specification and plan it keeps, read the rows each
/// CALL's values name: before the stock changes, after it changes, and
/// after an index is made over what they read.
#[test]
fn a_kept_bodys_cursors_read_each_calls_rows() {
    let (_db, a) = defined();
    a.run("UPDATE S SET Q = MOD(I * 7, 30) + W").unwrap();
    a.run(
        "CREATE PROCEDURE LOWS (IN WH INTEGER, IN BELOW INTEGER) MODIFIES SQL DATA
         BEGIN
           DECLARE N INTEGER DEFAULT 0;
           DECLARE FIRST_I INTEGER;
           DECLARE ITEM INTEGER;
           DECLARE C CURSOR FOR SELECT I FROM S WHERE W = WH AND Q < BELOW ORDER BY I;
           SET N = (SELECT COUNT(*) FROM S WHERE W = WH AND Q < BELOW);
           IF N > 0 THEN
             OPEN C;
             FETCH C INTO FIRST_I;
             CLOSE C;
           END IF;
           SET N = 0;
           FOR R AS SELECT Q FROM S WHERE W = WH AND Q >= BELOW DO
             SET N = N + R.Q;
           END FOR;
           INSERT INTO L VALUES (FIRST_I, N);
         END",
    )
    .unwrap();
    a.commit().unwrap();
    let mut expected = Vec::new();
    for round in 0..3_i64 {
        if round == 1 {
            a.run("UPDATE S SET Q = Q + 3 WHERE MOD(I, 4) = 0").unwrap();
        }
        if round == 2 {
            a.commit().unwrap();
            a.run("CREATE INDEX S_WQ ON S (W, Q)").unwrap();
        }
        for (w, below) in (0..8_i64).map(|k| (1 + k % 2, 4 + (k * 11) % 27)) {
            let first = rows(&a, &format!("SELECT MIN(I) FROM S WHERE W = {w} AND Q < {below}"));
            let sum = rows(&a, &format!("SELECT COALESCE(SUM(Q), 0) FROM S WHERE W = {w} AND Q >= {below}"));
            expected.push(format!("{},{}", first.join(""), sum.join("")));
            a.run(&format!("CALL LOWS({w}, {below})")).unwrap();
        }
    }
    a.commit().unwrap();
    let mut read = rows(&a, "SELECT W, LOW FROM L");
    read.sort();
    expected.sort();
    assert_eq!(read, expected);
}

/// Verifies: [DESIGN] RULES.md D418, a query of a kept body that joins two
/// tables, a variable named for a column of each, learns its plan, its
/// variable one slot, and every CALL reads the row its own values name:
/// those whose values happen to be equal and those whose are not, before
/// and after it is learned; and a prepared join learns its plan alike.
#[test]
fn a_kept_join_learns_its_plan_and_reads_each_calls_row() {
    let (_db, a) = defined();
    a.run("CREATE TABLE WH (W INTEGER NOT NULL PRIMARY KEY, TAX INTEGER)").unwrap();
    a.run("INSERT INTO WH VALUES (1, 100), (2, 200)").unwrap();
    a.run(
        "CREATE PROCEDURE JOINED (IN WH_ID INTEGER, IN ITEM INTEGER) MODIFIES SQL DATA
         BEGIN
           DECLARE QT INTEGER;
           DECLARE T INTEGER;
           SELECT Q, TAX INTO QT, T FROM S, WH WHERE WH.W = WH_ID AND S.W = WH_ID AND I = ITEM;
           INSERT INTO L VALUES (QT, T);
         END",
    )
    .unwrap();
    a.run("UPDATE S SET Q = I * 10 + W").unwrap();
    a.commit().unwrap();
    let calls: Vec<(i64, i64)> = (0..16_i64).map(|k| (1 + k % 2, 1 + (k * 7) % 50)).chain([(1, 1), (2, 2), (2, 1)]).collect();
    let mut expected = Vec::new();
    for (w, i) in &calls {
        expected.push(format!("{},{}", i * 10 + w, w * 100));
        a.run(&format!("CALL JOINED({w}, {i})")).unwrap();
    }
    a.commit().unwrap();
    let mut read = rows(&a, "SELECT W, LOW FROM L");
    read.sort();
    expected.sort();
    assert_eq!(read, expected);
    let p = a.prepare("SELECT Q, TAX FROM S, WH WHERE WH.W = ? AND S.W = WH.W AND I = ?").unwrap();
    for (w, i) in calls {
        let mut c = a.open_prepared(&p, &[number(w), number(i)]).unwrap();
        let row = c.fetch().unwrap().unwrap();
        let shown: Vec<String> = row.iter().map(|v| v.to_string().trim_end().to_owned()).collect();
        assert_eq!(shown, vec![(i * 10 + w).to_string(), (w * 100).to_string()]);
    }
}

/// The exact number `n`.
fn number(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}
