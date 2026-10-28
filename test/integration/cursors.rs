//! Cursors [X3.135-1986 §4.12, §8.1, §8.3, §8.4, §8.6, §8.8, §8.10, §8.11] and
//! parameters [X3.135-1986 §5.6, §7.3].

use turborel::db::{Completion, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, MAX_CHARACTER_LENGTH, Value};

use crate::common::{count, exec, rows, setup};
use turborel::value::Spelled;

fn targets() -> Params {
    Params::new()
        .declare("ID_OUT", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null)
        .declare("NAME_OUT", DataType::Character { length: 2 }, Value::Null)
        .indicator("NAME_IND", 0)
        .declare("LIMIT", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(1, 0)))
}

/// The fetch used by the cursor tests on cursor C of `declare_c`.
const FETCH_C: &str = "FETCH C INTO ID_OUT, NAME_OUT INDICATOR NAME_IND";

/// Declares cursor C: ID and NAME of the rows with ID above LIMIT, highest ID first.
fn declare_c(s: &Session) {
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID, NAME FROM T WHERE ID > LIMIT ORDER BY ID DESC").unwrap();
}

/// Verifies: [X3.135-1986 §8.3 SR1] two cursors may not have the same name.
#[test]
fn cursor_names_must_differ_8_3() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    assert_eq!(s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T").unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §8.3 SR1]
}

/// Verifies: [X3.135-1986 §8.6 GR1] FETCH needs an open cursor.
#[test]
fn fetch_needs_an_open_cursor_8_6() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    assert_eq!(s.execute(FETCH_C, &mut p).unwrap_err().code, Code::Cursor); // [X3.135-1986 §8.6 GR1] not open
}

/// Verifies: [X3.135-1986 §8.8 GR1] an open cursor cannot be opened again.
#[test]
fn an_open_cursor_cannot_be_opened_8_8() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(s.execute("OPEN C", &mut p).unwrap_err().code, Code::Cursor); // [X3.135-1986 §8.8 GR1]
}

/// Verifies: [X3.135-1986 §8.6 GR8] fetching a null sets the indicator to -1.
#[test]
fn fetch_of_a_null_sets_the_indicator_to_minus_one_8_6() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    // Row 3 has a null NAME: the indicator says -1 [X3.135-1986 §8.6 GR8].
    assert_eq!(s.execute(FETCH_C, &mut p).unwrap(), Completion::Success);
    assert_eq!((p.value("ID_OUT").to_string(), p.indicator_value("NAME_IND")), ("3".into(), Some(-1)));
}

/// Verifies: [X3.135-1986 §8.6 GR8a, GR10b] a string longer than its target is
/// cut to fit, and the indicator holds its full length; [ISO/IEC 9075:1992
/// §9.1 GR3b] the FETCH completes with the warning 01004, of SQLCODE 0, and
/// one cutting nothing without it.
#[test]
fn fetch_cuts_a_long_string_and_reports_its_length_8_6() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    assert_eq!(s.execute(FETCH_C, &mut p).unwrap(), Completion::Success, "a null value cuts nothing");
    // Row 2 is 'BOB   ': six characters into CHARACTER(2), so the indicator holds 6 [X3.135-1986 §8.6 GR8a, GR10b].
    let done = s.execute(FETCH_C, &mut p).unwrap();
    assert_eq!((p.value("NAME_OUT").to_string(), p.indicator_value("NAME_IND")), ("BO".into(), Some(6)));
    assert_eq!((done, done.sqlcode(), done.sqlstate()), (Completion::Warning(SqlState::STRING_TRUNCATED), 0, SqlState::STRING_TRUNCATED));
}

