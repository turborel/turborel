//! \[DESIGN\] An increment through a delta table reserved by a point change
//! (RULES.md D942): a compiled procedure's SELECT ... INTO of a FINAL
//! TABLE of an UPDATE by a whole unique key that only adds to its columns
//! reserves its increments where it reads its row, and writes nothing
//! before its transaction commits. Each test runs it so, and by the
//! general path it ran by before, EXECUTOR.POINT_OPERATIONS ON and OFF:
//! what each session is shown, which statements wait, which fail, and
//! what the tables hold are the same of both.

use std::thread;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{exec, rows};

/// EXECUTOR.POINT_OPERATIONS, by which the increment is reserved by the
/// point change, and by the general path.
const BOTH: [&str; 2] = ["ON", "OFF"];

/// The procedures of these cases. PAY adds to X and Y and shows N and
/// NAME, as Payment's year to date shows the district's address; SHOWN
/// adds to X and shows it. Each notes what it was shown in SEEN.
const PROCEDURES: [&str; 2] = [
    "CREATE PROCEDURE PAY (IN KK INTEGER, IN AMT DECIMAL(6,2), OUT NN INTEGER, OUT NAMED VARCHAR(8)) MODIFIES SQL DATA BEGIN
       SELECT N, NAME INTO NN, NAMED FROM FINAL TABLE (UPDATE T SET X = X + AMT, Y = Y - 1 WHERE ID = KK);
       INSERT INTO SEEN VALUES ('PAY', NN); END",
    "CREATE PROCEDURE SHOWN (IN KK INTEGER, IN AMT DECIMAL(6,2)) MODIFIES SQL DATA BEGIN DECLARE XX DECIMAL(6,2);
       SELECT X INTO XX FROM FINAL TABLE (UPDATE T SET X = X + AMT WHERE ID = KK); INSERT INTO SEEN VALUES ('SHOWN', XX); END",
];

/// A database whose statements wait for locks, escrow as `escrow` says
/// and point operations as `point` says, with table T of schema A, its
/// key ID: X DECIMAL(6,2), Y INTEGER, N INTEGER and NAME, rows 1 to 4 of
/// X 100, Y 0 and N 0; table SEEN; and `PROCEDURES`.
fn database(escrow: &str, point: &str) -> Database {
    let db = Database::new();
    db.set_setting("LOCKS.WAIT", &Value::Char("ON".into())).unwrap();
    db.set_setting("TRANSACTION.ESCROW", &Value::Char(escrow.into())).unwrap();
    db.set_setting("EXECUTOR.POINT_OPERATIONS", &Value::Char(point.into())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
         CREATE TABLE T (ID INTEGER NOT NULL PRIMARY KEY, X DECIMAL(6,2), Y INTEGER, N INTEGER, NAME VARCHAR(8))
         CREATE TABLE SEEN (P VARCHAR(8), V DECIMAL(8,2))",
    )
    .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 100, 0, 0, 'one'), (2, 100, 0, 0, 'two'), (3, 100, 0, 0, 'three'), (4, 100, 0, 0, 'four')").unwrap();
    for p in PROCEDURES {
        s.run(p).unwrap();
    }
    s.commit().unwrap();
    db
}

/// `s`, each of `PROCEDURES` called three times on row 4 and rolled
/// back, so the plans its session keeps settle and the compiled path runs
/// them from then on (D406).
fn warmed(s: Session) -> Session {
    for _ in 0..3_u8 {
        for call in ["CALL PAY(4, 1, ?, ?)", "CALL SHOWN(4, 1)"] {
            called(&s, call).unwrap();
        }
    }
    s.rollback().unwrap();
    s
}

/// What `sql`, a CALL, gave in `s`: its output values, or its error's code.
fn called(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let p = s.prepare(sql).map_err(|e| e.code)?;
    s.execute_prepared_out(&p, &[]).map(|(_, out)| out.iter().map(|v| v.to_string().trim_end().to_owned()).collect()).map_err(|e| e.code)
}

/// A session of `db` that waits 100 milliseconds for a row at most.
fn impatient(db: &Database) -> Session {
    let s = db.session("A");
    s.run("SET LOCKS.TIMEOUT_MS = 100").unwrap();
    warmed(s)
}

/// Whether `sql` in a second session is refused for want of a row a first
/// session's `held` holds, its transaction open at the isolation level
/// `level`, with escrow as `escrow` says and point operations as `point`.
fn holds((escrow, point, level): (&str, &str, &str), held: &str, sql: &str) -> bool {
    let db = database(escrow, point);
    let owner = warmed(db.session("A"));
    owner.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    called(&owner, held).unwrap_or_else(|e| panic!("{held}: {e:?}"));
    let other = impatient(&db);
    let refused = if sql.starts_with("CALL") { called(&other, sql).err() } else { exec(&other, sql).err() };
    other.rollback().unwrap();
    owner.rollback().unwrap();
    assert!(refused.is_none() || refused == Some(Code::Conflict), "{sql}: {refused:?}");
    refused.is_some()
}

