//! The diagnostics area [ISO/IEC 9075:1992 §4.19] and GET DIAGNOSTICS
//! [§18.1]: what each statement leaves there, its condition's items, and
//! what GET DIAGNOSTICS refuses.

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlState};
use turborel::params::Params;
use turborel::value::Spelled;
use turborel::value::{DataType, Decimal, Value};

/// Schema A: T, keyed by K, which CHECK POSITIVE keeps above 0; V, those
/// of T below 10, WITH CHECK OPTION; C, whose K references T's. T holds 1,
/// 2 and 3, C 1. P may read, insert into and delete from T.
fn diagnosed() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, N CHAR(2), CONSTRAINT POSITIVE CHECK (K > 0))
           CREATE VIEW V AS SELECT K, N FROM T WHERE K < 10 WITH CHECK OPTION
           CREATE TABLE C (K INTEGER REFERENCES T (K))
           GRANT SELECT, INSERT, DELETE ON T TO P",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'A'), (2, 'B'), (3, 'C')").unwrap();
    a.run("INSERT INTO C VALUES (1)").unwrap();
    a.commit().unwrap();
    db
}

/// Targets: NUM, exact; TXT, CHARACTER(128); J, 1, the condition number;
/// and Q, exact of scale 2.
fn targets() -> Params {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    Params::new()
        .declare("NUM", exact, Value::Null)
        .declare("TXT", DataType::Character { length: 128 }, Value::Null)
        .declare("J", exact, Value::Exact(Decimal::new(1, 0)))
        .declare("Q", DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric }, Value::Null)
}

/// The item `item` of the statement information, as text, its padding
/// trimmed.
fn statement(s: &Session, item: &str) -> String {
    let mut p = targets();
    let target = if ["NUMBER", "ROW_COUNT"].contains(&item) { "NUM" } else { "TXT" };
    s.execute(&format!("GET DIAGNOSTICS {target} = {item}"), &mut p).unwrap();
    p.value(target).to_string().trim_end().to_owned()
}

/// The item `item` of condition 1, as text, its padding trimmed.
fn condition(s: &Session, item: &str) -> String {
    let mut p = targets();
    let numeric = ["CONDITION_NUMBER", "MESSAGE_LENGTH", "MESSAGE_OCTET_LENGTH"].contains(&item);
    let target = if numeric { "NUM" } else { "TXT" };
    s.execute(&format!("GET DIAGNOSTICS EXCEPTION J {target} = {item}"), &mut p).unwrap();
    p.value(target).to_string().trim_end().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.19, §18.1 GR1] before any
