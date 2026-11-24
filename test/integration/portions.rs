//! UPDATE and DELETE FOR PORTION OF an application-time period [ISO/IEC
//! 9075-2:2023 §14.9, §14.14]: the rows whose period overlaps the portion
//! are changed, an UPDATE's period clipped to it, and each row's parts
//! outside it kept as rows of their own [§15.8 GR9, §15.14 GR12].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use crate::common::rows;

/// Schema A and its table PRICES of the period VALID, of item 1 priced 10
/// through January 2026, and item 2 priced 20 through February.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    ok(
        &db.session("A"),
        &[
            "CREATE TABLE PRICES (ITEM INTEGER, PRICE INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR VALID (S, E))",
            "INSERT INTO PRICES VALUES (1, 10, DATE '2026-01-01', DATE '2026-02-01'), (2, 20, DATE '2026-02-01', DATE '2026-03-01')",
        ],
    );
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of `sql` in `s`.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// The rows of PRICES, in order of item and start.
fn prices(s: &Session) -> Vec<String> {
    query(s, "SELECT ITEM, PRICE, S, E FROM PRICES ORDER BY ITEM, S")
}

/// The rows the last statement in `s` changed.
fn row_count(s: &Session) -> String {
    let mut p = Params::new().declare("N", DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }, Value::Null);
    s.execute("GET DIAGNOSTICS N = ROW_COUNT", &mut p).unwrap();
    p.value("N").to_string()
}

/// The error `sql` fails with in `s`, rolled back.
fn refused(s: &Session, sql: &str) -> SqlError {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    e
}

