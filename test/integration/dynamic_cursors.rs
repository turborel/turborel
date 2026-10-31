//! Dynamic cursors [ISO/IEC 9075:1992 §17.12-§17.20]: one a `<dynamic
//! declare cursor>` declares, of a statement name; one ALLOCATE CURSOR
//! allocates, of an extended name; their OPEN, FETCH and CLOSE; the
//! positioned DELETE and UPDATE through them, prepared too, of no table
//! (RULES.md D58).

use turborel::db::{Completion, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{code_with as code, dynamic_host, number, rows, run_with as run, set_text as text, setup};

/// The DIAGNOSTICS item `item`, text, of the last statement.
fn diagnostic(s: &Session, item: &str) -> String {
    let mut p = dynamic_host();
    s.execute(&format!("GET DIAGNOSTICS TXT = {item}"), &mut p).unwrap();
    p.value("TXT").to_string().trim_end().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.12 GR1, §17.6 GR9b, §17.14
/// GR1, GR3, GR5, §17.15, §17.16] a cursor declared for a statement name
/// is of the statement prepared by that name: OPEN takes the values of its
/// using clause; FETCH puts its rows into targets or a descriptor area;
/// CLOSE closes it. Before it is prepared, OPEN is refused (26000); with
/// parameters and no using clause (07004); the name prepared as no
/// `<cursor specification>` is refused (07005).
#[test]
fn a_dynamic_declared_cursor_17_12() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    s.declare_cursor("DECLARE C SCROLL CURSOR FOR S").unwrap();
    assert_eq!(run(&s, "OPEN C", &mut p), Err("26000"), "§17.14 GR1");
    text(&mut p, "DELETE FROM T");
    assert_eq!(run(&s, "PREPARE S FROM TEXT", &mut p), Err("07005"), "§17.6 GR9b ii");
    text(&mut p, "SELECT ID, NAME FROM T WHERE ID >= ? ORDER BY ID");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "OPEN C", &mut p), Err("07004"), "§17.14 GR3");
    assert_eq!(run(&s, "OPEN C USING K", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "DYNAMIC OPEN");
    assert_eq!(run(&s, "FETCH C INTO N, NM INDICATOR NI", &mut p), Ok(Completion::Success));
    assert_eq!(p.value("N").to_string(), "1");
    run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&s, "DESCRIBE S USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(run(&s, "FETCH LAST FROM C INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "DYNAMIC FETCH");
    run(&s, "GET DESCRIPTOR 'D' VALUE 1 VAL = DATA", &mut p).unwrap();
    assert_eq!(p.value("VAL").to_string(), "3");
    assert_eq!(run(&s, "FETCH C INTO SQL DESCRIPTOR 'D'", &mut p), Ok(Completion::NoData));
    assert_eq!(run(&s, "CLOSE C", &mut p), Ok(Completion::Success));
    s.declare_cursor("DECLARE P CURSOR FOR SELECT ID FROM T").unwrap();
    assert_eq!(code(&s, "OPEN P USING K", &mut p), Code::SyntaxRule, "§17.14 SR1: not a dynamic cursor");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.13 GR1-GR6, §17.14 GR2,