/// Verifies: [DESIGN] RULES.md D942, D849: an increment by a whole key
/// through a FINAL TABLE that shows other columns holds no row, reserved
/// by the point change as by the general path: another session's
/// increment of the row goes on. One that shows the column it adds to,
/// one at SERIALIZABLE, or one with escrow off writes its row and holds
/// it. MC/DC of the reservation: the column shown or not, the level, the
/// setting.
#[test]
fn an_increment_by_a_key_holds_no_row_unless_it_is_written() {
    let pay = "CALL PAY(1, 5, ?, ?)";
    for point in BOTH {
        assert!(!holds(("ON", point, "READ COMMITTED"), pay, pay), "{point}");
        assert!(!holds(("ON", point, "READ COMMITTED"), pay, "UPDATE T SET X = X + 1 WHERE ID = 1"), "{point}");
        assert!(holds(("ON", point, "READ COMMITTED"), "CALL SHOWN(1, 5)", pay), "{point}");
        assert!(holds(("ON", point, "SERIALIZABLE"), pay, pay), "{point}");
        assert!(holds(("OFF", point, "READ COMMITTED"), pay, pay), "{point}");
    }
}

/// Verifies: [DESIGN] RULES.md D942, D849: the reservation a point change
/// takes guards the columns its select list shows, the columns it adds
/// to and its key: another transaction's change of one waits, as for a
/// held row; a change of another row goes on.
#[test]
fn a_reservation_by_a_key_guards_what_it_shows() {
    let pay = "CALL PAY(2, 5, ?, ?)";
    for point in BOTH {
        for (sql, waits) in [
            ("UPDATE T SET N = 7 WHERE ID = 2", true),
            ("UPDATE T SET NAME = 'x' WHERE ID = 2", true),
            ("UPDATE T SET X = 0 WHERE ID = 2", true),
            ("UPDATE T SET ID = 9 WHERE ID = 2", true),
            ("DELETE FROM T WHERE ID = 2", true),
            ("UPDATE T SET N = 7 WHERE ID = 3", false),
            ("CALL PAY(3, 1, ?, ?)", false),
        ] {
            assert_eq!(holds(("ON", point, "READ COMMITTED"), pay, sql), waits, "{point}: {sql}");
        }
    }
}

/// What a session's payments, read back, rolled back and committed give,
/// with point operations as `point`.
fn scripted(point: &str) -> Vec<String> {
    let db = database("ON", point);
    let a = warmed(db.session("A"));
    let read = |s: &Session, sql: &str| format!("{sql}: {:?}", rows(s, sql));
    let mut gave = Vec::new();
    // Shown the row as read, none of its sums; no row, no data.
    gave.push(format!("{:?}", called(&a, "CALL PAY(1, 5.25, ?, ?)")));
    gave.push(format!("{:?}", called(&a, "CALL PAY(1, 1, ?, ?)")));
    gave.push(format!("{:?}", called(&a, "CALL PAY(9, 1, ?, ?)")));
    // Its own sums read back, then added to again.
    gave.push(read(&a, "SELECT X, Y FROM T WHERE ID = 1"));
    gave.push(format!("{:?}", called(&a, "CALL PAY(1, 2, ?, ?)")));
    gave.push(format!("{:?}", a.commit().map_err(|e| e.code)));
    gave.push(read(&a, "SELECT ID, X, Y, N FROM T ORDER BY ID"));
    // A rollback, and a rollback to a savepoint, take increments back.
    gave.push(format!("{:?}", called(&a, "CALL PAY(2, 7, ?, ?)")));
    gave.push(format!("{:?}", a.rollback().map_err(|e| e.code)));
    gave.push(format!("{:?}", called(&a, "CALL PAY(2, 3, ?, ?)")));
    gave.push(format!("{:?}", exec(&a, "SAVEPOINT S")));
    gave.push(format!("{:?}", called(&a, "CALL PAY(2, 4, ?, ?)")));
    gave.push(format!("{:?}", exec(&a, "ROLLBACK TO SAVEPOINT S")));
    gave.push(format!("{:?}", a.commit().map_err(|e| e.code)));
    // A sum past the column's type is written, and raises at its statement.
    gave.push(format!("{:?}", exec(&a, "UPDATE T SET X = 9990 WHERE ID = 3")));
    gave.push(format!("{:?}", a.commit().map_err(|e| e.code)));
    gave.push(format!("{:?}", called(&a, "CALL PAY(3, 5, ?, ?)")));
    gave.push(format!("{:?}", called(&a, "CALL PAY(3, 5, ?, ?)")));
    gave.push(format!("{:?}", a.commit().map_err(|e| e.code)));
    gave.push(read(&a, "SELECT ID, X, Y, N FROM T ORDER BY ID"));
    gave.push(read(&a, "SELECT P, V FROM SEEN ORDER BY P, V"));
    gave
}

