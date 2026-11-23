//! \[DESIGN\] Escrow of increments through a delta table (RULES.md D849):
//! a compiled procedure's SELECT ... INTO of a FINAL TABLE of an UPDATE
//! that only adds to its columns reserves its increments, as a plain
//! UPDATE does (D452), when its select list shows none of the columns it
//! adds to. Each test compares it with the rows written and held: what
//! each session reads and what is left are the same, but for the waits
//! escrow takes away.

use std::thread;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{exec, rows};

/// The procedures of these cases. PAY adds to X and shows N, as Payment's
/// year to date shows the district's address; TAKE adds to N and shows
/// it, as New Order's next order number; SHOWN adds to X and shows it.
/// Each notes what it was shown in SEEN.
const PROCEDURES: [&str; 3] = [
    "CREATE PROCEDURE PAY (IN KK INTEGER, IN AMT DECIMAL(6,2)) MODIFIES SQL DATA BEGIN DECLARE NN INTEGER;
       SELECT N INTO NN FROM FINAL TABLE (UPDATE T SET X = X + AMT WHERE ID = KK); INSERT INTO SEEN VALUES ('PAY', NN); END",
    "CREATE PROCEDURE TAKE (IN KK INTEGER) MODIFIES SQL DATA BEGIN DECLARE NN INTEGER;
       SELECT N - 1 INTO NN FROM FINAL TABLE (UPDATE T SET N = N + 1 WHERE ID = KK);
       INSERT INTO SEEN VALUES ('ORDER' || CAST(KK AS VARCHAR(1)), NN); END",
    "CREATE PROCEDURE SHOWN (IN KK INTEGER, IN AMT DECIMAL(6,2)) MODIFIES SQL DATA BEGIN DECLARE XX DECIMAL(6,2);
       SELECT X INTO XX FROM FINAL TABLE (UPDATE T SET X = X + AMT WHERE ID = KK); INSERT INTO SEEN VALUES ('SHOWN', XX); END",
];

/// A database whose statements wait for locks, escrow as `escrow` says,
/// with table T of schema A: ID, X DECIMAL(6,2), Y INTEGER, N INTEGER,
/// rows 1 to 4 of X 100, Y 0 and N 0; table SEEN; and `PROCEDURES`.
fn database(escrow: &str) -> Database {
    let db = Database::new();
    db.set_setting("LOCKS.WAIT", &Value::Char("ON".to_owned())).unwrap();
    db.set_setting("TRANSACTION.ESCROW", &Value::Char(escrow.to_owned())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER NOT NULL, X DECIMAL(6,2), Y INTEGER, N INTEGER)
         CREATE TABLE SEEN (P VARCHAR(8), V DECIMAL(8,2))",
    )
    .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 100, 0, 0), (2, 100, 0, 0), (3, 100, 0, 0), (4, 100, 0, 0)").unwrap();
    for p in PROCEDURES {
        s.run(p).unwrap();
    }
    s.commit().unwrap();
    db
}

/// `s`, each of `PROCEDURES` called three times on row 4 and rolled
/// back, so the plans its session keeps settle and the compiled path runs
/// them from then on (D406). Its settings are set before, as a setting
/// changed binds them again.
fn warmed(s: Session) -> Session {
    for _ in 0..3 {
        for call in ["CALL PAY(4, 1)", "CALL TAKE(4)", "CALL SHOWN(4, 1)"] {
            s.run(call).unwrap();
        }
    }
    s.rollback().unwrap();
    s
}

/// A session of `db` that waits 100 milliseconds for a row at most.
fn impatient(db: &Database) -> Session {
    let s = db.session("A");
    s.run("SET LOCKS.TIMEOUT_MS = 100").unwrap();
    warmed(s)
}

/// Whether `sql` in a second session is refused for want of a row a first
/// session's `held` holds, its transaction open, with escrow as `escrow`
/// says and the first session's procedures compiled as `compiled` says.
fn holds((escrow, compiled): (&str, &str), held: &str, sql: &str) -> bool {
    let db = database(escrow);
    let owner = db.session("A");
    owner.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{compiled}'")).unwrap();
    let owner = warmed(owner);
    owner.run(held).unwrap_or_else(|e| panic!("{held}: {e}"));
    let other = impatient(&db);
    let refused = exec(&other, sql).err();
    other.rollback().unwrap();
    owner.rollback().unwrap();
    assert!(refused.is_none() || refused == Some(Code::Conflict), "{sql}: {refused:?}");
    refused.is_some()
}