/// Verifies: [X3.135-1986 §8.6 GR2] a FETCH past the last row gives SQLCODE 100
/// and leaves the targets as they were.
#[test]
fn fetch_past_the_last_row_gives_sqlcode_100_8_6() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C, &mut p).unwrap();
    s.execute(FETCH_C, &mut p).unwrap();
    // [X3.135-1986 §8.6 GR2] past the last row: SQLCODE 100, targets untouched.
    assert_eq!(s.execute(FETCH_C, &mut p).unwrap(), Completion::NoData);
    assert_eq!(p.value("ID_OUT").to_string(), "2");
}

/// Verifies: [X3.135-1986 §8.1 GR1] a closed cursor cannot be closed again.
#[test]
fn a_closed_cursor_cannot_be_closed_8_1() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("CLOSE C", &mut p).unwrap();
    assert_eq!(s.execute("CLOSE C", &mut p).unwrap_err().code, Code::Cursor); // [X3.135-1986 §8.1 GR1]
}

/// Verifies: [X3.135-1986 §8.1 SR1, §8.8 SR1] CLOSE and OPEN must name a declared cursor.
#[test]
fn open_and_close_name_a_declared_cursor_8_1_8_8() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "CLOSE NOPE"), Err(Code::SyntaxRule)); // [X3.135-1986 §8.1 SR1]
    assert_eq!(exec(&s, "OPEN NOPE"), Err(Code::SyntaxRule)); // [X3.135-1986 §8.8 SR1]
}

/// Verifies: [X3.135-1986 §8.8 GR3a] a parameter's value is taken when the
/// cursor opens, so reopening uses its new value.
#[test]
fn open_takes_the_parameter_values_8_8() {
    let db = setup();
    let s = db.session("APP");
    declare_c(&s);
    let mut p = targets();
    // [X3.135-1986 §8.8 GR3a] LIMIT's value is taken when the cursor opens.
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("CLOSE C", &mut p).unwrap();
    // Reopening re-evaluates with the new value of LIMIT.
    p.set("LIMIT", Value::Exact(Decimal::new(2, 0)));
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C, &mut p).unwrap();
    assert_eq!(s.execute(FETCH_C, &mut p).unwrap(), Completion::NoData);
}

/// Declares cursor C on ID and NAME of T, highest ID first, and opens it.
fn open_c_for_targets(s: &Session) -> Params {
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID, NAME FROM T ORDER BY ID DESC").unwrap();
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    p
}

/// Verifies: [X3.135-1986 §8.6 SR2] FETCH needs one target per column.
#[test]
fn fetch_needs_one_target_per_column_8_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = open_c_for_targets(&s);
    assert_eq!(s.execute("FETCH C INTO ID_OUT", &mut p).unwrap_err().code, Code::SyntaxRule); // SR2
}

/// Verifies: [X3.135-1986 §8.6 SR3] each target's type must suit its
/// column: [ISO/IEC 9075:1992 §9.1 SR2, GR3k] a character string a
/// character string, a number a number, an approximate one into an exact
/// target rounded, SQL-86's exact-only rule being Entry SQL's [§13.3,
/// §13.5, Leveling Rules].
#[test]
fn fetch_targets_must_suit_their_columns_8_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = open_c_for_targets(&s);
    assert_eq!(s.execute("FETCH C INTO NAME_OUT, ID_OUT", &mut p).unwrap_err().code, Code::SyntaxRule); // SR3
    let mut q = targets().declare("AMT_OUT", DataType::Exact { precision: 8, scale: 2, spelled: Spelled::Numeric }, Value::Null);
    assert_eq!(s.execute("SELECT F * 1.0005E0 INTO AMT_OUT FROM T WHERE ID = 1", &mut q).unwrap(), Completion::Success);
    assert_eq!(q.value("AMT_OUT").to_string(), "1.50");
    assert_eq!(s.execute("SELECT 1E10 INTO AMT_OUT FROM T WHERE ID = 1", &mut q).unwrap_err().code, Code::Data, "GR3k");
}

