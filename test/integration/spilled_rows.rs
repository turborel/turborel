//! Spilling of what cursors and data-changing statements hold (RULES.md
//! D38): rows a cursor reads ahead or keeps to scroll, and the rows a
//! write chooses before it changes any, held past MEMORY.WORK_BYTES in spill
//! areas. Split from spilling.rs (RULES.md R-SIZE-2).

use turborel::db::{Completion, Cursor, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{Counted, rows};
use crate::spilling::{ROWS, SMALL, bytes, filled};

/// The rows of `T`, in the order a scan reads them.
const SCANNED: &str = "SELECT K, S FROM T";

/// `row`, as `rows` shows it.
fn shown(row: &[Value]) -> String {
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// Every row `c` has still to give, as `rows` shows them.
fn fetch_all(c: &mut Cursor) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(r) = c.fetch().unwrap() {
        out.push(shown(&r));
    }
    out
}

/// A database, filled, spilling to a counted provider, and a session of
/// it.
fn counted_database() -> (Database, Counted, Session) {
    let db = Database::new();
    filled(&db);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let s = db.session("S");
    (db, counted, s)
}

/// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.8 GR3b]. Regression
/// test (R-TEST-7): a cursor held every row it read ahead of its fetches
/// in memory. With MEMORY.WORK_BYTES at its least, a cursor whose plan can raise
/// an error, its rows read at OPEN, holds them in a spill area, and gives
/// the rows it gives in memory; one read as it is fetched from holds none,
/// until its transaction changes the table it reads: it then reads the
/// rest so, spilling, and gives them as they were before the change.
#[test]
fn a_cursor_holds_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    let raising = "SELECT K + 1, S FROM T";
    let (all_raising, all_scanned) = (rows(&s, raising).unwrap(), rows(&s, SCANNED).unwrap());
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    let mut c = s.open_cursor(raising).unwrap();
    assert!(counted.taken() > 0, "read at OPEN, and spilled");
    assert_eq!(fetch_all(&mut c), all_raising);
    drop(c);
    let mut c = s.open_cursor(SCANNED).unwrap();
    let mut read: Vec<String> = (0..10).map(|_| shown(&c.fetch().unwrap().unwrap())).collect();
    assert_eq!(counted.taken(), 0, "read as it is fetched from");
    s.run("INSERT INTO T VALUES (9999, 0, 'new')").unwrap();
    assert!(counted.taken() > 0, "the rest read before T changed, and spilled");
    read.extend(fetch_all(&mut c));
    assert_eq!(read, all_scanned, "the row inserted not among them");
}

/// Verifies: [DESIGN] RULES.md D38: a cursor whose rows cannot be held,
/// its spill area past the memory budget, fails: at OPEN if it reads them
/// there (SQLCODE -918); else, reading the rest before its transaction
/// changes the table, it gives the rows it held, then the error, then no
/// more.
#[test]
fn a_cursor_past_the_budget_fails() {
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    db.set_setting("MEMORY.BUDGET_BYTES", &bytes(1 << 20)).unwrap();
    assert_eq!(s.open_cursor("SELECT K + 1, S FROM T").err().map(|e| e.code), Some(Code::Resources), "at OPEN");
    let mut c = s.open_cursor(SCANNED).unwrap();
    assert!(c.fetch().unwrap().is_some());
    s.run("INSERT INTO T VALUES (9999, 0, 'new')").unwrap();
    let mut held = 0_usize;
    let failed = loop {
        match c.fetch() {
            Ok(Some(_)) => held += 1,
            Ok(None) => break None,
            Err(e) => break Some(e.code),
        }
    };
    assert_eq!(failed, Some(Code::Resources));
    assert!(held > 0 && held < ROWS, "{held} rows held before it");
    assert!(c.fetch().unwrap().is_none(), "no more after it");
}

/// K_OUT, a FETCH's target.
fn target() -> Params {
    Params::new().declare("K_OUT", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null)
}

