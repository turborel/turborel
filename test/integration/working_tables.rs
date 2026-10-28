//! The working tables of open cursors [X3.135-1986 §4.12, §8.8 GR3b]:
//! the rows a cursor gives while its own transaction changes the tables
//! it reads, and the effect of an error on a cursor. Split from cursors.rs
//! (RULES.md R-SIZE-2).

use turborel::db::Completion;
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{exec, rows, setup};
use turborel::value::Spelled;

/// Targets for fetching T's ID, NAME and AMT, with indicators for NAME
/// and AMT.
fn params() -> Params {
    Params::new()
        .declare("ID_OUT", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null)
        .declare("NAME_OUT", DataType::Character { length: 2 }, Value::Null)
        .indicator("NAME_IND", 0)
        .declare("AMT_OUT", DataType::Exact { precision: 8, scale: 2, spelled: Spelled::Numeric }, Value::Null)
        .indicator("AMT_IND", 0)
}

/// Verifies: [X3.135-1986 §8.8 GR3b] a read-only cursor's table is created
/// at OPEN: when its own transaction then changes, inserts into or deletes
/// from the table it reads, FETCH still gives the rows as they were, for a
/// declared cursor and for `open_cursor`; [DESIGN] rows read
/// as the cursor is fetched from.
#[test]
fn a_cursor_keeps_the_rows_of_its_open_8_8() {
    let db = setup();
    let s = db.session("APP");
    let mut p = params();
    s.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM T ORDER BY ID").unwrap();
    // DISTINCT makes it read-only [§5.25 SR11a]; an updatable cursor reads
    // each row as it reaches it.
    s.declare_cursor("DECLARE S CURSOR FOR SELECT DISTINCT ID, NAME FROM T WHERE ID > 0").unwrap();
    s.execute("OPEN R", &mut p).unwrap();
    s.execute("OPEN S", &mut p).unwrap();
    let mut anonymous = s.open_cursor("SELECT AMT FROM T WHERE ID = 1").unwrap();
    s.execute("FETCH S INTO ID_OUT, NAME_OUT INDICATOR NAME_IND", &mut p).unwrap();
    exec(&s, "UPDATE T SET NAME = 'NEW', AMT = 0").unwrap();
    exec(&s, "INSERT INTO T (ID) VALUES (9)").unwrap();
    exec(&s, "DELETE FROM T WHERE ID = 3").unwrap();
    let mut ids = Vec::new();
    // Each NAME is cut to fit NAME_OUT, a warning [ISO/IEC 9075:1992 §9.1 GR3b].
    while s.execute("FETCH S INTO ID_OUT, NAME_OUT INDICATOR NAME_IND", &mut p).unwrap() != Completion::NoData {
        ids.push((p.value("ID_OUT").to_string(), p.value("NAME_OUT").to_string()));
    }
    assert_eq!(ids.len(), 2, "rows 2 and 3, not 9: {ids:?}");
    assert!(ids.iter().all(|(id, name)| id != "9" && name != "NE"), "{ids:?}");
    assert_eq!(anonymous.fetch().unwrap().map(|r| r[0].to_string()), Some("10.50".into()));
    let mut ordered = Vec::new();
    while s.execute("FETCH R INTO ID_OUT", &mut p).unwrap() == Completion::Success {
        ordered.push(p.value("ID_OUT").to_string());
    }
    assert_eq!(ordered, ["1", "2", "3"]);
}

/// Verifies: [X3.135-1986 §8.8 GR3b, §5.9 GR6b] an error a cursor's rows
/// raise is raised by OPEN, however few rows are fetched: here a division
/// by zero in the third row's select list; [DESIGN] RULES.md D15.
#[test]
fn a_cursor_raises_its_errors_at_open_8_8() {
    let db = setup();
    let s = db.session("APP");
    let mut p = params();
    s.declare_cursor("DECLARE Z CURSOR FOR SELECT 1 / (ID - 3) FROM T").unwrap();
    assert_eq!(s.execute("OPEN Z", &mut p).unwrap_err().code, Code::Data);
    assert_eq!(s.open_cursor("SELECT 1 / (ID - 3) FROM T").err().map(|e| e.code), Some(Code::Data));
    assert_eq!(s.open_cursor("SELECT ID FROM T WHERE ID / (ID - 3) > 0").err().map(|e| e.code), Some(Code::Data));
}