/// Verifies: [DESIGN] RULES.md D942, D452: a payment reserved by the
/// point change is shown the row as it read it, reads its own sums back,
/// is taken back by a rollback or a rollback to a savepoint, finds no
/// data of a key not there, and raises at its own statement once its sum
/// would leave the column's type: all as the general path gives them.
#[test]
fn a_payment_by_a_key_gives_what_the_general_path_gives() {
    let (point, general) = (scripted("ON"), scripted("OFF"));
    assert_eq!(point, general, "by the point change, then by the general path");
    assert_eq!(point[0], "Ok([\"0\", \"one\"])", "{point:?}");
    assert_eq!(point[2], "Ok([\"NULL\", \"NULL\"])", "no row: {point:?}");
    assert!(point[3].contains("106.25,-2"), "its own sums: {point:?}");
    assert!(point[6].contains("\"1,108.25,-3,0\""), "committed: {point:?}");
    assert!(point[17].contains("Err(Data)"), "{point:?}");
    assert!(point[19].contains("\"2,103.00,-1,0\"") && point[19].contains("\"3,9995.00,-1,0\""), "{point:?}");
}

/// Verifies: [DESIGN] RULES.md D942; [ISO/IEC 9075-2:2023 §7.6 GR5,
/// ISO/IEC 9075:1992 §4.28]: at READ COMMITTED two sessions paying into
/// one row at once both commit, neither waiting, each shown the row's
/// other columns as committed, and both sums are applied. At SERIALIZABLE
/// each writes the row, so the second waits for the first and then fails,
/// its read no longer what is committed.
#[test]
fn two_payments_by_a_key_both_apply_at_read_committed() {
    for point in BOTH {
        let db = database("ON", point);
        let (a, b) = (warmed(db.session("A")), warmed(db.session("A")));
        a.run("UPDATE T SET N = 3 WHERE ID = 1").unwrap();
        a.commit().unwrap();
        assert_eq!(called(&a, "CALL PAY(1, 5, ?, ?)").unwrap(), ["3", "one"]);
        let paid = thread::spawn(move || called(&b, "CALL PAY(1, 2.5, ?, ?)").and_then(|out| b.commit().map(|()| out).map_err(|e| e.code)));
        assert_eq!(paid.join().unwrap().unwrap(), ["3", "one"], "{point}");
        a.commit().unwrap();
        let s = db.session("A");
        assert_eq!(rows(&s, "SELECT X, Y, N FROM T WHERE ID = 1").unwrap(), ["107.50,-2,3"], "{point}");
        assert_eq!(rows(&s, "SELECT P, V FROM SEEN ORDER BY P").unwrap(), ["PAY,3.00", "PAY,3.00"], "{point}");

        let (a, b) = (warmed(db.session("A")), warmed(db.session("A")));
        for s in [&a, &b] {
            s.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
            assert_eq!(rows(s, "SELECT COUNT(*) FROM SEEN").unwrap(), ["2"]);
        }
        called(&a, "CALL PAY(1, 5, ?, ?)").unwrap();
        let paid = thread::spawn(move || called(&b, "CALL PAY(1, 2.5, ?, ?)").and_then(|out| b.commit().map(|()| out).map_err(|e| e.code)));
        crate::common::until_waiting(&db, 1);
        a.commit().unwrap();
        assert_eq!(paid.join().unwrap(), Err(Code::Conflict), "{point}");
        assert_eq!(rows(&s, "SELECT X, Y, N FROM T WHERE ID = 1").unwrap(), ["112.50,-3,3"], "{point}");
    }
}

/// Twenty transactions of session `s`, the `t`th of many, each a payment
/// into one of two rows of T, run again until it commits.
fn paying(s: &Session, t: usize) {
    for n in 0..20_usize {
        let id = ((n >> 1_u32) + t) % 2 + 1;
        let sql = format!("CALL PAY({id}, 1.25, ?, ?)");
        while called(s, &sql).and_then(|_| s.commit().map_err(|e| e.code)).is_err() {
            s.rollback().unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D942; [ISO/IEC 9075:1992 §4.28]: sixteen
/// sessions paying into two rows at once, each committing every payment,
/// lose none, by the point change as by the general path.
#[test]
fn many_payments_by_a_key_lose_no_update() {
    let mut left = Vec::new();
    for point in BOTH {
        let db = database("ON", point);
        let threads: Vec<_> = (0..16_usize)
            .map(|t| {
                let db = db.clone();
                thread::spawn(move || paying(&warmed(db.session("A")), t))
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let s = db.session("A");
        left.push((rows(&s, "SELECT ID, X, Y FROM T WHERE ID < 3 ORDER BY ID").unwrap(), rows(&s, "SELECT COUNT(*) FROM SEEN").unwrap()));
    }
    assert_eq!(left[0], left[1], "by the point change, then by the general path");
    assert_eq!(left[0].0, ["1,300.00,-160", "2,300.00,-160"]);
    assert_eq!(left[0].1, ["320"]);
}
