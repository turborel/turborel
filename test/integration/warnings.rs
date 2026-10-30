//! Warnings an expression raises as a statement runs [ISO/IEC 9075:1992
//! §3.3.4.1]: a CAST that cuts a string or pads a bit string [§6.10], a
//! set function that eliminates a null [§6.5 GR1b]; the statement's
//! completion, read as its rows are read by batches and row by row, in
//! the diagnostics area [§18.1] too.

use turborel::db::{Completion, Database, Session};
use turborel::error::SqlState;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::{exec, rows};

/// Schema A: W, of every kind of column a batch holds, its second row all
/// nulls.
fn warned_db() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE W (K INTEGER, F FLOAT, N VARCHAR(4), C CHAR(3), B BIT(4))
           CREATE TABLE CK (N VARCHAR(4) CHECK (CAST(N AS CHAR(1)) = 'A'))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO W VALUES (1, 1.5E0, 'AB', 'XY', B'1010'), (NULL, NULL, NULL, NULL, NULL), (3, 2.5E0, 'A', 'Z', B'0001')").unwrap();
    a.commit().unwrap();
    db
}

/// The warning the query `sql` completed with, its rows all read, if any.
fn warning(s: &Session, sql: &str) -> Option<&'static str> {
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.warning().map(SqlState::as_str)
}

/// `warning`, read row by row (EXECUTOR.BATCHES OFF), then by batches
/// where it can be, which leaves its condition in the diagnostics area:
/// the two the same, or the query named.
fn both_ways(s: &Session, sql: &str) -> Option<&'static str> {
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    let row_by_row = warning(s, sql);
    exec(s, "SET EXECUTOR.BATCHES = 'ON'").unwrap();
    let batched = warning(s, sql);
    assert_eq!(batched, row_by_row, "{sql}");
    batched
}

