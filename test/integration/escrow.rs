//! \[DESIGN\] Escrow of increments (RULES.md D452): a READ COMMITTED
//! searched UPDATE that only adds to exact numeric columns reserves its
//! increments rather than holding its rows, and its transaction applies
//! them as it commits. Each test compares it with the rows written and
//! held, TRANSACTION.ESCROW OFF: what each session reads, what it waits
//! for and what it is refused, and what is left, are the same, but for
//! the waits escrow takes away.

use std::thread;
use std::time::Duration;

use turborel::db::{Database, Opened, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{Scratch, exec, reopened_after_crash, rows};

/// The settings compared: escrow on, then off.
const BOTH: [&str; 2] = ["ON", "OFF"];

/// A database whose statements wait for locks, escrow as `escrow` says,
/// with table T of schema A: ID, X DECIMAL(6,2), Y INTEGER, N INTEGER,
/// rows 1 to 4 of X 100, Y 0 and N 0.
fn database(escrow: &str) -> Database {
    let db = Database::new();
    db.set_setting("LOCKS.WAIT", &Value::Char("ON".to_owned())).unwrap();
    db.set_setting("TRANSACTION.ESCROW", &Value::Char(escrow.to_owned())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER NOT NULL, X DECIMAL(6,2), Y INTEGER, N INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 100, 0, 0), (2, 100, 0, 0), (3, 100, 0, 0), (4, 100, 0, 0)").unwrap();
    s.commit().unwrap();
    db
}

/// A session of `db` that waits `ms` milliseconds for a row at most.
fn impatient(db: &Database, ms: u32) -> Session {
    let s = db.session("A");
    s.run(&format!("SET LOCKS.TIMEOUT_MS = {ms}")).unwrap();
    s
}

/// T's rows, by ID.
fn table(s: &Session) -> Vec<String> {
    rows(s, "SELECT ID, X, Y, N FROM T ORDER BY ID").unwrap()
}

/// One of T's columns of the row of ID `id`, as `s` reads it.
fn value(s: &Session, column: &str, id: i32) -> String {
    rows(s, &format!("SELECT {column} FROM T WHERE ID = {id}")).unwrap().concat()
}

/// Whether `sql` in a second session is refused for want of a row a
/// first session's `held` holds, its transaction open, with escrow as
/// `escrow` says, after `setup` runs: whether `held` held its rows.
fn holds(escrow: &str, setup: &[&str], (held, sql): (&str, &str)) -> bool {
    let db = database(escrow);
    let owner = db.session("A");
    for one in setup {
        owner.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    owner.commit().unwrap();
    owner.run(held).unwrap_or_else(|e| panic!("{held}: {e}"));
    let other = impatient(&db, 100);
    let refused = exec(&other, sql).err();
    other.rollback().unwrap();
    owner.rollback().unwrap();
    assert!(refused.is_none() || refused == Some(Code::Conflict), "{sql}: {refused:?}");
    refused.is_some()
}

/// Thirty transactions of session `s`, the `t`th of many, each committing
/// one increment of a row of T, or a decrement, run again until it commits.
fn adding(s: &Session, t: usize) {
    for n in 0..30 {
        let id = (n + t) % 4 + 1;
        let sql = match n % 3 {
            0 => format!("UPDATE T SET X = X + 1.25, Y = Y - 2 WHERE ID = {id}"),
            1 => format!("UPDATE T SET X = X - 0.5 WHERE ID = {id}"),
            _ => format!("UPDATE T SET N = N + 1 WHERE ID = {id} AND Y <= 0"),
        };
        while s.run(&sql).and_then(|_| s.commit()).is_err() {
            s.rollback().unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D452: sixteen sessions adding to and
/// subtracting from T's rows at once, each committing every change, leave
/// the sums the rows written and held leave.
#[test]
fn many_sessions_adding_leave_the_sums_of_the_locked_path() {
    let mut left = Vec::new();
    for escrow in BOTH {
        let db = database(escrow);
        let threads: Vec<_> = (0..16)
            .map(|t| {
                let db = db.clone();
                thread::spawn(move || adding(&db.session("A"), t))
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = db.session("A");
        left.push((table(&s), rows(&s, "SELECT SUM(X), SUM(Y), SUM(N) FROM T").unwrap()));
    }
    assert_eq!(left[0], left[1], "escrow on, then off");
    assert_eq!(left[0].1, ["520.00,-320,160"]);
}

/// Verifies: [DESIGN] RULES.md D452: sessions adding to a row while
/// others write the column they add to, each holding the row a moment
/// before it commits, are never refused: a write never waits for a
/// reservation while it holds the row, so no wait closes a cycle; and
/// every increment is applied.
#[test]
fn increments_and_writes_of_one_column_never_deadlock() {
    let db = database("ON");
    let threads: Vec<_> = (0..24)
        .map(|t| {
            let s = db.session("A");
            let sql = if t % 3 == 0 { "UPDATE T SET X = X * 1 WHERE ID = 1" } else { "UPDATE T SET X = X + 1 WHERE ID = 1" };
            thread::spawn(move || {
                for _ in 0..20 {
                    s.run(sql).map_err(|e| e.msg)?;
                    thread::sleep(Duration::from_millis(1));
                    s.commit().map_err(|e| e.msg)?;
                }
                Ok::<(), String>(())
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), Ok(()));
    }
    assert_eq!(value(&db.session("A"), "X", 1), "420.00");
}

/// Verifies: [DESIGN] RULES.md D452: an increment reserved holds no row:
/// another session's increment of the row goes on, where the locked path
/// waits; both are applied.
#[test]
fn an_increment_holds_no_row() {
    let increment = "UPDATE T SET X = X + 1 WHERE ID = 1";
    assert!(!holds("ON", &[], (increment, increment)));
    assert!(holds("OFF", &[], (increment, increment)));
    let db = database("ON");
    let (a, b) = (db.session("A"), impatient(&db, 100));
    a.run(increment).unwrap();
    b.run(increment).unwrap();
    b.commit().unwrap();
    a.commit().unwrap();
    assert_eq!(value(&a, "X", 1), "102.00");
}

/// Verifies: [DESIGN] RULES.md D452: a session reads its own increments,
/// in any column it reads, and another reads only those committed; a
/// read of other columns leaves them reserved.
#[test]
fn a_session_reads_its_own_increments_and_no_other() {
    for escrow in BOTH {
        let db = database(escrow);
        let (a, b) = (db.session("A"), db.session("A"));
        a.run("UPDATE T SET X = X + 5, Y = Y + 1 WHERE ID = 2").unwrap();
        assert_eq!(value(&a, "N", 2), "0", "{escrow}");
        assert_eq!(value(&a, "X - 1", 2), "104.00", "{escrow}");
        assert_eq!(rows(&a, "SELECT SUM(Y) FROM T").unwrap(), ["1"], "{escrow}");
        assert_eq!(value(&b, "X", 2), "100.00", "{escrow}");
        a.run("UPDATE T SET X = X + 5 WHERE ID = 2").unwrap();
        assert_eq!(value(&a, "X", 2), "110.00", "{escrow}");
        assert_eq!(value(&b, "X", 2), "100.00", "{escrow}");
        a.commit().unwrap();
        assert_eq!(value(&b, "X", 2), "110.00", "{escrow}");
    }
}

/// Verifies: [DESIGN] RULES.md D452: while an increment is reserved,
/// another transaction's change of its column, of a column its condition
/// reads, or deletion of its row waits for it, as for a held row; a
/// change of another column goes on, and both are applied.
#[test]
fn a_reservation_guards_its_column_and_its_condition() {
    let increment = "UPDATE T SET X = X + 1 WHERE ID = 3";
    for (sql, waits) in [
        ("UPDATE T SET X = 0 WHERE ID = 3", true),
        ("UPDATE T SET ID = 9 WHERE ID = 3", true),
        ("DELETE FROM T WHERE ID = 3", true),
        ("UPDATE T SET N = 7 WHERE ID = 3", false),
        ("UPDATE T SET X = 0 WHERE ID = 4", false),
    ] {
        assert_eq!(holds("ON", &[], (increment, sql)), waits, "{sql}");
        assert!(holds("OFF", &[], (increment, sql)) || sql.ends_with("ID = 4"), "{sql}");
    }
    let db = database("ON");
    let (a, b) = (db.session("A"), db.session("A"));
    a.run(increment).unwrap();
    b.run("UPDATE T SET N = 7 WHERE ID = 3").unwrap();
    b.commit().unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT X, N FROM T WHERE ID = 3").unwrap(), ["101.00,7"]);
}

/// Verifies: [DESIGN] RULES.md D452; [ISO/IEC 9075:1992 §9.2 GR3b]: an
/// increment that would leave its column's type, with those reserved,
/// raises its error at its own statement, once the reservations it
/// waits for are applied, as the locked path does.
#[test]
fn a_sum_out_of_its_type_raises_at_its_statement() {
    for escrow in BOTH {
        let db = database(escrow);
        let a = db.session("A");
        a.run("UPDATE T SET X = 9990 WHERE ID = 1").unwrap();
        a.commit().unwrap();
        a.run("UPDATE T SET X = X + 5 WHERE ID = 1").unwrap();
        let other = db.clone();
        let late = thread::spawn(move || {
            let b = other.session("A");
            let refused = exec(&b, "UPDATE T SET X = X + 5 WHERE ID = 1").err();
            b.rollback().unwrap();
            refused
        });
        thread::sleep(Duration::from_millis(100));
        a.commit().unwrap();
        assert_eq!(late.join().unwrap(), Some(Code::Data), "{escrow}");
        assert_eq!(exec(&a, "UPDATE T SET X = X + 10 WHERE ID = 1").err(), Some(Code::Data), "{escrow}");
        a.rollback().unwrap();
        assert_eq!(value(&a, "X", 1), "9995.00", "{escrow}");
    }
}

/// Verifies: [DESIGN] RULES.md D452; [ISO/IEC 9075:1992 §4.28]: a
/// rollback takes its increments back, a rollback to a savepoint those
/// since it, whether read meanwhile or not, and a statement that fails
/// after reading them leaves them reserved.
#[test]
fn rollbacks_take_increments_back() {
    for escrow in BOTH {
        let db = database(escrow);
        let a = db.session("A");
        a.run("UPDATE T SET X = X + 1 WHERE ID = 1").unwrap();
        a.rollback().unwrap();
        a.run("UPDATE T SET X = X + 1 WHERE ID = 1").unwrap();
        a.run("SAVEPOINT S").unwrap();
        a.run("UPDATE T SET X = X + 10 WHERE ID = 1").unwrap();
        a.run("ROLLBACK TO SAVEPOINT S").unwrap();
        a.run("SAVEPOINT R").unwrap();
        a.run("UPDATE T SET X = X + 20 WHERE ID = 1").unwrap();
        assert_eq!(value(&a, "X", 1), "121.00", "{escrow}");
        a.run("ROLLBACK TO SAVEPOINT R").unwrap();
        assert_eq!(value(&a, "X", 1), "101.00", "{escrow}");
        a.run("UPDATE T SET X = X + 2 WHERE ID = 1").unwrap();
        assert_eq!(exec(&a, "UPDATE T SET N = 1 / (X - X) WHERE ID = 1").err(), Some(Code::Data), "{escrow}");
        a.commit().unwrap();
        assert_eq!(table(&a)[0], "1,103.00,0,0", "{escrow}");
    }
}

/// Verifies: [DESIGN] RULES.md D452: what may see a sum before it is
/// applied, or reads its column, makes the rows written and held: a
/// condition or another SET value reading the column, a SET value not an
/// increment, SERIALIZABLE, LOCKS.WAIT OFF, an index or a CHECK on the
/// column, and escrow turned off.
#[test]
fn what_could_see_a_reserved_sum_holds_the_row() {
    let other = "UPDATE T SET X = X + 1 WHERE ID = 1";
    let plain = "UPDATE T SET X = X + 1 WHERE ID = 1";
    assert!(!holds("ON", &[], (plain, other)));
    for (setup, held) in [
        (&[][..], "UPDATE T SET X = X + 1 WHERE ID = 1 AND X > 0"),
        (&[], "UPDATE T SET X = X + 1, Y = Y + X WHERE ID = 1"),
        (&[], "UPDATE T SET X = X * 2 WHERE ID = 1"),
        (&[], "UPDATE T SET X = X + 1, N = 5 WHERE ID = 1"),
        (&["SET TRANSACTION.ISOLATION = 'SERIALIZABLE'"], plain),
        (&["CREATE INDEX T_X ON T (X)"], plain),
        (&["ALTER TABLE T ADD CONSTRAINT POSITIVE CHECK (X > 0)"], plain),
        (&["SET TRANSACTION.ESCROW = 'OFF'"], plain),
    ] {
        assert!(holds("ON", setup, (held, other)), "{setup:?}: {held}");
    }
    let db = database("ON");
    db.set_setting("LOCKS.WAIT", &Value::Char("OFF".to_owned())).unwrap();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run(plain).unwrap();
    assert_eq!(exec(&b, other).err(), Some(Code::Conflict), "no-wait: held");
}

/// Verifies: [DESIGN] RULES.md D452: New-Order's increment of a
/// district's next order number, read back at once, holds the row from
/// then on, and Payment's increment of the district's year to date goes
/// on meanwhile, applied once the other ends; both sums are kept.
#[test]
fn an_increment_read_back_holds_its_row_and_another_column_goes_on() {
    let db = database("ON");
    let (order, payment) = (db.session("A"), impatient(&db, 100));
    order.run("UPDATE T SET N = N + 1 WHERE ID = 4").unwrap();
    assert_eq!(value(&order, "N - 1", 4), "0");
    payment.run("UPDATE T SET X = X + 7.5 WHERE ID = 4").unwrap();
    assert_eq!(value(&payment, "Y", 4), "0");
    let paid = thread::spawn(move || payment.commit());
    thread::sleep(Duration::from_millis(50));
    order.commit().unwrap();
    paid.join().unwrap().unwrap();
    assert_eq!(table(&order)[3], "4,107.50,0,1");
}

/// Verifies: [DESIGN] RULES.md D25, D452: increments committed are
/// recovered after a crash, from the log alone and after a checkpoint,
/// and those of a transaction open as it crashes are not.
#[test]
fn a_crash_keeps_the_increments_committed_alone() {
    for checkpoint in [false, true] {
        let scratch = Scratch::new("escrow-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        crashed.set_setting("LOCKS.WAIT", &Value::Char("ON".to_owned())).unwrap();
        crashed.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER NOT NULL, X DECIMAL(6,2))").unwrap();
        let s = crashed.session("A");
        s.run("INSERT INTO T VALUES (1, 0)").unwrap();
        s.commit().unwrap();
        s.run("UPDATE T SET X = X + 5 WHERE ID = 1").unwrap();
        s.commit().unwrap();
        if checkpoint {
            crashed.checkpoint().unwrap();
        }
        s.run("UPDATE T SET X = X + 2.5 WHERE ID = 1").unwrap();
        s.commit().unwrap();
        let open = crashed.session("A");
        open.run("UPDATE T SET X = X + 100 WHERE ID = 1").unwrap();
        let (db, opened) = reopened_after_crash(&dir);
        assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
        assert_eq!(rows(&db.session("A"), "SELECT X FROM T").unwrap(), ["7.50"], "checkpoint {checkpoint}");
        drop((db, open, s, crashed));
    }
}