/// Verifies: [X3.135-1986 §7.3 SR3] an indicator must be a declared parameter.
#[test]
fn an_indicator_must_be_declared_7_3() {
    let db = setup();
    let s = db.session("APP");
    let mut p = open_c_for_targets(&s);
    assert_eq!(s.execute("FETCH C INTO ID_OUT, NAME_OUT INDICATOR NOPE", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §7.3 SR3]
}

/// Verifies: [X3.135-1986 §5.6 SR2] an indicator parameter is exact numeric.
#[test]
fn an_indicator_must_be_exact_numeric_5_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = open_c_for_targets(&s);
    assert_eq!(s.execute("FETCH C INTO ID_OUT, ID_OUT INDICATOR NAME_OUT", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §5.6 SR2]
}

/// Verifies: [X3.135-1986 §8.6 GR8] fetching a null into a target with no indicator is an error.
#[test]
fn fetch_of_a_null_needs_an_indicator_8_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = open_c_for_targets(&s);
    // [X3.135-1986 §8.6 GR8] a null with no indicator is an error.
    assert_eq!(s.execute("FETCH C INTO ID_OUT, NAME_OUT", &mut p).unwrap_err().code, Code::Data);
}

/// Verifies: [X3.135-1986 §8.6 GR6, GR8b, GR10a, GR10d] a CHARACTER value of
/// exactly the target's length is copied whole with indicator 0, and an exact
/// number that would lose leading digits in its target is an error.
#[test]
fn fetch_assigns_by_the_type_of_the_target() {
    let db = setup();
    let s = db.session("APP");
    s.declare_cursor("DECLARE N CURSOR FOR SELECT NAME FROM T WHERE ID = 1").unwrap();
    s.declare_cursor("DECLARE A CURSOR FOR SELECT AMT FROM T WHERE ID = 1").unwrap();
    let mut p = Params::new().declare("NAME6", DataType::Character { length: 6 }, Value::Null).indicator("NAME6_IND", 0).declare(
        "ONE_DIGIT",
        DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Numeric },
        Value::Null,
    );
    s.execute("OPEN N", &mut p).unwrap();
    s.execute("FETCH N INTO NAME6 INDICATOR NAME6_IND", &mut p).unwrap();
    assert_eq!((p.value("NAME6").to_string(), p.indicator_value("NAME6_IND")), ("ANN   ".into(), Some(0)));
    s.execute("OPEN A", &mut p).unwrap();
    assert_eq!(s.execute("FETCH A INTO ONE_DIGIT", &mut p).map_err(|e| e.code), Err(Code::Data)); // 10.50 needs 2 digits
}

/// Verifies: [X3.135-1986 §8.2 GR2, §8.9 GR2, §4.12]
#[test]
fn commit_closes_cursors_8_2_gr2() {
    let db = setup();
    let s = db.session("APP");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T").unwrap();
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    let mut anonymous = s.open_cursor("SELECT ID FROM T").unwrap();
    s.commit().unwrap();
    assert_eq!(s.execute("FETCH C INTO ID_OUT", &mut p).unwrap_err().code, Code::Cursor);
    assert_eq!(anonymous.fetch().unwrap_err().code, Code::Cursor);
    s.execute("OPEN C", &mut p).unwrap(); // [X3.135-1986 §4.12] closed again, so it can be reopened
    s.rollback().unwrap();
    assert_eq!(s.execute("FETCH C INTO ID_OUT", &mut p).unwrap_err().code, Code::Cursor); // [X3.135-1986 §8.9 GR2]
}

/// The fetch used by the positioned UPDATE and DELETE tests on cursor C of `positioned_cursors`.
const FETCH_C_AMT: &str = "FETCH C INTO ID_OUT, AMT_OUT INDICATOR AMT_IND";