/// statement the area holds no condition; after each, one, MORE being N;
/// COMMAND_FUNCTION names the statement (Table 22), none for one that is
/// not SQL, and DYNAMIC_FUNCTION the prepared statement an EXECUTE ran;
/// ROW_COUNT is the rows an INSERT, a searched UPDATE or DELETE changed,
/// all of the table with no WHERE, 0 for none; GET DIAGNOSTICS changes
/// nothing of the area.
#[test]
fn statement_information_18_1() {
    let db = diagnosed();
    let a = db.session("A");
    assert_eq!(statement(&a, "NUMBER"), "0", "nothing yet");
    a.run("INSERT INTO T VALUES (4, 'D'), (5, 'E')").unwrap();
    assert_eq!((statement(&a, "NUMBER"), statement(&a, "MORE")), ("1".into(), "N".into()));
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), statement(&a, "ROW_COUNT")), ("INSERT".into(), "2".into()), "again: unchanged");
    assert_eq!(statement(&a, "DYNAMIC_FUNCTION"), "");
    a.run("UPDATE T SET N = 'Z' WHERE K > 3").unwrap();
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), statement(&a, "ROW_COUNT")), ("UPDATE WHERE".into(), "2".into()));
    a.run("UPDATE T SET N = 'Z' WHERE K > 9").unwrap();
    assert_eq!((statement(&a, "ROW_COUNT"), condition(&a, "RETURNED_SQLSTATE")), ("0".into(), "02000".into()), "no data");
    a.run("DELETE FROM C").unwrap();
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), statement(&a, "ROW_COUNT")), ("DELETE WHERE".into(), "1".into()));
    a.run("DROP VIEW V CASCADE").unwrap();
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "DROP VIEW");
    a.commit().unwrap();
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "COMMIT WORK", "through the Rust API too");
    drop(a.open_cursor("SELECT K FROM T").unwrap());
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "SELECT");
    let insert = a.prepare("INSERT INTO T VALUES (?, 'F')").unwrap();
    a.execute_prepared(&insert, &[Value::Exact(Decimal::new(6, 0))]).unwrap();
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), statement(&a, "DYNAMIC_FUNCTION")), ("EXECUTE".into(), "INSERT".into()));
    assert!(a.run("NOT A STATEMENT").is_err());
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), condition(&a, "RETURNED_SQLSTATE")), (String::new(), "2A000".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.18, §18.1 GR2, GR3a–GR3d,
/// GR3k–GR3o] successful completion is a condition, of SQLSTATE 00000 and
/// no message; an exception gives its SQLSTATE, its message and its
/// lengths; the standard's classes are of ISO 9075, this
/// implementation's of TURBOREL; there is no server or connection name.
#[test]
fn condition_information_18_1() {
    let db = diagnosed();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (7, 'G')").unwrap();
    assert_eq!(condition(&a, "CONDITION_NUMBER"), "1");
    assert_eq!(condition(&a, "RETURNED_SQLSTATE"), "00000");
    assert_eq!((condition(&a, "CLASS_ORIGIN"), condition(&a, "SUBCLASS_ORIGIN")), ("ISO 9075".into(), "ISO 9075".into()));
    assert_eq!((condition(&a, "MESSAGE_TEXT"), condition(&a, "MESSAGE_LENGTH")), (String::new(), "0".into()));
    assert_eq!((condition(&a, "SERVER_NAME"), condition(&a, "CONNECTION_NAME")), (String::new(), String::new()));
    let failed = a.run("INSERT INTO T VALUES (8, 'Ü'), (8, 'H')").unwrap_err();
    assert_eq!(condition(&a, "RETURNED_SQLSTATE"), "23000");
    let message = failed.described();
    assert_eq!(condition(&a, "MESSAGE_TEXT"), message);
    assert_eq!(condition(&a, "MESSAGE_LENGTH"), message.chars().count().to_string());
    assert_eq!(condition(&a, "MESSAGE_OCTET_LENGTH"), message.len().to_string());
    a.commit().unwrap();
    let other = db.session("A");
    other.run("DELETE FROM T WHERE K = 7").unwrap();
    assert_eq!(a.run("DELETE FROM T WHERE K = 7").err().map(|e| e.code), Some(Code::Conflict));
    assert_eq!((condition(&a, "RETURNED_SQLSTATE"), condition(&a, "CLASS_ORIGIN")), ("TB911".into(), "TURBOREL".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §18.1 GR3f–GR3j] a constraint
/// broken is named, with its catalog and schema, and its table; a view
/// whose CHECK OPTION a row breaks, and a cursor in an invalid state, are
/// named; a table of an access rule violation is named; outside that
/// class, a table the identifier holds no privilege on is not, nor its
/// constraint.
#[test]
fn what_a_condition_names_18_1() {
    let db = diagnosed();
    let a = db.session("A");
    assert!(a.run("INSERT INTO T VALUES (-1, 'X')").is_err());
    let names = ["CONSTRAINT_CATALOG", "CONSTRAINT_SCHEMA", "CONSTRAINT_NAME", "CATALOG_NAME", "SCHEMA_NAME", "TABLE_NAME"];
    let given = |s: &Session| names.map(|item| condition(s, item));
    assert_eq!(given(&a), ["TURBOREL", "A", "POSITIVE", "TURBOREL", "A", "T"]);
    assert!(a.run("INSERT INTO V VALUES (11, 'K')").is_err());
    assert_eq!((condition(&a, "RETURNED_SQLSTATE"), condition(&a, "TABLE_NAME")), ("44000".into(), "V".into()));
    a.declare_cursor("DECLARE CR CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = targets();
    assert!(a.execute("FETCH CR INTO NUM", &mut p).is_err());
    assert_eq!((condition(&a, "RETURNED_SQLSTATE"), condition(&a, "CURSOR_NAME")), ("24000".into(), "CR".into()));
    let p = db.session("P");
    assert!(p.run("DELETE FROM A.C").is_err());
    assert_eq!((condition(&p, "RETURNED_SQLSTATE"), condition(&p, "TABLE_NAME")), ("2A000".into(), "C".into()), "access rule");
    let refused = p.run("DELETE FROM A.T WHERE K = 1").unwrap_err();
    assert_eq!(refused.code, Code::Constraint, "C references it: {refused:?}");
    assert_eq!(condition(&p, "RETURNED_SQLSTATE"), "23000");
    assert_eq!(given(&p), ["", "", "", "", "", ""], "P holds no privilege on C");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §18.1 SR1, SR2, GR2; §17.6]
/// refused: a target not of its item's type (SQLCODE -103); a condition
/// number not exact with scale 0 (-103), or of no condition (-301,
/// SQLSTATE 35000); an indicator on a target, or an item of no name, is a
/// syntax error; GET DIAGNOSTICS is not prepared (SQLSTATE 37000).
#[test]
fn what_get_diagnostics_refuses_18_1() {
    let db = diagnosed();
    let a = db.session("A");
    let mut p = targets();
    let code = |sql: &str, p: &mut Params| a.execute(sql, p).err().map(|e| (e.code, e.sqlstate().as_str().to_owned()));
    assert_eq!(code("GET DIAGNOSTICS TXT = NUMBER", &mut p).map(|c| c.0), Some(Code::SyntaxRule), "SR1");
    assert_eq!(code("GET DIAGNOSTICS Q = ROW_COUNT", &mut p).map(|c| c.0), Some(Code::SyntaxRule), "SR1: a scale");
    assert_eq!(code("GET DIAGNOSTICS NUM = MORE", &mut p).map(|c| c.0), Some(Code::SyntaxRule), "SR1");
    a.run("INSERT INTO T VALUES (9, 'I')").unwrap();
    assert_eq!(code("GET DIAGNOSTICS EXCEPTION 1.5 TXT = MESSAGE_TEXT", &mut p).map(|c| c.0), Some(Code::SyntaxRule), "SR2");
    for n in ["0", "2"] {
        let refused = code(&format!("GET DIAGNOSTICS EXCEPTION {n} TXT = MESSAGE_TEXT"), &mut p);
        assert_eq!(refused, Some((Code::Data, "35000".to_owned())), "GR2: {n}");
    }
    p = p.indicator("IND", 0);
    assert_eq!(code("GET DIAGNOSTICS NUM INDICATOR IND = NUMBER", &mut p).map(|c| c.0), Some(Code::Syntax));
    assert_eq!(code("GET DIAGNOSTICS NUM = NOTHING", &mut p).map(|c| c.0), Some(Code::Syntax));
    assert_eq!(statement(&a, "COMMAND_FUNCTION"), "INSERT", "the refusals left the area");
    let prepared = a.prepare("GET DIAGNOSTICS ? = NUMBER").err().map(|e| e.sqlstate().as_str().to_owned());
    assert_eq!(prepared.as_deref(), Some("37000"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 GR8, §18.1 GR3e] a
/// referential action on a row updated through a cursor still open
/// completes with the warning cursor operation conflict (SQLSTATE 01001),
/// naming the cursor; once it is closed, none.
#[test]
fn a_cursor_operation_conflict_11_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY)
           CREATE TABLE R (K INTEGER REFERENCES P ON UPDATE CASCADE, N CHAR(1))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO P VALUES (1), (2)").unwrap();
    a.run("INSERT INTO R VALUES (1, 'A'), (2, 'B')").unwrap();
    a.declare_cursor("DECLARE CR CURSOR FOR SELECT K FROM R WHERE K = 1 FOR UPDATE OF N").unwrap();
    let mut p = targets();
    a.execute("OPEN CR", &mut p).unwrap();
    a.execute("FETCH CR INTO NUM", &mut p).unwrap();
    a.execute("UPDATE R SET N = 'Z' WHERE CURRENT OF CR", &mut p).unwrap();
    let done = a.run("UPDATE P SET K = 3 WHERE K = 1").unwrap();
    assert_eq!(done.sqlstate().as_str(), "01001");
    assert_eq!((condition(&a, "RETURNED_SQLSTATE"), condition(&a, "CURSOR_NAME")), ("01001".into(), "CR".into()));
    a.execute("CLOSE CR", &mut p).unwrap();
    assert_eq!(a.run("UPDATE P SET K = 4 WHERE K = 3").unwrap().sqlstate().as_str(), "00000", "closed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §18.1 GR3g] a syntax error or
/// access rule violation for an inaccessible column names it with its
/// table: one a privilege is refused on, or one a named table does not
/// have; none for the privilege on a whole table.
#[test]
fn an_inaccessible_column_is_named_18_1() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER) GRANT SELECT, UPDATE (K) ON T TO B").unwrap();
    let b = db.session("B");
    let named = |s: &Session| ["RETURNED_SQLSTATE", "TABLE_NAME", "COLUMN_NAME"].map(|item| condition(s, item));
    assert!(b.run("UPDATE A.T SET N = 1").is_err());
    assert_eq!(named(&b), ["2A000", "T", "N"], "UPDATE(N)");
    assert!(crate::common::rows(&b, "SELECT A.T.M FROM A.T").is_err());
    assert_eq!(named(&b), ["2A000", "T", "M"], "no column M");
    assert!(b.run("UPDATE A.T SET M = 1").is_err());
    assert_eq!(named(&b), ["2A000", "T", "M"], "no column M to set");
    assert!(b.run("INSERT INTO A.T (M) VALUES (1)").is_err());
    assert_eq!(named(&b), ["2A000", "T", "M"], "no column M to insert");
    assert!(b.run("DELETE FROM A.T").is_err());
    assert_eq!(named(&b), ["2A000", "T", ""], "DELETE, on the table");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §9.1 GR3b, §18.1 GR1a] a string
/// a SELECT INTO cuts to fit its target is a warning, 01004, which
/// the area keeps; GET DIAGNOSTICS cutting one into its own target
/// completes with that warning too, and leaves the area as it was.
#[test]
fn a_string_cut_to_fit_is_a_warning() {
    let db = diagnosed();
    let a = db.session("A");
    let mut p = targets().declare("SHORT", DataType::Character { length: 1 }, Value::Null);
    let done = a.execute("SELECT N INTO SHORT FROM T WHERE K = 1", &mut p).unwrap();
    assert_eq!((done, p.value("SHORT").to_string()), (Completion::Warning(SqlState::STRING_TRUNCATED), "A".to_owned()));
    assert_eq!(condition(&a, "RETURNED_SQLSTATE"), "01004");
    let done = a.execute("GET DIAGNOSTICS SHORT = COMMAND_FUNCTION", &mut p).unwrap();
    assert_eq!((done, p.value("SHORT").to_string()), (Completion::Warning(SqlState::STRING_TRUNCATED), "S".to_owned()));
    assert_eq!((statement(&a, "COMMAND_FUNCTION"), condition(&a, "RETURNED_SQLSTATE")), ("SELECT".to_owned(), "01004".to_owned()));
}
