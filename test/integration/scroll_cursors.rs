//! Cursors of SQL-92 [ISO/IEC 9075:1992 §13.1, §13.3]: SCROLL and FETCH's
//! orientations, INSENSITIVE, and the `<updatability clause>`, with the
//! positioned statements it permits [§13.6, §13.9].

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Spelled;
use turborel::value::{DataType, Decimal, Value};

/// Schema A: T, of rows K 1 to 5, N 'A' to 'E'.
fn five_rows() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N CHAR(2))").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 'A'), (2, 'B'), (3, 'C'), (4, 'D'), (5, 'E')").unwrap();
    s.commit().unwrap();
    db
}

/// K_OUT, the target, and J, a number.
fn params(j: Value) -> Params {
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    Params::new().declare("K_OUT", integer, Value::Null).declare("J", integer, j)
}

/// The K that `FETCH how FROM C` gives, none for no data.
fn fetched(s: &Session, p: &mut Params, how: &str) -> Result<Option<String>, Code> {
    match s.execute(&format!("FETCH {how} FROM C INTO K_OUT"), p) {
        Ok(Completion::NoData) => Ok(None),
        Ok(_) => Ok(Some(p.value("K_OUT").to_string())),
        Err(e) => Err(e.code),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.3 GR2–GR5] a SCROLL cursor moves to
/// the row each orientation names, ABSOLUTE and RELATIVE by a literal or a
/// parameter; with no data it is left after the last row or before the
/// first, as GR5b says, and moves on from there; FROM alone is NEXT.
#[test]
fn a_scroll_cursor_moves_anywhere_13_3() {
    let db = five_rows();
    let s = db.session("A");
    s.declare_cursor("DECLARE C SCROLL CURSOR FOR SELECT K FROM T ORDER BY K").unwrap();
    let mut p = params(Value::Exact(Decimal::new(-2, 0)));
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(fetched(&s, &mut p, "LAST"), Ok(Some("5".into())));
    assert_eq!(fetched(&s, &mut p, "PRIOR"), Ok(Some("4".into())));
    assert_eq!(fetched(&s, &mut p, "FIRST"), Ok(Some("1".into())));
    assert_eq!(fetched(&s, &mut p, ""), Ok(Some("2".into())), "FROM alone is NEXT");
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 4"), Ok(Some("4".into())));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE J"), Ok(Some("4".into())), "-2: the second from the last");
    assert_eq!(fetched(&s, &mut p, "RELATIVE J"), Ok(Some("2".into())));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 0"), Ok(Some("2".into())));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 3"), Ok(Some("5".into())));
    assert_eq!(fetched(&s, &mut p, "NEXT"), Ok(None), "after the last row");
    assert_eq!(fetched(&s, &mut p, "PRIOR"), Ok(Some("5".into())));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 9"), Ok(None));
    assert_eq!(fetched(&s, &mut p, "RELATIVE -1"), Ok(Some("5".into())), "from after the last");
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE -9"), Ok(None), "before the first row");
    assert_eq!(fetched(&s, &mut p, "NEXT"), Ok(Some("1".into())));
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 0"), Ok(None));
    assert_eq!(fetched(&s, &mut p, "RELATIVE 0"), Ok(None), "not on a row");
    assert_eq!(fetched(&s, &mut p, "NEXT"), Ok(Some("1".into())), "still before the first");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.3 SR3, SR4; §13.1 Format] refused: an