/// Asserts `sql` fails in `s` by the Syntax Rule `cited`.
fn refused_by(s: &Session, sql: &str, cited: &str) {
    let e = refused(s, sql);
    assert_eq!(e.code, Code::SyntaxRule, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.9 SR7a iv, GR7; §15.8 GR9]
/// DELETE FOR PORTION OF deletes the rows whose period overlaps the
/// portion, and keeps each one's parts before and after it; a row it
/// does not reach is kept whole; ROW_COUNT counts the rows deleted.
#[test]
fn delete_for_portion_of_keeps_the_rest_14_9() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-01-10' TO DATE '2026-01-20'"]);
    assert_eq!(prices(&s), ["1,10,2026-01-01,2026-01-10", "1,10,2026-01-20,2026-02-01", "2,20,2026-02-01,2026-03-01"]);
    s.run("DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-01-25' TO DATE '2026-02-10' WHERE PRICE > 0").unwrap();
    assert_eq!(row_count(&s), "2");
    s.commit().unwrap();
    assert_eq!(prices(&s), ["1,10,2026-01-01,2026-01-10", "1,10,2026-01-20,2026-01-25", "2,20,2026-02-10,2026-03-01"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.14 SR7a vi; §15.14 GR12]
/// UPDATE FOR PORTION OF changes the part of each row within the portion,
/// its period clipped to it, and keeps the parts outside unchanged; a row
/// within the portion whole is changed whole, keeping nothing.
#[test]
fn update_for_portion_of_splits_its_rows_14_14() {
    let db = database();
    let s = db.session("A");
    s.run("UPDATE PRICES FOR PORTION OF VALID FROM DATE '2026-01-10' TO DATE '2026-01-20' SET PRICE = 12 WHERE ITEM = 1").unwrap();
    assert_eq!(row_count(&s), "1");
    s.commit().unwrap();
    assert_eq!(
        prices(&s),
        ["1,10,2026-01-01,2026-01-10", "1,12,2026-01-10,2026-01-20", "1,10,2026-01-20,2026-02-01", "2,20,2026-02-01,2026-03-01"]
    );
    ok(&s, &["UPDATE PRICES FOR PORTION OF VALID FROM DATE '2025-12-01' TO DATE '2027-01-01' SET PRICE = PRICE + 1 WHERE ITEM = 2"]);
    assert_eq!(query(&s, "SELECT PRICE, S, E FROM PRICES WHERE ITEM = 2"), ["21,2026-02-01,2026-03-01"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.9 SR7a iv; §15.8 GR9c, GR9d]
/// A portion that begins at a row's start keeps nothing before it; one
/// that ends at its end, nothing after. A portion that is empty, or meets
/// a row's period without overlapping it, changes no row (02000).
#[test]
fn a_portion_at_a_bound_keeps_no_empty_part_14_9() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["UPDATE PRICES FOR PORTION OF VALID FROM DATE '2026-01-01' TO DATE '2026-01-15' SET PRICE = 11 WHERE ITEM = 1"]);
    assert_eq!(
        query(&s, "SELECT PRICE, S, E FROM PRICES WHERE ITEM = 1 ORDER BY S"),
        ["11,2026-01-01,2026-01-15", "10,2026-01-15,2026-02-01"]
    );
    for portion in ["DATE '2026-01-05', DATE '2026-01-05'", "DATE '2026-01-09', DATE '2026-01-05'", "DATE '2026-03-01', DATE '2026-04-01'"]
    {
        let (from, to) = portion.split_once(", ").unwrap();
        let sql = format!("DELETE FROM PRICES FOR PORTION OF VALID FROM {from} TO {to}");
        assert_eq!(s.run(&sql).unwrap(), turborel::db::Completion::NoData, "{sql}");
    }
    assert_eq!(prices(&s).len(), 3);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.14 GR12b; §15.11]
/// A part kept is given its identity column's next value and its generated
/// columns again, and is inserted as an INSERT inserts it, an AFTER INSERT
/// trigger executing for it.
#[test]
fn a_part_kept_is_inserted_as_a_row_15_14() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE T (ID INTEGER GENERATED ALWAYS AS IDENTITY, V INTEGER, DAYS INTEGER GENERATED ALWAYS AS (V * 2), \
             S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR P (S, E))",
            "CREATE TABLE LOG (ID INTEGER)",
            "CREATE TRIGGER KEPT AFTER INSERT ON T REFERENCING NEW ROW AS N FOR EACH ROW INSERT INTO LOG VALUES (N.ID)",
            "INSERT INTO T (V, S, E) VALUES (1, DATE '2026-01-01', DATE '2026-01-31')",
            "DELETE FROM LOG",
            "UPDATE T FOR PORTION OF P FROM DATE '2026-01-10' TO DATE '2026-01-20' SET V = 5",
        ],
    );
    assert_eq!(
        query(&s, "SELECT ID, V, DAYS, S, E FROM T ORDER BY S"),
        ["2,1,2,2026-01-01,2026-01-10", "1,5,10,2026-01-10,2026-01-20", "3,1,2,2026-01-20,2026-01-31"]
    );
    assert_eq!(query(&s, "SELECT ID FROM LOG ORDER BY ID"), ["2", "3"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.9 SR7a, §14.14 SR7a ii, iii]
/// Refused: a period the table has not, a view, a SET clause of the
/// period's columns, a point in time of a column of the table, of no
/// datetime or of one not comparable with the period's, and a
/// non-deterministic function in one.
#[test]
fn for_portion_of_is_refused_against_its_rules_14_14() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE VIEW V AS SELECT * FROM PRICES",
            "CREATE FUNCTION SOMEDAY () RETURNS DATE NOT DETERMINISTIC CONTAINS SQL RETURN CURRENT_DATE",
        ],
    );
    let jan = "FROM DATE '2026-01-01' TO DATE '2026-01-05'";
    for (sql, cited) in [
        (format!("DELETE FROM PRICES FOR PORTION OF OTHER {jan}"), "§14.9 SR7a"),
        (format!("DELETE FROM V FOR PORTION OF VALID {jan}"), "§14.9 SR7a"),
        (format!("UPDATE PRICES FOR PORTION OF VALID {jan} SET S = DATE '2026-01-02'"), "§14.14 SR7a ii"),
        ("UPDATE PRICES FOR PORTION OF VALID FROM S TO DATE '2026-01-05' SET PRICE = 1".into(), "§14.14 SR7a iii"),
        ("DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-01-01' TO E".into(), "§14.9 SR7a iii"),
        ("DELETE FROM PRICES FOR PORTION OF VALID FROM 1 TO 2".into(), "§14.9 SR7a ii"),
        ("DELETE FROM PRICES FOR PORTION OF VALID FROM TIMESTAMP '2026-01-01 00:00:00' TO DATE '2026-01-05'".into(), "§8.2 SR1"),
        ("DELETE FROM PRICES FOR PORTION OF VALID FROM SOMEDAY() TO DATE '2027-01-01'".into(), "§14.9 SR7a ii"),
    ] {
        refused_by(&s, &sql, cited);
    }
    assert_eq!(refused(&s, "DELETE FROM PRICES FOR PORTION OF VALID WHERE ITEM = 1").code, Code::Syntax);
    ok(&s, &["DELETE FROM PRICES FOR PORTION OF VALID FROM CURRENT_DATE - INTERVAL '1' DAY TO CURRENT_DATE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.14 SR7a; DESIGN]
/// A `?` point in time is of the period's type, and the statement may be
/// prepared and executed again with other points.
#[test]
fn a_parameter_point_in_time_is_of_the_periods_type_14_14() {
    let db = database();
    let s = db.session("A");
    let p = s.prepare("UPDATE PRICES FOR PORTION OF VALID FROM ? TO ? SET PRICE = PRICE * 10 WHERE ITEM = 1").unwrap();
    assert_eq!(p.inputs().iter().map(|t| t.clone().data_type()).collect::<Vec<_>>(), [DataType::Date, DataType::Date]);
    let day = |d: &str| s.open_cursor(&format!("VALUES DATE '{d}'")).unwrap().fetch().unwrap().unwrap().remove(0);
    s.execute_prepared(&p, &[day("2026-01-10"), day("2026-01-12")]).unwrap();
    s.execute_prepared(&p, &[day("2026-01-20"), day("2026-01-21")]).unwrap();
    s.commit().unwrap();
    assert_eq!(
        query(&s, "SELECT PRICE, S, E FROM PRICES WHERE ITEM = 1 ORDER BY S"),
        [
            "10,2026-01-01,2026-01-10",
            "100,2026-01-10,2026-01-12",
            "10,2026-01-12,2026-01-20",
            "100,2026-01-20,2026-01-21",
            "10,2026-01-21,2026-02-01"
        ]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3b, SR3c, GR2f]
/// DROP PERIOD RESTRICT is refused while a trigger's action or a
/// procedure's body changes its table FOR PORTION OF it; CASCADE drops
/// them.
#[test]
fn drop_period_refused_by_a_portion_11_28() {
    let cases = [
        (
            "CREATE TRIGGER G AFTER INSERT ON PRICES REFERENCING NEW ROW AS N FOR EACH ROW \
             DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2030-01-01' TO DATE '2030-01-02'",
            "SR3b ii, SR3c ii",
            "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_SCHEMA = 'A'",
        ),
        (
            "CREATE PROCEDURE RAISE_JAN () MODIFIES SQL DATA \
             UPDATE PRICES FOR PORTION OF VALID FROM DATE '2026-01-01' TO DATE '2026-02-01' SET PRICE = PRICE + 1",
            "SR3b i, SR3c i",
            "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'",
        ),
    ];
    for (create, cited, count) in cases {
        let db = database();
        let s = db.session("A");
        ok(&s, &[create]);
        refused_by(&s, "ALTER TABLE PRICES DROP PERIOD FOR VALID RESTRICT", cited);
        ok(&s, &["ALTER TABLE PRICES DROP PERIOD FOR VALID CASCADE"]);
        assert_eq!(query(&s, count), ["0"], "{create}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.8 GR9 NOTE 707]
/// The parts kept are inserted without constraint checking, and checked
/// with the rest as the statement ends: a part whose end a CHECK refuses
/// fails the DELETE (23000), and nothing is changed.
#[test]
fn the_parts_kept_are_checked_as_the_statement_ends_15_8() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["ALTER TABLE PRICES ADD CONSTRAINT NOT_TENTH CHECK (E <> DATE '2026-01-10')"]);
    let e = refused(&s, "DELETE FROM PRICES FOR PORTION OF VALID FROM DATE '2026-01-10' TO DATE '2026-01-15'");
    assert_eq!(e.sqlstate().as_str(), "23000", "{e}");
    assert_eq!(prices(&s).len(), 2);
}
