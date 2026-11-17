//! The single-row SELECT [X3.135-1986 §8.10]: its row assigned to its
//! targets, no row SQLCODE 100, more than one an error, and grouping
//! allowed [ISO/IEC 9075:1992 §13.5].

use turborel::db::Completion;
use turborel::error::{Code, SqlState};

use crate::common::{exec, setup};
use crate::cursors::targets;

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
