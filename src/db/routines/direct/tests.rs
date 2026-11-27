//! Unit tests of a prepared CALL run straight (RULES.md D940, R-TEST-1).
//! Each script runs in two databases made alike, one with compiled
//! procedures ON, whose prepared CALLs run straight once kept, and one with
//! them OFF, whose every CALL runs as any does. What each statement gave
//! and each table as it ends are the same of both.

use std::sync::Arc;

use super::{BoundCall, Given};
use crate::db::{Database, Prepared, Session};
use crate::logical::{Expr, Slot};
use crate::params::conformed;
use crate::value::types::ArrayType;
use crate::value::{CharUnits, DataType, Decimal, Spelled, Value, assign};

const TABLES: &str = "CREATE SCHEMA AUTHORIZATION A
   CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, Q INTEGER CHECK (Q >= 0), NAME VARCHAR(5), AMOUNT DECIMAL(6,2))
   CREATE TABLE L (K INTEGER NOT NULL, N INTEGER NOT NULL, PRIMARY KEY (K, N))";

/// A database with `TABLES` and some rows, and A's session in it, compiled
/// procedures ON or OFF.
fn database(compiled: bool) -> (Database, Session) {
    let db = Database::new();
    db.schema(TABLES).unwrap();
    let a = session(&db, compiled);
    a.run("INSERT INTO T VALUES (1, 10, 'one', 1.50), (2, 20, 'two', 2.50), (3, 30, 'three', 3.50)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// A session of A in `db`, compiled procedures ON or OFF.
fn session(db: &Database, compiled: bool) -> Session {
    session_of(db, "A", compiled)
}

/// A session of `user` in `db`, compiled procedures ON or OFF.
fn session_of(db: &Database, user: &str, compiled: bool) -> Session {
    let a = db.session(user);
    a.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    a
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

/// `v` as it is shown whole, to compare values by.
fn shown(v: &Value) -> String {
    format!("{v:?}")
}

fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
}

/// What a statement gave: its completion and values, or its error whole.
fn outcome<T: std::fmt::Debug>(r: crate::error::Result<T>) -> String {
    match r {
        Ok(done) => format!("ok {done:?}"),
        Err(e) => format!("error {} {} {e:?}", e.sqlstate().as_str(), e.sqlcode()),
    }
}

/// What executing `p` with `values` gave in `s`, its outputs as text.
fn executed(s: &Session, p: &Prepared, values: &[Value]) -> String {
    outcome(s.execute_prepared_out(p, values).map(|(done, out)| (done, out.iter().map(ToString::to_string).collect::<Vec<_>>())))
}

/// The rows of `query` in `s`, each its values as text.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Both tables of `TABLES` as they are.
fn tables(s: &Session) -> Vec<String> {
    let mut all = rows(s, "SELECT K, Q, NAME, AMOUNT FROM T ORDER BY K");
    all.extend(rows(s, "SELECT K, N FROM L ORDER BY K, N"));
    all
}

/// How many prepared CALLs `s` keeps beside their bindings, how many of
/// them it runs straight, and how many hold a frame an execution left.
fn kept(s: &Session) -> (usize, usize, usize) {
    let inner = s.inner.borrow();
    let calls = &inner.routines.1.0;
    (calls.len(), calls.iter().filter(|c| c.running.is_some()).count(), calls.iter().filter(|c| c.frame.is_some()).count())
}

/// `setup` run, `sql` prepared, and executed with each of `values` in
/// turn, in a database compiled and in one not: what each execution gave
/// and the tables as they end, the same of both. What the compiled
/// session keeps beside the binding, as `kept` counts it.
fn same(setup: &[&str], sql: &str, values: &[Vec<Value>]) -> (Vec<String>, (usize, usize, usize)) {
    let mut seen = Vec::new();
    let mut straight = (0, 0, 0);
    for compiled in [false, true] {
        let (_db, a) = database(compiled);
        for s in setup {
            a.run(s).unwrap();
        }
        a.commit().unwrap();
        let p = a.prepare(sql).unwrap();
        let mut gave: Vec<String> = values.iter().map(|v| executed(&a, &p, v)).collect();
        gave.push(outcome(a.commit()));
        gave.extend(tables(&a));
        straight = kept(&a);
        seen.push(gave);
    }
    let compiled = seen.pop().unwrap();
    let interpreted = seen.pop().unwrap();
    assert_eq!(compiled, interpreted, "compiled and run straight, then as any CALL");
    (compiled, straight)
}

const MOVE: &str =
    "CREATE PROCEDURE MOVE (IN FROM_K INTEGER, IN TO_K INTEGER, IN N INTEGER, IN LABEL VARCHAR(5), INOUT TOTAL INTEGER, OUT LEFT_Q INTEGER)
     MODIFIES SQL DATA
     BEGIN
       DECLARE HAD INTEGER;
       SELECT Q INTO HAD FROM T WHERE K = FROM_K;
       UPDATE T SET Q = Q - N, NAME = LABEL WHERE K = FROM_K;
       UPDATE T SET Q = Q + N WHERE K = TO_K;
       SET TOTAL = TOTAL + N;
       SET LEFT_Q = HAD - N;
     END";

/// Verifies: [DESIGN] RULES.md D940: a prepared CALL whose arguments are
/// `?`s runs straight from its third execution on, the session keeping
/// its program and a frame beside its binding, and gives the output
/// values, the completions, the constraint's exception and the tables any
/// CALL gives; an execution that fails leaves the next to run straight.
#[test]
fn a_prepared_call_runs_straight_as_any_call_runs() {
    let values: Vec<Vec<Value>> = vec![
        vec![int(1), int(2), int(1), text("a"), int(100)],
        vec![int(2), int(3), int(2), text("b"), int(200)],
        vec![int(3), int(1), int(3), text("c"), int(300)],
        // The CHECK of Q fails: the body's exception, its statement undone.
        vec![int(1), int(2), int(500), text("d"), int(400)],
        vec![int(1), int(2), int(4), Value::Null, int(500)],
        vec![int(9), int(2), int(1), text("f"), Value::Null],
    ];
    let (gave, straight) = same(&[MOVE], "CALL MOVE(?, ?, ?, ?, ?, ?)", &values);
    assert_eq!(straight, (1, 1, 1), "kept, run straight, and its frame left for the next");
    assert!(gave[2].contains("\"303\"") && gave[2].contains("\"29\""), "the outputs of the third: {gave:?}");
    assert!(gave[3].starts_with("error 23"), "the CHECK: {gave:?}");
    assert!(gave[4].starts_with("ok"), "after a failure: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D940: an argument that is a constant is
/// given as the CALL was bound, and one that is an array `?` is cast to
/// its parameter's type whole, as any CALL gives them.
#[test]
fn constants_and_arrays_are_given_as_any_call_gives_them() {
    let setup = "CREATE PROCEDURE LINES (IN OF_K INTEGER, IN NS INTEGER ARRAY[4], IN STEP INTEGER, OUT MADE INTEGER) MODIFIES SQL DATA
       BEGIN
         DECLARE I INTEGER DEFAULT 1;
         WHILE I <= CARDINALITY(NS) DO
           INSERT INTO L VALUES (OF_K, NS[I] * STEP);
           SET I = I + 1;
         END WHILE;
         SET MADE = CARDINALITY(NS);
       END";
    let array = |ns: &[i64]| Value::Array(ns.iter().map(|n| int(*n)).collect::<Vec<_>>().into());
    let values: Vec<Vec<Value>> = vec![
        vec![int(1), array(&[1, 2])],
        vec![int(2), array(&[3])],
        vec![int(3), array(&[4, 5, 6, 7])],
        vec![int(4), array(&[])],
        // Past the maximum cardinality: the cast's exception, as any CALL's.
        vec![int(5), array(&[1, 2, 3, 4, 5])],
        vec![int(6), Value::Null],
        vec![int(7), array(&[8])],
    ];
    let (gave, straight) = same(&[setup], "CALL LINES(?, ?, 10, ?)", &values);
    assert_eq!(straight, (1, 1, 1));
    assert!(gave[2].contains("\"4\""), "{gave:?}");
    assert!(gave[4].starts_with("error"), "{gave:?}");
    assert!(gave.iter().any(|row| row == "3,70"), "{gave:?}");
}

/// Verifies: [DESIGN] RULES.md D940: an execution whose argument is cast
/// with a warning, or cannot be cast, runs as any CALL does: it gives the
/// value cut, or the cast's exception; the executions around it run
/// straight.
#[test]
fn an_argument_not_given_cleanly_runs_as_any_call() {
    let values: Vec<Vec<Value>> = vec![
        vec![int(1), int(2), int(1), text("a"), int(1)],
        vec![int(1), int(2), int(1), text("b"), int(1)],
        vec![int(1), int(2), int(1), text("c"), int(1)],
        // Cut to the parameter's five characters: string data, right truncation.
        vec![int(3), int(2), int(1), text("too long"), int(1)],
        // No number.
        vec![text("x"), int(2), int(1), text("d"), int(1)],
        vec![int(1), int(2), int(1), text("e"), int(1)],
    ];
    let (gave, straight) = same(&[MOVE], "CALL MOVE(?, ?, ?, ?, ?, ?)", &values);
    assert_eq!(straight, (1, 1, 1));
    assert!(gave.iter().any(|row| row.starts_with("3,29,too l,")), "cut to the type's length: {gave:?}");
    assert!(gave[4].starts_with("error 22"), "the cast's exception: {gave:?}");
    assert!(gave.iter().any(|row| row.starts_with("1,6,e")), "the last ran: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D940: a CALL with an argument that is
/// neither a `?` nor a constant keeps nothing to run straight with; one
/// whose procedure's body is no compound statement is kept as one that
/// runs as any CALL; both give what the interpreter gives. MC/DC of what
/// is kept: every argument plain or one not, the body compound or not.
#[test]
fn a_call_of_another_shape_is_never_run_straight() {
    let values: Vec<Vec<Value>> = (1..=4).map(|n| vec![int(n)]).collect();
    let single = "CREATE PROCEDURE ONE (IN OF_K INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES (OF_K, 1)";
    let (_, of_single) = same(&[single], "CALL ONE(?)", &values);
    assert_eq!(of_single, (1, 0, 0), "kept as one that never runs straight");
    let compound = "CREATE PROCEDURE ONE (IN OF_K INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (OF_K, 1); END";
    let (_, computed) = same(&[compound], "CALL ONE(? + 1)", &values);
    assert_eq!(computed, (0, 0, 0), "an argument computed");
    let (gave, reading) = same(&[compound], "CALL ONE((SELECT MAX(K) FROM T) + ?)", &values);
    assert_eq!(reading, (0, 0, 0), "an argument that reads a table");
    assert!(gave.iter().any(|row| row == "7,1"), "{gave:?}");
    let (_, straight) = same(&[compound], "CALL ONE(?)", &values);
    assert_eq!(straight, (1, 1, 1), "the same body, its argument a `?`");
}

/// Verifies: [DESIGN] RULES.md D940, D45, D406: once the procedure is
/// defined anew, the schema is set to another, or EXECUTE is revoked, the
/// CALL is bound again, and what the session kept beside the binding of
/// before is not found; it runs the new body, or fails as any CALL does.
#[test]
fn a_binding_bound_again_finds_nothing_kept() {
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let (db, a) = database(compiled);
        a.run("CREATE PROCEDURE NOTE (IN OF_K INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (OF_K, 1); END").unwrap();
        a.commit().unwrap();
        let p = a.prepare("CALL NOTE(?)").unwrap();
        let mut gave: Vec<String> = (1..=4).map(|n| executed(&a, &p, &[int(n)])).collect();
        a.run("DROP PROCEDURE NOTE RESTRICT").unwrap();
        a.run("CREATE PROCEDURE NOTE (IN OF_K INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (OF_K, 2); END").unwrap();
        a.run("GRANT EXECUTE ON PROCEDURE NOTE TO B").unwrap();
        gave.extend((5..=8).map(|n| executed(&a, &p, &[int(n)])));
        a.commit().unwrap();
        db.schema("CREATE SCHEMA C AUTHORIZATION A CREATE TABLE L (K INTEGER, N INTEGER)").unwrap();
        a.run("CREATE PROCEDURE C.NOTE (IN OF_K INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO C.L VALUES (OF_K, 3); END").unwrap();
        a.run("SET SCHEMA 'C'").unwrap();
        gave.extend((9..=11).map(|n| executed(&a, &p, &[int(n)])));
        a.run("SET SCHEMA 'A'").unwrap();
        gave.extend((12..=14).map(|n| executed(&a, &p, &[int(n)])));
        gave.push(outcome(a.commit()));
        let b = session_of(&db, "B", compiled);
        let q = b.prepare("CALL A.NOTE(?)").unwrap();
        gave.extend((20..=23).map(|n| executed(&b, &q, &[int(n)])));
        gave.push(outcome(b.commit()));
        a.run("REVOKE EXECUTE ON PROCEDURE NOTE FROM B RESTRICT").unwrap();
        a.commit().unwrap();
        gave.push(executed(&b, &q, &[int(24)]));
        gave.extend(tables(&a));
        gave.extend(rows(&a, "SELECT K, N FROM C.L ORDER BY K"));
        seen.push(gave);
    }
    assert_eq!(seen[1], seen[0], "compiled and run straight, then as any CALL");
    let gave = &seen[1];
    assert!(
        gave.iter().any(|row| row == "4,1") && gave.iter().any(|row| row == "8,2"),
        "the body of then, and the one made anew: {gave:?}"
    );
    assert!(gave.iter().any(|row| row == "11,3") && gave.iter().any(|row| row == "14,2"), "the schema's own: {gave:?}");
    assert!(gave.iter().any(|row| row == "23,2") && gave.iter().any(|g| g.starts_with("error 42000")), "EXECUTE revoked: {gave:?}");
}

/// Verifies: [DESIGN] RULES.md D940, [ISO/IEC 9075-2:2023 §4.41.5, §9.18
/// GR7]: an ATOMIC body run straight is undone whole if it fails, and a
/// body of NEW SAVEPOINT LEVEL runs at a level of its own, as any CALL's.
#[test]
fn atomic_bodies_and_savepoint_levels_are_kept() {
    let atomic = "CREATE PROCEDURE PAIR (IN A_K INTEGER, IN B_Q INTEGER) MODIFIES SQL DATA
       BEGIN ATOMIC
         INSERT INTO L VALUES (A_K, 1);
         UPDATE T SET Q = B_Q WHERE K = 1;
       END";
    let values: Vec<Vec<Value>> =
        vec![vec![int(1), int(5)], vec![int(2), int(6)], vec![int(3), int(-1)], vec![int(4), int(7)], vec![int(4), int(8)]];
    let (gave, straight) = same(&[atomic], "CALL PAIR(?, ?)", &values);
    assert_eq!(straight, (1, 1, 1));
    assert!(gave[2].starts_with("error 23") && !gave.iter().any(|row| row == "3,1"), "undone whole: {gave:?}");
    assert!(gave[4].starts_with("error 23") && gave.iter().any(|row| row == "4,1"), "the key of the fourth stays: {gave:?}");
    let level = "CREATE PROCEDURE SAVED (IN A_K INTEGER) MODIFIES SQL DATA NEW SAVEPOINT LEVEL
       BEGIN
         INSERT INTO L VALUES (A_K, 1);
         SAVEPOINT S;
         INSERT INTO L VALUES (A_K, 2);
         ROLLBACK TO SAVEPOINT S;
       END";
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let (_db, a) = database(compiled);
        a.run(level).unwrap();
        a.commit().unwrap();
        let p = a.prepare("CALL SAVED(?)").unwrap();
        let mut each: Vec<String> = (1..=4).map(|n| executed(&a, &p, &[int(n)])).collect();
        // Its savepoint was of its own level, destroyed as it returned.
        each.push(outcome(a.run("ROLLBACK TO SAVEPOINT S")));
        each.push(outcome(a.commit()));
        each.extend(tables(&a));
        each.push(format!("{:?}", kept(&a).1));
        seen.push(each);
    }
    assert_eq!(seen[1][..seen[1].len() - 1], seen[0][..seen[0].len() - 1], "compiled and run straight, then as any CALL");
    let levelled = &seen[1];
    assert!(levelled[4].starts_with("error 3B001"), "no savepoint of the caller's level: {levelled:?}");
    assert!(levelled.iter().any(|row| row == "4,1") && !levelled.iter().any(|row| row == "4,2"), "{levelled:?}");
    assert_eq!(levelled.last().map(String::as_str), Some("1"), "run straight");
}

/// Verifies: [DESIGN] RULES.md D940, D454: at READ COMMITTED a body run
/// straight reads at a snapshot taken as each statement begins, seeing
/// what another committed since; at SERIALIZABLE its reads are recorded,
/// so of two transactions each reading what the other writes one fails as
/// it commits, as of CALLs run as any are.
#[test]
fn isolation_levels_are_kept() {
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let mut gave = Vec::new();
        for level in ["READ COMMITTED", "SERIALIZABLE"] {
            let (db, a) = database(compiled);
            a.run(MOVE).unwrap();
            a.commit().unwrap();
            let b = session(&db, compiled);
            let (pa, pb) = (a.prepare("CALL MOVE(?, ?, ?, ?, ?, ?)").unwrap(), b.prepare("CALL MOVE(?, ?, ?, ?, ?, ?)").unwrap());
            // Each session's CALL kept and run straight before the levels meet.
            for (s, p) in [(&a, &pa), (&b, &pb)] {
                for _ in 0..3_u8 {
                    gave.push(executed(s, p, &[int(3), int(3), int(0), text("w"), int(0)]));
                    gave.push(outcome(s.commit()));
                }
                gave.push(format!("{:?}", kept(s).0));
                s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            }
            gave.push(executed(&a, &pa, &[int(1), int(2), int(1), text("a"), int(0)]));
            gave.push(outcome(b.run("UPDATE T SET Q = 77 WHERE K = 3")));
            gave.push(outcome(b.commit()));
            gave.push(outcome(b.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}"))));
            // A reads row 3 as B committed it, at READ COMMITTED alone.
            gave.push(executed(&a, &pa, &[int(3), int(1), int(2), text("b"), int(0)]));
            gave.push(executed(&b, &pb, &[int(1), int(3), int(2), text("c"), int(0)]));
            gave.push(outcome(a.commit()));
            gave.push(outcome(b.commit()));
            gave.extend(tables(&a));
        }
        seen.push(gave);
    }
    assert_eq!(seen[1], seen[0], "compiled and run straight, then as any CALL");
    assert!(seen[1].iter().any(|g| g.starts_with("error 40")), "one fails at SERIALIZABLE: {:?}", seen[1]);
}

/// Verifies: [DESIGN] RULES.md D940: `Given::of` takes a `?` of the
/// parameter's type and a constant the type takes, and nothing else. MC/DC
/// of each: a slot of the type or of another, a constant assignable or
/// not, another expression.
#[test]
fn only_a_slot_of_the_type_or_a_constant_is_given() {
    let slot = |data_type| Expr::Dynamic(Slot { index: 2, data_type, at: 0 });
    assert!(matches!(Given::of(&slot(integer()), &integer()), Some(Given::Slot { index: 2, settled: true })));
    let varying = DataType::Varying { length: 5, units: CharUnits::Characters };
    assert!(Given::of(&slot(varying.clone()), &integer()).is_none(), "a slot of another type");
    let Some(Given::Constant(v)) = Given::of(&Expr::Const(int(7)), &integer()) else { panic!("a constant of the type is given") };
    assert_eq!(shown(&v), shown(&int(7)));
    assert!(Given::of(&Expr::Const(text("too long")), &varying).is_none(), "a constant the type takes not");
    assert!(Given::of(&Expr::Neg(Box::new(Expr::Const(int(7)))), &integer()).is_none(), "an expression");
    let approximate = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
    assert!(matches!(Given::of(&slot(approximate.clone()), &approximate), Some(Given::Slot { settled: false, .. })));
}

/// Verifies: [DESIGN] RULES.md D940: a value cast with no warning to a
/// type `settled` names is assigned to that type unchanged, so a slot's
/// value is not assigned again; `Given::value` gives the value any CALL
/// assigns, and none where the cast warns or fails.
#[test]
fn a_value_cast_to_a_settled_type_is_assigned_unchanged() {
    let exact = DataType::Exact { precision: 6, scale: 2, spelled: Spelled::Decimal };
    let fixed = DataType::Character { length: 4, units: CharUnits::Characters };
    let varying = DataType::Varying { length: 4, units: CharUnits::Characters };
    let array = |element: &DataType| DataType::Array(Arc::new(ArrayType { element: element.clone(), max: 3 }));
    let list = |items: Vec<Value>| Value::Array(items.into());
    let cases: Vec<(DataType, Vec<Value>)> = vec![
        (integer(), vec![int(5), Value::Exact(Decimal::new(1250, 2)), text(" 42 "), Value::Null]),
        (exact.clone(), vec![int(5), Value::Exact(Decimal::new(12345, 3)), Value::Exact(Decimal::new(-999_999, 2)), text("1.5")]),
        (fixed.clone(), vec![text("ab"), text("abcd"), text(""), int(12)]),
        (varying.clone(), vec![text("ab"), text("abcd"), text("ab  "), int(12)]),
        (DataType::Boolean, vec![Value::Bool(true), text("FALSE")]),
        (array(&integer()), vec![list(vec![int(1), Value::Null, text("3")]), list(vec![]), Value::Null]),
        (array(&exact), vec![list(vec![int(1), Value::Exact(Decimal::new(15, 1))])]),
        (array(&varying), vec![list(vec![text("a"), text("abcd")])]),
        (array(&fixed), vec![list(vec![text("a")])]),
    ];
    for (to, values) in cases {
        assert!(super::super::call::settled(&to), "{to}");
        for v in values {
            let (cast, warning) = conformed(v.clone(), &to).unwrap();
            assert!(warning.is_none(), "{v} to {to}");
            assert_eq!(shown(&assign(cast.clone(), &to, "RULE").unwrap()), shown(&cast), "{v} cast to {to}, assigned again");
            let given = Given::Slot { index: 0, settled: true }.value(&to, std::slice::from_ref(&v));
            assert_eq!(given.as_ref().map(shown), Some(shown(&cast)), "{v} given to {to}");
        }
    }
    // A warning, an exception, or no value at the place: none given.
    let slot = Given::Slot { index: 0, settled: true };
    assert!(slot.value(&varying, &[text("too long")]).is_none(), "cut with a warning");
    assert!(slot.value(&integer(), &[text("x")]).is_none(), "no number");
    assert!(slot.value(&integer(), &[]).is_none(), "no value");
    let unsettled = Given::Slot { index: 0, settled: false };
    assert_eq!(unsettled.value(&integer(), &[text("7")]).as_ref().map(shown), Some(shown(&int(7))));
    assert_eq!(Given::Constant(int(3)).value(&integer(), &[]).as_ref().map(shown), Some(shown(&int(3))));
}

/// Verifies: [DESIGN] RULES.md D940: the session keeps what runs at most
/// `MOST_KEPT_CALLS` prepared CALLs straight, and one more forgets the
/// one kept first, which then runs as any CALL and is kept again.
#[test]
fn the_calls_kept_are_bounded() {
    let (_db, a) = database(true);
    a.run("CREATE PROCEDURE NOTE (IN OF_K INTEGER, IN OF_N INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO L VALUES (OF_K, OF_N); END")
        .unwrap();
    a.commit().unwrap();
    let most = super::MOST_KEPT_CALLS;
    let prepared: Vec<Prepared> = (0..=most).map(|n| a.prepare(&format!("CALL NOTE(?, {n})")).unwrap()).collect();
    for (n, p) in prepared.iter().enumerate() {
        for k in 0..3_usize {
            a.execute_prepared(p, &[int(i64::try_from(n * 10 + k).unwrap())]).unwrap();
        }
        assert_eq!(kept(&a).0, (n + 1).min(most));
    }
    let first: Arc<BoundCall> = Arc::clone(&a.inner.borrow().routines.1.0[0].bound);
    let Some([_, Given::Constant(v)]) = first.given.as_deref() else { panic!("the first kept gives a constant") };
    assert_eq!(shown(v), shown(&int(1)), "the first kept is now the second prepared");
    a.execute_prepared(&prepared[0], &[int(1000)]).unwrap();
    assert_eq!(kept(&a), (most, most, most - 1), "kept again, its frame not yet left");
    a.execute_prepared(&prepared[0], &[int(1001)]).unwrap();
    assert_eq!(kept(&a), (most, most, most), "run straight again");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM L"), vec![format!("{}", (most + 1) * 3 + 2)]);
}