/// orientation other than NEXT on a cursor not SCROLL; ABSOLUTE or
/// RELATIVE of a value not exact with scale 0; and the null value
/// (SQLCODE -301); a cursor named NEXT is a syntax error, NEXT being
/// reserved; NEXT, written or not, fetches as before.
#[test]
fn what_fetch_refuses_13_3() {
    let db = five_rows();
    let s = db.session("A");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM T ORDER BY K").unwrap();
    let mut p = params(Value::Null);
    s.execute("OPEN C", &mut p).unwrap();
    for how in ["PRIOR", "FIRST", "LAST", "ABSOLUTE 1", "RELATIVE 1"] {
        assert_eq!(fetched(&s, &mut p, how), Err(Code::SyntaxRule), "SR3: {how}");
    }
    assert_eq!(fetched(&s, &mut p, "NEXT"), Ok(Some("1".into())));
    s.execute("FETCH C INTO K_OUT", &mut p).unwrap();
    assert_eq!(p.value("K_OUT").to_string(), "2", "no orientation: NEXT");
    s.declare_cursor("DECLARE D SCROLL CURSOR FOR SELECT K FROM T").unwrap();
    s.execute("OPEN D", &mut p).unwrap();
    for how in ["ABSOLUTE 1.5", "ABSOLUTE 'A'", "RELATIVE 1E0"] {
        assert_eq!(
            s.execute(&format!("FETCH {how} FROM D INTO K_OUT"), &mut p).err().map(|e| e.code),
            Some(Code::SyntaxRule),
            "SR4: {how}"
        );
    }
    let mut null = p.clone().indicator("J_IND", -1);
    let refused = s.execute("FETCH ABSOLUTE J INDICATOR J_IND FROM D INTO K_OUT", &mut null);
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Data), "J is null");
    assert_eq!(s.declare_cursor("DECLARE NEXT CURSOR FOR SELECT K FROM T").err().map(|e| e.code), Some(Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.21, §13.1 SR5a, SR12] an INSENSITIVE
/// cursor does not see its transaction's changes made while it is open,
/// and is read-only: a positioned UPDATE through it is refused, as is FOR
/// UPDATE written with INSENSITIVE.
#[test]
fn an_insensitive_cursor_sees_no_change_13_1() {
    let db = five_rows();
    let s = db.session("A");
    s.declare_cursor("DECLARE C INSENSITIVE CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = params(Value::Null);
    s.execute("OPEN C", &mut p).unwrap();
    s.run("DELETE FROM T WHERE K > 1").unwrap();
    s.run("INSERT INTO T VALUES (6, 'F')").unwrap();
    let mut seen = Vec::new();
    while let Ok(Some(k)) = fetched(&s, &mut p, "NEXT") {
        seen.push(k);
    }
    seen.sort();
    assert_eq!(seen, ["1", "2", "3", "4", "5"]);
    assert_eq!(s.execute("UPDATE T SET N = 'Z' WHERE CURRENT OF C", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule));
    let refused = s.declare_cursor("DECLARE D INSENSITIVE CURSOR FOR SELECT K FROM T FOR UPDATE");
    assert_eq!(refused.err().map(|e| e.code), Some(Code::SyntaxRule), "SR12");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1 SR5, SR12–SR14; §13.9 SR8] FOR READ
/// ONLY makes a cursor of an updatable table read-only; FOR UPDATE OF
/// columns lets a positioned UPDATE set those alone; FOR UPDATE of a table
/// that is not updatable, or of a column it has not, is refused at OPEN;
/// \[DESIGN\] an unnamed cursor is read-only, and not FOR UPDATE.
#[test]
fn the_updatability_clause_13_1() {
    let db = five_rows();
    let s = db.session("A");
    let mut p = params(Value::Null);
    s.declare_cursor("DECLARE R CURSOR FOR SELECT K, N FROM T FOR READ ONLY").unwrap();
    s.execute("OPEN R", &mut p).unwrap();
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF R", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "read-only");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM T WHERE K = 2 FOR UPDATE OF N").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(fetched(&s, &mut p, "NEXT"), Ok(Some("2".into())));
    assert_eq!(s.execute("UPDATE T SET K = 9 WHERE CURRENT OF C", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "SR8");
    s.execute("UPDATE T SET N = 'Z' WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(crate::common::rows(&s, "SELECT N FROM T WHERE K = 2"), Ok(vec!["Z ".into()]));
    s.declare_cursor("DECLARE D CURSOR FOR SELECT DISTINCT K FROM T FOR UPDATE").unwrap();
    assert_eq!(s.execute("OPEN D", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "SR12: DISTINCT");
    s.declare_cursor("DECLARE E CURSOR FOR SELECT K FROM T FOR UPDATE OF NOPE").unwrap();
    assert_eq!(s.execute("OPEN E", &mut p).err().map(|e| e.code), Some(Code::SyntaxRule), "SR14");
    assert_eq!(s.open_cursor("SELECT K FROM T FOR UPDATE").err().map(|e| e.code), Some(Code::SyntaxRule), "unnamed");
    assert_eq!(crate::common::count(&s, "SELECT K FROM T FOR READ ONLY"), 5);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1 SR5, GR3; §13.9 SR4; §13.6] an
/// ordered cursor FOR UPDATE is updatable, in its order: a positioned
/// UPDATE sets a column it is not ordered by, and not one it is; a
/// positioned DELETE leaves it before the next row.
#[test]
fn an_ordered_cursor_for_update_13_9() {
    let db = five_rows();
    let s = db.session("A");
    let mut p = params(Value::Null);
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K, N FROM T WHERE K > 1 ORDER BY K DESC FOR UPDATE").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    let mut q = p.clone().declare("N_OUT", DataType::Character { length: 2 }, Value::Null);
    s.execute("FETCH C INTO K_OUT, N_OUT", &mut q).unwrap();
    assert_eq!(q.value("K_OUT").to_string(), "5", "highest first");
    s.execute("UPDATE T SET N = 'Y' WHERE CURRENT OF C", &mut q).unwrap();
    assert_eq!(s.execute("UPDATE T SET K = 0 WHERE CURRENT OF C", &mut q).err().map(|e| e.code), Some(Code::SyntaxRule), "SR4");
    s.execute("FETCH C INTO K_OUT, N_OUT", &mut q).unwrap();
    s.execute("DELETE FROM T WHERE CURRENT OF C", &mut q).unwrap();
    s.execute("FETCH C INTO K_OUT, N_OUT", &mut q).unwrap();
    assert_eq!(q.value("K_OUT").to_string(), "3", "after the row deleted");
    s.commit().unwrap();
    assert_eq!(crate::common::sorted(&s, "SELECT K FROM T"), ["1", "2", "3", "5"]);
    assert_eq!(crate::common::rows(&s, "SELECT N FROM T WHERE K = 5"), Ok(vec!["Y ".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.1, §13.3, §4.21] a SCROLL cursor FOR
/// UPDATE moves among its rows as a read-only one does, passing over a row
/// deleted since OPEN; after a positioned DELETE, RELATIVE 0 has no data
/// and PRIOR is the row before.
#[test]
fn a_scroll_cursor_for_update_13_3() {
    let db = five_rows();
    let s = db.session("A");
    let mut p = params(Value::Null);
    s.declare_cursor("DECLARE C SCROLL CURSOR FOR SELECT K FROM T ORDER BY K FOR UPDATE OF N").unwrap();
    s.execute("OPEN C", &mut p).unwrap();
    s.run("DELETE FROM T WHERE K = 2").unwrap();
    assert_eq!(fetched(&s, &mut p, "ABSOLUTE 2"), Ok(Some("3".into())), "2 is gone");
    assert_eq!(fetched(&s, &mut p, "LAST"), Ok(Some("5".into())));
    s.execute("DELETE FROM T WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(fetched(&s, &mut p, "RELATIVE 0"), Ok(None));
    assert_eq!(fetched(&s, &mut p, "PRIOR"), Ok(Some("4".into())));
    s.execute("UPDATE T SET N = 'Q' WHERE CURRENT OF C", &mut p).unwrap();
    assert_eq!(fetched(&s, &mut p, "FIRST"), Ok(Some("1".into())));
    s.commit().unwrap();
    assert_eq!(crate::common::rows(&s, "SELECT N FROM T WHERE K = 4"), Ok(vec!["Q ".into()]));
}