/// The SQLSTATE GET DIAGNOSTICS gives of the last statement's condition.
fn returned(s: &Session) -> String {
    let mut p = Params::new().declare("S", DataType::Character { length: 5 }, Value::Null);
    s.execute("GET DIAGNOSTICS EXCEPTION 1 S = RETURNED_SQLSTATE", &mut p).unwrap();
    p.value("S").to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR5c ii, GR6c ii,
/// §3.3.4.1] a CAST that cuts a character string to CHARACTER or
/// CHARACTER VARYING completes with the warning string data, right
/// truncation, 01004, if a character cut is not a space: of a query, read
/// by batches and row by row, of a constant too, which is not folded
/// away; of SELECT ... INTO and INSERT, and of a CHECK constraint the
/// INSERT checks; GET DIAGNOSTICS gives it, and the next statement clears
/// it.
#[test]
fn a_cast_that_cuts_a_character_string_warns_6_10() {
    let db = warned_db();
    let a = db.session("A");
    assert_eq!(both_ways(&a, "SELECT CAST(N AS VARCHAR(1)) FROM W"), Some("01004"), "'AB' cut");
    assert_eq!(returned(&a), "01004");
    assert_eq!(both_ways(&a, "SELECT CAST(C AS CHAR(1)) FROM W"), Some("01004"), "'XY ' cut");
    assert_eq!(both_ways(&a, "SELECT CAST(N AS CHAR(2)), CAST(C AS VARCHAR(2)) FROM W"), None, "only spaces cut");
    assert_eq!(returned(&a), "00000");
    assert_eq!(both_ways(&a, "SELECT CAST('ABC' AS CHAR(2)) FROM W"), Some("01004"), "a constant");
    assert_eq!(both_ways(&a, "SELECT CAST('A  ' AS VARCHAR(1)), CAST('A' AS CHAR(3)) FROM W"), None, "spaces cut, or padded");
    let mut p = Params::new().declare("X", DataType::Character { length: 1 }, Value::Null);
    let done = a.execute("SELECT CAST(N AS VARCHAR(1)) INTO X FROM W WHERE K = 1", &mut p).unwrap();
    assert_eq!((done, p.value("X").to_string()), (Completion::Warning(SqlState::STRING_TRUNCATED), "A".to_owned()));
    assert_eq!(exec(&a, "INSERT INTO W (K, C) VALUES (9, CAST('LONG' AS CHAR(3)))"), Ok(Completion::Warning(SqlState::STRING_TRUNCATED)));
    assert_eq!(rows(&a, "SELECT C FROM W WHERE K = 9").unwrap(), ["LON"]);
    assert_eq!(exec(&a, "DELETE FROM W WHERE K = 9"), Ok(Completion::Success), "cleared");
    let check = Completion::Warning(SqlState::STRING_TRUNCATED);
    assert_eq!(exec(&a, "INSERT INTO CK VALUES ('AB')"), Ok(check), "a CHECK's CAST");
    assert_eq!(exec(&a, "INSERT INTO CK VALUES ('A ')"), Ok(Completion::Success));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR7b, GR7c, GR8b,
/// §3.3.4.1] a CAST to BIT that cuts bits completes with the warning
/// string data, right truncation, 01004, one that pads them with 0 bits
/// with implicit zero-bit padding, 01008; to BIT VARYING, only a cut
/// warns; of a character string's bits too.
#[test]
fn a_cast_that_cuts_or_pads_a_bit_string_warns_6_10() {
    let db = warned_db();
    let a = db.session("A");
    assert_eq!(both_ways(&a, "SELECT CAST(B AS BIT(2)) FROM W"), Some("01004"));
    assert_eq!(both_ways(&a, "SELECT CAST(B AS BIT(6)) FROM W"), Some("01008"));
    assert_eq!(both_ways(&a, "SELECT CAST(B AS BIT VARYING(2)) FROM W"), Some("01004"));
    assert_eq!(both_ways(&a, "SELECT CAST(B AS BIT VARYING(6)), CAST(B AS BIT(4)) FROM W"), None);
    assert_eq!(both_ways(&a, "SELECT CAST(N AS BIT(4)) FROM W"), Some("01004"), "8 bits a character");
    assert_eq!(both_ways(&a, "SELECT CAST('A' AS BIT(8)), CAST('A' AS BIT VARYING(9)) FROM W"), None);
    assert_eq!(returned(&a), "00000");
    assert_eq!(both_ways(&a, "SELECT CAST('A' AS BIT(9)) FROM W"), Some("01008"));
    assert_eq!(returned(&a), "01008");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.10 GR5d, GR6d, §3.3.4.1] a
/// CAST of a bit string to a character string warns of implicit zero-bit
/// padding, 01008, if its bits are not a whole number of characters, or a
/// fixed length adds characters; of string data, right truncation, 01004,
/// if characters are cut.
#[test]
fn a_cast_of_a_bit_string_to_characters_warns_6_10() {
    let db = warned_db();
    let a = db.session("A");
    assert_eq!(both_ways(&a, "SELECT CAST(X'4142' AS CHAR(1)) FROM W"), Some("01004"), "GR5d i");
    assert_eq!(both_ways(&a, "SELECT CAST(X'4142' AS CHAR(3)) FROM W"), Some("01008"), "GR5d ii");
    assert_eq!(both_ways(&a, "SELECT CAST(X'4142' AS VARCHAR(1)) FROM W"), Some("01004"), "GR6d ii");
    assert_eq!(both_ways(&a, "SELECT CAST(B'010000010' AS VARCHAR(2)) FROM W"), Some("01008"), "PAD not 0");
    assert_eq!(both_ways(&a, "SELECT CAST(X'4142' AS CHAR(2)), CAST(X'4142' AS VARCHAR(3)) FROM W"), None);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR1b, §3.3.4.1] a set
/// function whose argument takes out a null completes with the warning
/// null value eliminated in set function, 01003, read by batches and row
/// by row: of a column of each kind a batch holds, of an expression of
/// integers, of approximate numbers, of a constant and of one evaluated a
/// row at a time; grouped or not, DISTINCT or not, in a subquery too.
/// COUNT(*) eliminates none, nor does a set function of no null.
#[test]
fn a_set_function_that_eliminates_a_null_warns_6_5() {
    let db = warned_db();
    let a = db.session("A");
    let columns = ["SUM(K)", "AVG(F)", "MAX(N)", "MIN(C)", "COUNT(B)", "SUM(K + 1)", "SUM(F * 2)", "MAX(CAST(N AS VARCHAR(4)))"];
    for f in columns.iter().chain(&["COUNT(DISTINCT K)"]) {
        assert_eq!(both_ways(&a, &format!("SELECT {f} FROM W")), Some("01003"), "{f}");
        assert_eq!(both_ways(&a, &format!("SELECT {f} FROM W WHERE K IS NOT NULL")), None, "{f}: no null");
        assert_eq!(both_ways(&a, &format!("SELECT {f} FROM W WHERE K > 0 OR K IS NULL")), Some("01003"), "{f}: of rows kept");
        assert_eq!(both_ways(&a, &format!("SELECT K, {f} FROM W GROUP BY K")), Some("01003"), "{f}: grouped");
        assert_eq!(both_ways(&a, &format!("SELECT K, {f} FROM W WHERE K IS NOT NULL GROUP BY K")), None, "{f}: grouped, no null");
    }
    for f in ["COUNT(CAST(NULL AS INTEGER))", "K, COUNT(CAST(NULL AS INTEGER))"] {
        let grouped = if f.starts_with('K') { " GROUP BY K" } else { "" };
        assert_eq!(both_ways(&a, &format!("SELECT {f} FROM W{grouped}")), Some("01003"), "a null constant: {f}");
        assert_eq!(both_ways(&a, &format!("SELECT {f} FROM W WHERE K > 9{grouped}")), None, "of no row: {f}");
    }
    assert_eq!(both_ways(&a, "SELECT COUNT(*) FROM W"), None, "GR1a");
    assert_eq!(returned(&a), "00000");
    assert_eq!(both_ways(&a, "SELECT K FROM W WHERE K < (SELECT SUM(K) FROM W)"), Some("01003"), "in a subquery");
    assert_eq!(returned(&a), "01003");
    let mut p = Params::new().declare("X", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    let done = a.execute("SELECT COUNT(K) INTO X FROM W", &mut p).map_err(|e| e.code);
    assert_eq!((done, p.value("X").to_string()), (Ok(Completion::Warning(SqlState::NULL_VALUE_ELIMINATED)), "2".to_owned()));
    a.rollback().unwrap();
}