/// Verifies: [X3.135-1986 §4.12, §8.8 GR3b] an updatable cursor's table is
/// a working viewed table, each row derived when the cursor reaches it: a
/// row changed since OPEN has its new values, and one changed so it no
/// longer satisfies the WHERE clause is skipped, as a deleted row is; a row
/// inserted since OPEN is not covered, the implementor-defined effect of an
/// insertion on the cursor's position being none.
#[test]
fn an_updatable_cursor_derives_each_row_as_it_reaches_it_4_12() {
    let db = setup();
    let s = db.session("APP");
    let mut p = params();
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID, AMT FROM T WHERE ID < 3").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    exec(&s, "UPDATE T SET AMT = 1 WHERE ID = 1").unwrap();
    exec(&s, "UPDATE T SET ID = 7 WHERE ID = 2").unwrap();
    exec(&s, "INSERT INTO T (ID, AMT) VALUES (0, 0)").unwrap();
    assert_eq!(s.execute("FETCH C INTO ID_OUT, AMT_OUT INDICATOR AMT_IND", &mut p).map_err(|e| e.code), Ok(Completion::Success));
    assert_eq!((p.value("ID_OUT").to_string(), p.value("AMT_OUT").to_string()), ("1".into(), "1.00".into()), "the new value");
    assert_eq!(
        s.execute("FETCH C INTO ID_OUT, AMT_OUT INDICATOR AMT_IND", &mut p).map_err(|e| e.code),
        Ok(Completion::NoData),
        "row 2 no longer qualifies; row 0 is not covered"
    );
}

/// Verifies: [X3.135-1986 §4.12, §8.6 GR6] the implementor-defined effect
/// of an error on an open cursor: a FETCH that fails deriving its row (its
/// WHERE clause raising an error on the row's new values) does not move
/// the cursor, which stays on its current row for a positioned UPDATE and
/// fetches the row once it can be derived; a FETCH that fails assigning a
/// value leaves the cursor on the row it fetched.
#[test]
fn a_failed_fetch_leaves_the_cursor_where_it_says_4_12() {
    let db = setup();
    let s = db.session("APP");
    let mut p = params();
    s.declare_cursor("DECLARE E CURSOR FOR SELECT ID FROM T WHERE 1 / (4 - ID) > 0").unwrap();
    s.execute("OPEN E", &mut p).unwrap();
    s.execute("FETCH E INTO ID_OUT", &mut p).unwrap();
    exec(&s, "UPDATE T SET ID = 4 WHERE ID = 2").unwrap();
    assert_eq!(s.execute("FETCH E INTO ID_OUT", &mut p).unwrap_err().code, Code::Data, "1 / 0");
    assert_eq!(s.execute("FETCH E INTO ID_OUT", &mut p).unwrap_err().code, Code::Data, "still before row 2");
    s.execute("UPDATE T SET AMT = 5 WHERE CURRENT OF E", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT AMT FROM T WHERE ID = 1"), Ok(vec!["5.00".into()]), "on row 1 still");
    exec(&s, "UPDATE T SET ID = 2 WHERE ID = 4").unwrap();
    let mut fetched = Vec::new();
    while s.execute("FETCH E INTO ID_OUT", &mut p).unwrap() == Completion::Success {
        fetched.push(p.value("ID_OUT").to_string());
    }
    assert_eq!(fetched, ["2", "3"]);
    s.declare_cursor("DECLARE N CURSOR FOR SELECT ID, NAME FROM T ORDER BY ID").unwrap();
    s.execute("OPEN N", &mut p).unwrap();
    s.execute("FETCH N INTO ID_OUT, NAME_OUT", &mut p).unwrap();
    s.execute("FETCH N INTO ID_OUT, NAME_OUT", &mut p).unwrap();
    assert_eq!(s.execute("FETCH N INTO ID_OUT, NAME_OUT", &mut p).unwrap_err().code, Code::Data, "row 3's NAME is null: no indicator");
    assert_eq!(
        s.execute("FETCH N INTO ID_OUT, NAME_OUT", &mut p).map_err(|e| e.code),
        Ok(Completion::NoData),
        "the cursor was on row 3, the last"
    );
}