/// Verifies: [DESIGN] RULES.md D849: an increment through a FINAL TABLE
/// whose select list shows another column holds no row: another
/// session's increment of the row goes on, through the same procedure or
/// a plain UPDATE, where the locked path waits. One that shows the column
/// it adds to, escrow turned off, or the procedure interpreted, holds it.
#[test]
fn an_increment_through_a_final_table_showing_other_columns_holds_no_row() {
    let pay = "CALL PAY(1, 5)";
    assert!(!holds(("ON", "ON"), pay, pay));
    assert!(!holds(("ON", "ON"), pay, "UPDATE T SET X = X + 1 WHERE ID = 1"));
    assert!(holds(("ON", "ON"), "CALL SHOWN(1, 5)", pay));
    assert!(holds(("ON", "ON"), "CALL TAKE(1)", "CALL TAKE(1)"));
    assert!(holds(("OFF", "ON"), pay, pay));
    assert!(holds(("ON", "OFF"), pay, pay));
}

/// Verifies: [DESIGN] RULES.md D849: a reservation through a FINAL TABLE
/// guards the columns its select list shows, as well as the column it adds
/// to and those its condition reads: another transaction's change of one
/// of them waits, as for a held row, so what it showed stays as it was
/// until its transaction ends; a change of another column goes on.
#[test]
fn a_reservation_through_a_final_table_guards_what_it_shows() {
    let pay = "CALL PAY(2, 5)";
    for (sql, waits) in [
        ("UPDATE T SET N = 7 WHERE ID = 2", true),
        ("UPDATE T SET X = 0 WHERE ID = 2", true),
        ("UPDATE T SET ID = 9 WHERE ID = 2", true),
        ("UPDATE T SET Y = 7 WHERE ID = 2", false),
        ("CALL TAKE(2)", true),
        ("CALL TAKE(3)", false),
    ] {
        assert_eq!(holds(("ON", "ON"), pay, sql), waits, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D849; [ISO/IEC 9075-2:2023 §7.6 GR5]: two
/// sessions paying into one row at once both commit; each FINAL TABLE
/// shows the row's other column as committed, and both sums are applied;
/// as the locked path, which waits between them.
#[test]
fn two_payments_into_one_row_both_apply() {
    for escrow in ["ON", "OFF"] {
        let db = database(escrow);
        let (a, b) = (warmed(db.session("A")), warmed(db.session("A")));
        a.run("UPDATE T SET N = 3 WHERE ID = 1").unwrap();
        a.commit().unwrap();
        a.run("CALL PAY(1, 5)").unwrap();
        let paid = thread::spawn(move || b.run("CALL PAY(1, 2.5)").and_then(|_| b.commit()).map_err(|e| e.msg));
        if escrow == "ON" {
            paid.join().unwrap().unwrap();
            a.commit().unwrap();
        } else {
            crate::common::until_waiting(&db, 1);
            a.commit().unwrap();
            paid.join().unwrap().unwrap();
        }
        let s = db.session("A");
        assert_eq!(rows(&s, "SELECT X, N FROM T WHERE ID = 1").unwrap(), ["107.50,3"], "{escrow}");
        assert_eq!(rows(&s, "SELECT P, V FROM SEEN ORDER BY P").unwrap(), ["PAY,3.00", "PAY,3.00"], "{escrow}");
    }
}

/// Twenty transactions of session `s`, the `t`th of many, each a payment
/// into a row of T and an order of a row's next number, run again until
/// it commits.
fn trading(s: &Session, t: usize) {
    for n in 0..20_usize {
        let id = ((n >> 1_u32) + t) % 2 + 1;
        let sql = if (n + t).is_multiple_of(2) { format!("CALL PAY({id}, 1.25)") } else { format!("CALL TAKE({id})") };
        while s.run(&sql).and_then(|_| s.commit()).is_err() {
            s.rollback().unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D849; [ISO/IEC 9075:1992 §4.28]: sixteen
/// sessions paying into two rows and taking their next numbers at once,
/// each committing every change, lose no payment, and give each number
/// once, with no gap: as the rows written and held do.
#[test]
fn many_payments_and_orders_lose_no_update() {
    let mut left = Vec::new();
    for escrow in ["ON", "OFF"] {
        let db = database(escrow);
        let threads: Vec<_> = (0..16)
            .map(|t| {
                let db = db.clone();
                thread::spawn(move || trading(&warmed(db.session("A")), t))
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = db.session("A");
        let numbers = "SELECT P, COUNT(*), COUNT(DISTINCT V), MIN(V), MAX(V) FROM SEEN WHERE P LIKE 'ORDER_' GROUP BY P ORDER BY P";
        left.push((rows(&s, "SELECT ID, X, N FROM T WHERE ID < 3 ORDER BY ID").unwrap(), rows(&s, numbers).unwrap()));
    }
    assert_eq!(left[0], left[1], "escrow on, then off");
    assert_eq!(left[0].0, ["1,200.00,80", "2,200.00,80"]);
    assert_eq!(left[0].1, ["ORDER1,80,80,0.00,79.00", "ORDER2,80,80,0.00,79.00"]);
}