/// Declares the updatable cursor C (ID, AMT of rows 1 and 2) and the read-only
/// cursor R (ordered IDs), and returns parameters for fetching from them.
fn positioned_cursors(s: &Session) -> Params {
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID, AMT FROM T WHERE ID < 3").unwrap();
    s.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM T ORDER BY ID").unwrap();
    targets().declare("AMT_OUT", DataType::Exact { precision: 8, scale: 2, spelled: Spelled::Numeric }, Value::Null).indicator("AMT_IND", 0)
}

/// Verifies: [X3.135-1986 §8.4 GR1] a cursor that is not open is not on a row,
/// so a positioned DELETE through it fails.
#[test]
fn positioned_delete_needs_an_open_cursor_8_4() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    // [X3.135-1986 §8.4 GR1] a cursor that is not open is not on a row.
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Cursor);
}

/// Verifies: [X3.135-1986 §8.11 GR1] a positioned UPDATE needs the cursor on a row.
#[test]
fn positioned_update_needs_a_current_row_8_11() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    // [X3.135-1986 §8.11 GR1] not yet on a row.
    assert_eq!(s.execute("UPDATE T SET AMT = 0 WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Cursor);
}

/// Verifies: [X3.135-1986 §8.11 GR2, GR3] a positioned UPDATE changes the row under the cursor.
#[test]
fn positioned_update_changes_the_current_row_8_11() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    let first = p.value("ID_OUT").to_string();
    s.execute("UPDATE T SET AMT = AMT + 100 WHERE CURRENT OF C", &mut p).unwrap();
    // [X3.135-1986 §8.11 GR2, GR3] the row under the cursor is the one changed: 10.50 or -3.00, plus 100.
    let updated = if first == "1" { "110.50" } else { "97.00" };
    assert_eq!(rows(&s, &format!("SELECT AMT FROM T WHERE ID = {first}")), Ok(vec![updated.to_owned()]));
}

/// Verifies: [X3.135-1986 §4.12] a positioned DELETE removes the current row and
/// leaves the cursor before the next row, not on one.
#[test]
fn positioned_delete_leaves_the_cursor_before_the_next_row_4_12() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    let first = p.value("ID_OUT").to_string();
    s.execute("DELETE FROM T WHERE CURRENT OF C", &mut p).unwrap();
    // [X3.135-1986 §4.12] after the delete the cursor is before the next row, not on one.
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Cursor);
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    assert_ne!(p.value("ID_OUT").to_string(), first);
    assert_eq!(count(&s, "SELECT ID FROM T"), 2);
}

/// Verifies: [X3.135-1986 §8.4 SR4] a positioned DELETE names the cursor's table.
#[test]
fn positioned_delete_names_the_cursors_table_8_4() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    assert_eq!(s.execute("DELETE FROM U WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §8.4 SR4]
}

/// Verifies: [X3.135-1986 §8.4 SR2] a positioned DELETE names a declared cursor.
#[test]
fn positioned_delete_names_a_declared_cursor_8_4() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF NOPE", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §8.4 SR2]
}

/// Verifies: [X3.135-1986 §8.4 SR3, §8.11 SR3] a read-only cursor allows no
/// positioned DELETE or UPDATE.
#[test]
fn a_read_only_cursor_allows_no_positioned_change_8_4_8_11() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN R", &mut p).unwrap();
    s.execute("FETCH R INTO ID_OUT", &mut p).unwrap();
    assert_eq!(s.execute("DELETE FROM T WHERE CURRENT OF R", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §8.4 SR3] ORDER BY makes it read-only
    assert_eq!(s.execute("UPDATE T SET AMT = 0 WHERE CURRENT OF R", &mut p).unwrap_err().code, Code::SyntaxRule); // [X3.135-1986 §8.11 SR3]
}