/// The K that `FETCH how FROM C` gives, none for no data.
fn fetched(s: &Session, p: &mut Params, how: &str) -> Option<String> {
    match s.execute(&format!("FETCH {how} FROM C INTO K_OUT"), p).unwrap() {
        Completion::NoData => None,
        _ => Some(p.value("K_OUT").to_string()),
    }
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §13.3 GR2–GR5].
/// Regression test (R-TEST-7): a SCROLL cursor kept every row it read in
/// memory. With MEMORY.WORK_BYTES at its least, it keeps them in a spill area,
/// and moves to the row each orientation names, as it does in memory:
/// NEXT, PRIOR, FIRST, LAST, ABSOLUTE and RELATIVE, back and forth across
/// the rows it spilled.
#[test]
fn a_scroll_cursor_keeps_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    let keys = rows(&s, "SELECT K FROM T").unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    s.declare_cursor("DECLARE C SCROLL CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = target();
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(counted.taken(), 0, "nothing read yet");
    let at = |i: usize| Some(keys[i].clone());
    assert_eq!(fetched(&s, &mut p, "NEXT"), at(0));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 2000"), at(1999));
    assert!(counted.taken() > 0, "the rows read kept in a spill area");
    assert_eq!(fetched(&s, &mut p, "PRIOR"), at(1998));
    assert_eq!(fetched(&s, &mut p, "RELATIVE -1500"), at(498));
    assert_eq!(fetched(&s, &mut p, "FIRST"), at(0));
    assert_eq!(fetched(&s, &mut p, "LAST"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE -3000"), at(0));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 2999"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "NEXT"), None, "after the last");
    assert_eq!(fetched(&s, &mut p, "PRIOR"), at(ROWS - 1));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 1500"), at(1499));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 0"), at(1499));
    assert_eq!(fetched(&s, &mut p, "NEXT"), at(1500));
    for i in (0..ROWS).rev().step_by(97) {
        assert_eq!(fetched(&s, &mut p, &format!("ABSOLUTE {}", i + 1)), at(i), "row {i}");
    }
}

/// Verifies: [DESIGN] RULES.md D38 [ISO/IEC 9075:1992 §13.1 GR3, §13.9].
/// Regression test (R-TEST-7): an updatable cursor sorted its rows in
/// memory. With MEMORY.WORK_BYTES at its least, its rows, in its order, are
/// sorted at OPEN in a work area that spills; it gives them in the order it
/// gives in memory, and a positioned UPDATE changes the row it is on.
#[test]
fn an_updatable_cursor_sorts_its_rows_by_spilling() {
    let (db, counted, s) = counted_database();
    // Ordered by G, of ties, a key of the row each FETCH gives with K.
    s.declare_cursor("DECLARE C CURSOR FOR SELECT G, K FROM T ORDER BY G DESC FOR UPDATE OF S").unwrap();
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let next = |p: &mut Params| match s.execute("FETCH NEXT FROM C INTO G_OUT, K_OUT", p).unwrap() {
        Completion::NoData => None,
        _ => Some((p.value("G_OUT").to_string(), p.value("K_OUT").to_string())),
    };
    let mut orders = Vec::new();
    for small in [false, true] {
        if small {
            db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
        }
        let mut p = target().declare("G_OUT", integer, Value::Null);
        s.execute("OPEN C", &mut p).unwrap();
        assert_eq!(counted.taken() > 0, small, "sorted by spilling");
        let order: Vec<Option<(String, String)>> = (0..ROWS).map(|_| next(&mut p)).collect();
        assert!(order.iter().all(Option::is_some));
        s.execute("UPDATE T SET S = 'last' WHERE CURRENT OF C", &mut p).unwrap();
        assert_eq!(next(&mut p), None);
        s.execute("CLOSE C", &mut p).unwrap();
        let (_, last) = order.last().cloned().flatten().unwrap();
        assert_eq!(rows(&s, "SELECT K FROM T WHERE S = 'last'").unwrap(), [last]);
        s.rollback().unwrap();
        orders.push(order);
    }
    assert_eq!(orders[0], orders[1]);
}

/// Verifies: [DESIGN] RULES.md D38 [X3.135-1986 §8.5 GR1, §8.7 GR3, §8.12
/// GR1; ISO/IEC 9075:1992 §13.10 GR6]. Regression test (R-TEST-7): a
/// searched UPDATE or DELETE, and an INSERT of a query, held every row it
/// chose in memory. With MEMORY.WORK_BYTES at its least, each holds them in a
/// spill area, every one chosen before any is changed, and changes the
/// rows it changes in memory: an INSERT of the table's own rows adds each
/// once, an UPDATE of the key it chooses rows by changes each once.
#[test]
fn writes_hold_their_rows_by_spilling() {
    let mut tables = Vec::new();
    for small in [false, true] {
        let (db, counted, s) = counted_database();
        // Its changes held in memory, so the statements' rows alone spill.
        s.run("ALTER TABLE T SET (TRANSACTIONS.HELD_BYTES = 67108864)").unwrap();
        if small {
            db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
        }
        for write in [
            "INSERT INTO T SELECT K + 3000, G, S FROM T",
            "UPDATE T SET K = K + 10000, S = 'u' WHERE G < 500 AND K < 10000",
            "DELETE FROM T WHERE K >= 10000 AND G < 250",
        ] {
            s.run(write).unwrap();
            assert_eq!(counted.taken() > 0, small, "{write}");
        }
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["4500"]);
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K >= 20000").unwrap(), ["0"], "each row updated once");
        tables.push(crate::common::sorted(&s, "SELECT K, G, S FROM T"));
    }
    assert_eq!(tables[0], tables[1]);
}