/// §17.15, §17.16, §17.17, §17.18] ALLOCATE CURSOR allocates a cursor of an
/// extended name for a statement prepared; it is opened, fetched from,
/// changed through and closed by that name; one of a name allocated
/// (34000), of a statement not prepared (26000) or no `<cursor
/// specification>` (07005) is refused, and an OPEN of a name not
/// allocated (34000).
#[test]
fn an_allocated_cursor_17_13() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    p.set("SN", Value::Char("Q".to_owned()));
    p.set("CN", Value::Char("CUR".to_owned()));
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR SN", &mut p), Err("26000"), "GR1");
    text(&mut p, "SELECT ID, AMT FROM T WHERE ID < ? FOR UPDATE OF AMT");
    run(&s, "PREPARE SN FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR GLOBAL SN", &mut p), Err("26000"), "GR1: LOCAL Q, not GLOBAL");
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR LOCAL SN", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "ALLOCATE CURSOR");
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR SN", &mut p), Err("34000"), "GR5");
    assert_eq!(run(&s, "OPEN GLOBAL CN USING K", &mut p), Err("34000"), "§17.14 GR2");
    p.set("K", number(3));
    assert_eq!(run(&s, "OPEN CN USING K", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "FETCH CN INTO ID, N", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "UPDATE T SET AMT = 99 WHERE CURRENT OF CN", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "DYNAMIC UPDATE CURSOR");
    assert_eq!(run(&s, "FETCH CN INTO ID, N", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "DELETE FROM T WHERE CURRENT OF CN", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "DYNAMIC DELETE CURSOR");
    assert_eq!(run(&s, "CLOSE CN", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "DYNAMIC CLOSE");
    assert_eq!(run(&s, "CLOSE CN", &mut p), Err("24000"));
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY ID").unwrap(), ["1,99.00", "3,NULL"]);
    text(&mut p, "DELETE FROM T");
    run(&s, "PREPARE GLOBAL SN FROM TEXT", &mut p).unwrap();
    p.set("CN", Value::Char("OTHER".to_owned()));
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR GLOBAL SN", &mut p), Err("07005"), "GR2");
    p.set("CN", Value::Char("NOT ONE".to_owned()));
    assert_eq!(run(&s, "ALLOCATE CN CURSOR FOR SN", &mut p), Err("34000"), "GR4");
    assert_eq!(code(&s, "ALLOCATE 'C' CURSOR FOR SN", &mut p), Code::SyntaxRule, "§5.4 SR13");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.7 GR2, GR3, §17.13 GR6]
/// DEALLOCATE PREPARE of a statement whose cursor is open is refused
/// (24000); once it is closed, the cursors allocated for it are destroyed,
/// and so are the statements prepared that change a row through them.
#[test]
fn deallocate_prepare_destroys_its_cursors_17_7() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    p.set("SN", Value::Char("Q".to_owned()));
    p.set("CN", Value::Char("CUR".to_owned()));
    text(&mut p, "SELECT ID FROM T");
    run(&s, "PREPARE SN FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE CN CURSOR FOR SN", &mut p).unwrap();
    text(&mut p, "DELETE FROM T WHERE CURRENT OF CUR");
    run(&s, "PREPARE D FROM TEXT", &mut p).unwrap();
    run(&s, "OPEN CN", &mut p).unwrap();
    assert_eq!(run(&s, "DEALLOCATE PREPARE SN", &mut p), Err("24000"), "GR2");
    run(&s, "CLOSE CN", &mut p).unwrap();
    assert_eq!(run(&s, "DEALLOCATE PREPARE SN", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "OPEN CN", &mut p), Err("34000"), "GR3: the cursor destroyed");
    assert_eq!(run(&s, "EXECUTE D", &mut p), Err("26000"), "GR3: so too D, of it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR2, §17.19 SR1, §17.20
/// SR1, §17.10 GR8b, GR8c, §18.1 GR1d] a positioned DELETE or UPDATE
/// prepared names a cursor declared, dynamic or not, or allocated: one of
/// no table changes the table of the cursor's specification; through the
/// API too. No cursor of the name is refused (34000); one of each kind
/// (3C000); a cursor of no updatable table, for one of no table (37000).
#[test]
fn positioned_statements_prepared_17_19_17_20() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "DELETE WHERE CURRENT OF CUR");
    assert_eq!(run(&s, "PREPARE D FROM TEXT", &mut p), Err("34000"), "GR2b");
    s.declare_cursor("DECLARE CUR CURSOR FOR S").unwrap();
    text(&mut p, "SELECT ID, AMT FROM T WHERE ID > 1");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    text(&mut p, "UPDATE SET AMT = ? WHERE CURRENT OF CUR");
    run(&s, "PREPARE U FROM TEXT", &mut p).unwrap();
    text(&mut p, "DELETE WHERE CURRENT OF CUR");
    run(&s, "PREPARE D FROM TEXT", &mut p).unwrap();
    run(&s, "OPEN CUR", &mut p).unwrap();
    run(&s, "FETCH CUR INTO ID, N INDICATOR NI", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE U USING K", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "DYNAMIC_FUNCTION"), "DYNAMIC UPDATE CURSOR");
    run(&s, "FETCH CUR INTO ID, N INDICATOR NI", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE D", &mut p), Ok(Completion::Success));
    let api = s.prepare("DELETE FROM T WHERE CURRENT OF CUR").unwrap();
    assert_eq!(s.execute_prepared(&api, &[]).map_err(|e| e.code), Err(Code::Cursor), "on no row, deleted");
    run(&s, "CLOSE CUR", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY ID").unwrap(), ["1,10.50", "2,1.00"]);
    p.set("CN", Value::Char("CUR".to_owned()));
    p.set("SN", Value::Char("S".to_owned()));
    text(&mut p, "SELECT ID FROM T");
    run(&s, "PREPARE SN FROM TEXT", &mut p).unwrap();
    run(&s, "ALLOCATE GLOBAL CN CURSOR FOR SN", &mut p).unwrap();
    text(&mut p, "DELETE WHERE CURRENT OF CUR");
    assert_eq!(run(&s, "PREPARE D FROM TEXT", &mut p), Err("3C000"), "GR2a");
    s.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM T UNION SELECT ID FROM U").unwrap();
    text(&mut p, "DELETE WHERE CURRENT OF R");
    assert_eq!(run(&s, "PREPARE D FROM TEXT", &mut p), Err("37000"), "§17.19 SR2: no table");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.17, §17.18, §17.16, §13.6
/// GR1, §13.9 GR1] in a READ ONLY transaction a positioned DELETE or
/// UPDATE through an extended cursor is refused (SQLCODE -917); one of a
/// name not allocated (34000); CLOSE of one not allocated (34000).
#[test]
fn extended_cursor_names_refused_17_17_17_18() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    p.set("CN", Value::Char("CUR".to_owned()));
    for refused in ["DELETE FROM T WHERE CURRENT OF CN", "UPDATE T SET AMT = 1 WHERE CURRENT OF CN", "CLOSE CN", "FETCH CN INTO ID"] {
        assert_eq!(run(&s, refused, &mut p), Err("34000"), "{refused}");
    }
    s.rollback().unwrap();
    run(&s, "SET TRANSACTION READ ONLY", &mut p).unwrap();
    assert_eq!(code(&s, "DELETE FROM T WHERE CURRENT OF CN", &mut p), Code::TransactionState);
    assert_eq!(code(&s, "UPDATE T SET AMT = 1 WHERE CURRENT OF CN", &mut p), Code::TransactionState);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.19 SR1, §17.20 SR1,
/// §17.6 GR2, §17.14 GR1, §17.9 GR6a] a positioned statement prepared of
/// no table, its cursor open, is of the table the cursor changes; of a
/// dynamic cursor whose statement is not prepared, refused (26000); one
/// with more after the cursor's name is no such statement (SQLCODE -103);
/// a statement of no row executed INTO targets does not match them
/// (07002).
#[test]
fn positioned_statements_of_open_cursors_17_19() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    s.declare_cursor("DECLARE U CURSOR FOR SELECT T.ID, AMT FROM T WHERE AMT < 0 FOR UPDATE OF AMT").unwrap();
    run(&s, "OPEN U", &mut p).unwrap();
    run(&s, "FETCH U INTO ID, N", &mut p).unwrap();
    text(&mut p, "UPDATE SET AMT = 5 WHERE CURRENT OF U");
    run(&s, "PREPARE P FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE P", &mut p), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT AMT FROM T WHERE ID = 2").unwrap(), ["5.00"]);
    assert_eq!(run(&s, "EXECUTE P INTO N", &mut p), Err("07002"), "§17.9 GR6a: of degree 0");
    assert_eq!(s.prepare("DELETE WHERE CURRENT OF U MORE").map_err(|e| e.code).err(), Some(Code::SyntaxRule));
    s.declare_cursor("DECLARE V CURSOR FOR NOT_YET").unwrap();
    text(&mut p, "DELETE WHERE CURRENT OF V");
    assert_eq!(run(&s, "PREPARE Q FROM TEXT", &mut p), Err("26000"));
}