/// Verifies: [X3.135-1986 §8.11 SR8a] a positioned UPDATE may not set a NOT
/// NULL column to null: [ISO/IEC 9075:1992 §11.4 SR9a] its NOT NULL
/// constraint is violated as the statement ends.
#[test]
fn positioned_update_keeps_not_null_8_11() {
    let db = setup();
    let s = db.session("APP");
    let mut p = positioned_cursors(&s);
    s.execute("OPEN C", &mut p).unwrap();
    s.execute(FETCH_C_AMT, &mut p).unwrap();
    assert_eq!(s.execute("UPDATE T SET ID = NULL WHERE CURRENT OF C", &mut p).unwrap_err().code, Code::Constraint);
    assert_eq!(s.execute("UPDATE T SET AMT = F WHERE CURRENT OF C", &mut p).unwrap(), Completion::Success, "§13.9 SR10, §9.2 SR2");
}

/// Verifies: [X3.135-1986 §4.12]
#[test]
fn deleted_rows_are_skipped_4_12() {
    let db = setup();
    let s = db.session("APP");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T").unwrap();
    let mut p = targets();
    s.execute("OPEN C", &mut p).unwrap();
    s.execute("FETCH C INTO ID_OUT", &mut p).unwrap();
    let seen = p.value("ID_OUT").to_string();
    // Delete every other row through a searched DELETE in the same transaction.
    s.execute(&format!("DELETE FROM T WHERE ID <> {seen}"), &mut Params::new()).unwrap();
    assert_eq!(s.execute("FETCH C INTO ID_OUT", &mut p).unwrap(), Completion::NoData);
}

/// Verifies: [X3.135-1986 §8.10] SELECT INTO assigns the values of its one row to the targets;
/// [ISO/IEC 9075:1992 §9.1 GR3b] a name cut to fit its target is the warning 01004.
#[test]
fn select_into_assigns_the_row_8_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = targets();
    let done = s.execute("SELECT ID, NAME INTO ID_OUT, NAME_OUT NAME_IND FROM T WHERE ID = 1", &mut p).unwrap();
    assert_eq!(done, Completion::Warning(SqlState::STRING_TRUNCATED));
    assert_eq!(s.execute("SELECT ID INTO ID_OUT FROM T WHERE ID = 1", &mut p).unwrap(), Completion::Success);
    assert_eq!(p.value("ID_OUT").to_string(), "1");
}

/// Verifies: [X3.135-1986 §8.10 GR2] SELECT INTO with no row gives SQLCODE 100
/// and the targets keep their values.
#[test]
fn select_into_of_no_row_gives_sqlcode_100_8_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = targets();
    s.execute("SELECT ID, NAME INTO ID_OUT, NAME_OUT NAME_IND FROM T WHERE ID = 1", &mut p).unwrap();
    // [X3.135-1986 §8.10 GR2] no row: SQLCODE 100 and the targets keep their values.
    assert_eq!(s.execute("SELECT ID INTO ID_OUT FROM T WHERE ID = 99", &mut p).unwrap(), Completion::NoData);
    assert_eq!(p.value("ID_OUT").to_string(), "1");
}

/// Verifies: [X3.135-1986 §8.10 GR2] SELECT INTO of more than one row is an error.
#[test]
fn select_into_of_several_rows_is_an_error_8_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = targets();
    // [X3.135-1986 §8.10 GR2] more than one row is an error.
    assert_eq!(s.execute("SELECT ID INTO ID_OUT FROM T", &mut p).unwrap_err().code, Code::Cardinality);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.5, Leveling Rules] SELECT
/// INTO may group, which SQL-86 refuses [X3.135-1986 §8.10 SR2], giving one
/// row at most; set functions alone give one row.
#[test]
fn select_into_may_group_13_5() {
    let db = setup();
    let s = db.session("APP");
    let mut p = targets();
    assert_eq!(s.execute("SELECT COUNT(*) INTO ID_OUT FROM T GROUP BY NAME", &mut p).unwrap_err().code, Code::Cardinality, "three groups");
    s.execute("SELECT COUNT(*) INTO ID_OUT FROM T WHERE ID = 1 GROUP BY NAME", &mut p).unwrap();
    assert_eq!(p.value("ID_OUT").to_string(), "1");
    assert_eq!(s.execute("SELECT COUNT(*) INTO ID_OUT FROM T", &mut p).unwrap(), Completion::Success);
    assert_eq!(p.value("ID_OUT").to_string(), "3");
}

/// Verifies: [X3.135-1986 §8.10] a select statement needs INTO.
#[test]
fn a_select_statement_needs_into_8_10() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "SELECT ID FROM T"), Err(Code::Syntax)); // a <select statement> needs INTO
}

/// Verifies: [DESIGN] `MAX_CHARACTER_LENGTH` applies to parameters declared
/// from Rust as to columns: a longer one is refused with SQLCODE -910 when used
/// (`CHAR(4000000000)` was once accepted, and each value padded to it).
#[test]
fn character_parameters_are_bounded_too() {
    let db = setup();
    let s = db.session("APP");
    let at_most = |length| Params::new().declare("C", DataType::Character { length }, Value::Null);
    assert!(s.execute("COMMIT WORK", &mut at_most(MAX_CHARACTER_LENGTH)).is_ok());
    assert_eq!(s.execute("COMMIT WORK", &mut at_most(MAX_CHARACTER_LENGTH + 1)).map_err(|e| e.code), Err(Code::Api));
}

/// Verifies: [X3.135-1986 §5.6 GR1] a negative indicator makes the parameter's value null.
#[test]
fn a_negative_indicator_makes_the_value_null_5_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = Params::new()
        .declare("ID", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(7, 0)))
        .declare("N", DataType::Character { length: 6 }, Value::Char("ZED".into()))
        .indicator("NI", -1);
    // [X3.135-1986 §5.6 GR1] a negative indicator makes the value null.
    s.execute("INSERT INTO T (ID, NAME) VALUES (ID, N INDICATOR NI)", &mut p).unwrap();
    assert_eq!(rows(&s, "SELECT NAME FROM T WHERE ID = 7"), Ok(vec!["NULL".into()]));
}

/// Verifies: [X3.135-1986 §7.3 SR4] where a parameter and a column share a name,
/// the unqualified name is the parameter, so the column must be qualified.
#[test]
fn an_unqualified_name_is_the_parameter_7_3() {
    let db = setup();
    let s = db.session("APP");
    let mut p = Params::new()
        .declare("ID", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(8, 0)))
        .declare("N", DataType::Character { length: 6 }, Value::Char("ZED".into()))
        .indicator("NI", 0);
    s.execute("INSERT INTO T (ID, NAME) VALUES (ID, N NI)", &mut p).unwrap();
    // [X3.135-1986 §7.3 SR4] the column ID must be qualified, since a parameter is named ID.
    let mut out = targets();
    out = out.declare("ID", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(8, 0)));
    s.execute("SELECT NAME INTO NAME_OUT NAME_IND FROM T WHERE T.ID = ID", &mut out).unwrap();
    assert_eq!((out.value("NAME_OUT").to_string(), out.indicator_value("NAME_IND")), ("ZE".into(), Some(6)));
    // ...so an unqualified ID is the parameter: ID = 8 holds for every row, not just row 8.
    s.execute("SELECT COUNT(*) INTO ID_OUT FROM T WHERE ID = 8", &mut out).unwrap();
    assert_eq!(out.value("ID_OUT").to_string(), count(&s, "SELECT ID FROM T").to_string());
}

/// Verifies: [X3.135-1986 §5.4, §7.3 SR2] parameter names are identifiers, each declared once.
#[test]
fn parameter_names_are_identifiers_declared_once_7_3() {
    let db = setup();
    let s = db.session("APP");
    // Names must be <identifier>s, declared once [X3.135-1986 §5.4, §7.3 SR2].
    let mut bad = Params::new().declare("lower", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    assert_eq!(s.execute("COMMIT WORK", &mut bad).unwrap_err().code, Code::Api);
    let mut twice = Params::new().indicator("I", 0).indicator("I", 0);
    assert_eq!(s.execute("COMMIT WORK", &mut twice).unwrap_err().code, Code::Api);
}

/// Verifies: [X3.135-1986 §8.8 GR3a, §8.4 SR3]
///
/// Regression test for a defect found in review: the positioned
/// statement re-parsed the cursor's text with its own parameters, and failed
/// on the opener's INDICATOR.
#[test]
fn positioned_update_needs_only_its_own_parameters() {
    let db = setup();
    let s = db.session("APP");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM T WHERE ID > LIM INDICATOR LI").unwrap();
    let mut open = Params::new()
        .declare("LIM", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Exact(Decimal::new(0, 0)))
        .indicator("LI", 0)
        .declare("X", DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.execute("OPEN C", &mut open).unwrap();
    s.execute("FETCH C INTO X", &mut open).unwrap();
    assert_eq!(exec(&s, "UPDATE T SET AMT = 1 WHERE CURRENT OF C"), Ok(Completion::Success));
    let fetched = open.value("X").to_string();
    assert_eq!(rows(&s, &format!("SELECT AMT FROM T WHERE ID = {fetched}")), Ok(vec!["1.00".to_owned()]));
    // A read-only cursor is reported as such even before it is opened.
    s.declare_cursor("DECLARE R CURSOR FOR SELECT ID FROM T ORDER BY ID").unwrap();
    assert_eq!(exec(&s, "DELETE FROM T WHERE CURRENT OF R"), Err(Code::SyntaxRule));
}

/// Verifies: [X3.135-1986 §5.6 SR2, GR1, §5.4]; [DESIGN] a parameter's
/// argument must suit its declared type. Robustness cases for each check made
/// when a parameter is used.
#[test]
fn parameters_are_checked_when_used_5_6() {
    let db = setup();
    let s = db.session("APP");
    let exact = DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric };
    let short = DataType::Character { length: 2 };
    let insert = |p: &mut Params, sql: &str| s.execute(sql, p).map_err(|e| e.code);
    // §5.6 SR2: an indicator parameter is exact numeric with scale 0.
    let mut p = Params::new().declare("N", short, Value::Char("AB".into())).declare("NI", short, Value::Char("X".into()));
    assert_eq!(insert(&mut p, "INSERT INTO T (ID, NAME) VALUES (9, N INDICATOR NI)"), Err(Code::SyntaxRule));
    // §5.6 GR1: a null argument needs a negative indicator.
    let mut p = Params::new().declare("N", short, Value::Null);
    assert_eq!(insert(&mut p, "INSERT INTO T (ID, NAME) VALUES (9, N)"), Err(Code::Api));
    // [DESIGN] an argument that does not fit its declared type is refused.
    let mut p = Params::new().declare("N", short, Value::Char("ABCDE".into()));
    assert_eq!(insert(&mut p, "INSERT INTO T (ID, NAME) VALUES (9, N)"), Err(Code::Api));
    let mut p = Params::new().declare("K", exact, Value::Approx(9.0_f64));
    assert_eq!(insert(&mut p, "INSERT INTO T (ID) VALUES (K)"), Err(Code::Api));
    // §5.4: a parameter name is one <identifier>, not a key word or two words.
    for name in ["SELECT", "A B"] {
        let mut p = Params::new().declare(name, exact, Value::Null);
        assert_eq!(insert(&mut p, "COMMIT WORK"), Err(Code::Api), "{name}");
    }
    // The normal case: an exact argument for an approximate parameter fits.
    let mut p =
        Params::new().declare("K", DataType::Approximate { precision: 24, spelled: Spelled::Float }, Value::Exact(Decimal::new(9, 0)));
    assert_eq!(insert(&mut p, "INSERT INTO T (ID, F) VALUES (9, K)"), Ok(Completion::Success));
}
