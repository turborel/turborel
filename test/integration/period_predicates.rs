//! Period predicates [ISO/IEC 9075-2:2023 §8.20]: OVERLAPS, EQUALS,
//! CONTAINS, PRECEDES, SUCCEEDS, IMMEDIATELY PRECEDES and IMMEDIATELY
//! SUCCEEDS of periods written PERIOD ( start , end ) or named by a
//! `<period reference>`, CONTAINS of a point in time too; and what DROP
//! PERIOD does to what references a period [§11.28 SR3d, GR2f].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::value::{DataType, Value};

use crate::common::rows;

/// Schema A, its table BOOKINGS of the period BOOKED, of rooms 1, 2 and
/// 3, booked from January 1, 5 and 10 2026 to the 5th, 10th and 20th,
/// and its table ROOMS of rooms 1, 2 and 4.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    ok(
        &db.session("A"),
        &[
            "CREATE TABLE BOOKINGS (ROOM INTEGER, S DATE NOT NULL, E DATE NOT NULL, PERIOD FOR BOOKED (S, E))",
            "INSERT INTO BOOKINGS VALUES (1, DATE '2026-01-01', DATE '2026-01-05'), \
             (2, DATE '2026-01-05', DATE '2026-01-10'), (3, DATE '2026-01-10', DATE '2026-01-20')",
            "CREATE TABLE ROOMS (ROOM INTEGER)",
            "INSERT INTO ROOMS VALUES (1), (2), (4)",
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

/// The error `sql` fails with in `s`, rolled back: a query's as it is
/// opened or read through.
fn refused(s: &Session, sql: &str) -> SqlError {
    let failed = if sql.starts_with("SELECT") {
        s.open_cursor(sql).and_then(|mut c| {
            while c.fetch()?.is_some() {}
            Ok(())
        })
    } else {
        s.run(sql).map(|_| ())
    };
    let e = failed.err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    e
}

/// Asserts `sql` fails in `s` by Syntax Rule `cited`.
fn refused_by(s: &Session, sql: &str, cited: &str) {
    let e = refused(s, sql);
    assert_eq!(e.code, Code::SyntaxRule, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// The rooms of BOOKINGS whose period `predicate` holds of, in order.
fn rooms(s: &Session, predicate: &str) -> Vec<String> {
    query(s, &format!("SELECT ROOM FROM BOOKINGS WHERE {predicate} ORDER BY ROOM"))
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4b]
/// Each predicate of a <period reference> and a period written: a period
/// that meets another overlaps it not, precedes it and immediately
/// precedes it; one of the same bounds equals it.
#[test]
fn each_predicate_of_a_period_reference_8_20() {
    let db = database();
    let s = db.session("A");
    let jan = |a: u32, b: u32| format!("PERIOD (DATE '2026-01-{a:02}', DATE '2026-01-{b:02}')");
    assert_eq!(rooms(&s, &format!("BOOKED OVERLAPS {}", jan(4, 6))), ["1", "2"]);
    assert_eq!(rooms(&s, &format!("BOOKED OVERLAPS {}", jan(10, 12))), ["3"], "2 only meets it");
    assert_eq!(rooms(&s, &format!("BOOKED EQUALS {}", jan(5, 10))), ["2"]);
    assert_eq!(rooms(&s, &format!("BOOKED CONTAINS {}", jan(6, 9))), ["2"]);
    assert_eq!(rooms(&s, &format!("BOOKED PRECEDES {}", jan(10, 12))), ["1", "2"]);
    assert_eq!(rooms(&s, &format!("BOOKED SUCCEEDS {}", jan(1, 5))), ["2", "3"]);
    assert_eq!(rooms(&s, &format!("BOOKED IMMEDIATELY PRECEDES {}", jan(10, 12))), ["2"]);
    assert_eq!(rooms(&s, &format!("BOOKED IMMEDIATELY SUCCEEDS {}", jan(1, 5))), ["2"]);
    assert_eq!(rooms(&s, &format!("{} CONTAINS BOOKED", jan(1, 10))), ["1", "2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4a v]
/// CONTAINS of a point in time takes the start and leaves out the end; a
/// timestamp is compared with dates as TIMESTAMP WITH TIME ZONE; a null
/// point is Unknown, so NOT of it is too.
#[test]
fn contains_a_point_in_time_8_20() {
    let db = database();
    let s = db.session("A");
    assert_eq!(rooms(&s, "BOOKED CONTAINS DATE '2026-01-05'"), ["2"]);
    assert_eq!(rooms(&s, "BOOKED CONTAINS TIMESTAMP '2026-01-04 23:59:59.999999'"), ["1"]);
    assert_eq!(rooms(&s, "BOOKED CONTAINS CAST(NULL AS DATE)"), Vec::<String>::new());
    assert_eq!(rooms(&s, "NOT (BOOKED CONTAINS CAST(NULL AS DATE))"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR2]
/// A written period of a null bound makes the predicate Unknown; one
/// whose start is not before its end fails with 22020.
#[test]
fn a_written_period_is_judged_8_20() {
    let db = database();
    let s = db.session("A");
    assert_eq!(rooms(&s, "BOOKED OVERLAPS PERIOD (CAST(NULL AS DATE), DATE '2026-01-03')"), Vec::<String>::new());
    assert_eq!(rooms(&s, "NOT (BOOKED OVERLAPS PERIOD (CAST(NULL AS DATE), DATE '2026-01-03'))"), Vec::<String>::new());
    for bounds in ["DATE '2026-01-03', DATE '2026-01-03'", "DATE '2026-01-04', DATE '2026-01-03'"] {
        let e = refused(&s, &format!("SELECT ROOM FROM BOOKINGS WHERE BOOKED OVERLAPS PERIOD ({bounds})"));
        assert_eq!(e.sqlstate().as_str(), "22020", "{e}");
    }
    let e = refused(&s, "SELECT ROOM FROM BOOKINGS WHERE PERIOD (E, S) PRECEDES BOOKED");
    assert_eq!(e.sqlstate().as_str(), "22020", "{e}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4a i 2]
/// A period reference qualified by a correlation name, of each of two
/// rows joined; one of an outer query in a subquery; and one of the side
/// of an outer join that has no row, whose bounds are null, so no
/// predicate of it is True.
#[test]
fn period_references_are_resolved_8_20() {
    let db = database();
    let s = db.session("A");
    assert_eq!(
        query(&s, "SELECT X.ROOM, Y.ROOM FROM BOOKINGS X, BOOKINGS Y WHERE X.BOOKED IMMEDIATELY PRECEDES Y.BOOKED ORDER BY X.ROOM"),
        ["1,2", "2,3"]
    );
    assert_eq!(
        query(
            &s,
            "SELECT ROOM FROM BOOKINGS X WHERE EXISTS (SELECT * FROM BOOKINGS Y WHERE Y.ROOM <> X.ROOM AND X.BOOKED SUCCEEDS Y.BOOKED) ORDER BY ROOM"
        ),
        ["2", "3"]
    );
    assert_eq!(
        query(
            &s,
            "SELECT R.ROOM FROM ROOMS R LEFT JOIN BOOKINGS B ON B.ROOM = R.ROOM \
             WHERE NOT (B.BOOKED CONTAINS DATE '2026-01-06') ORDER BY R.ROOM"
        ),
        ["1", "4"]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 SR1, SR2]
/// Refused: a written period of TIME, of two types, or not of
/// datetimes; a point in time of TIME; an operand that is no period, or
/// names a period of two tables; IMMEDIATELY not before PRECEDES or
/// SUCCEEDS.
#[test]
fn period_predicates_are_refused_against_their_rules_8_20() {
    let db = database();
    let s = db.session("A");
    for (predicate, cited) in [
        ("PERIOD (TIME '10:00:00', TIME '11:00:00') OVERLAPS BOOKED", "§8.20 SR1a"),
        ("PERIOD (1, 2) OVERLAPS BOOKED", "§8.20 SR1a"),
        ("PERIOD (DATE '2026-01-01', TIMESTAMP '2026-01-02 00:00:00') OVERLAPS BOOKED", "§8.20 SR1b"),
        ("BOOKED CONTAINS TIME '10:00:00'", "§8.20 SR2"),
        ("ROOM OVERLAPS BOOKED", "names a period"),
        ("BOOKED OVERLAPS DATE '2026-01-01'", "names a period"),
    ] {
        refused_by(&s, &format!("SELECT ROOM FROM BOOKINGS WHERE {predicate}"), cited);
    }
    refused_by(&s, "SELECT X.ROOM FROM BOOKINGS X, BOOKINGS Y WHERE BOOKED CONTAINS DATE '2026-01-01'", "of more than one table");
    assert_eq!(refused(&s, "SELECT ROOM FROM BOOKINGS WHERE BOOKED IMMEDIATELY OVERLAPS BOOKED").code, Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20]
/// EQUALS, CONTAINS, PRECEDES, SUCCEEDS and IMMEDIATELY are no key words
/// of SQL-92: a column or an alias may be so named, and a period
/// predicate is a value of the select list.
#[test]
fn the_words_of_period_predicates_name_columns() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE W (CONTAINS INTEGER, PRECEDES INTEGER, EQUALS INTEGER)", "INSERT INTO W VALUES (1, 2, 3)"]);
    assert_eq!(query(&s, "SELECT CONTAINS, PRECEDES SUCCEEDS, EQUALS IMMEDIATELY FROM W"), ["1,2,3"]);
    assert_eq!(query(&s, "SELECT W.CONTAINS FROM W WHERE CONTAINS = 1"), ["1"]);
    assert_eq!(query(&s, "SELECT ROOM, BOOKED CONTAINS DATE '2026-01-05' FROM BOOKINGS ORDER BY ROOM"), ["1,FALSE", "2,TRUE", "3,FALSE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 SR1b; ISO/IEC 9075:1992 §17.6 GR5]
/// A `?` bound of a written period takes the other bound's type; a `?`
/// point in time, TIMESTAMP WITH TIME ZONE.
#[test]
fn a_parameter_of_a_period_takes_its_type_8_20() {
    let db = database();
    let s = db.session("A");
    let p = s.prepare("SELECT ROOM FROM BOOKINGS WHERE BOOKED OVERLAPS PERIOD (?, DATE '2026-01-06') ORDER BY ROOM").unwrap();
    assert_eq!(p.parameters().iter().map(|t| t.clone().data_type()).collect::<Vec<_>>(), [DataType::Date]);
    let mut cursor = s.open_prepared(&p, &[Value::Null]).unwrap();
    assert!(cursor.fetch().unwrap().is_none(), "Unknown of a null bound");
    let point = s.prepare("SELECT ROOM FROM BOOKINGS WHERE BOOKED CONTAINS ?").unwrap();
    assert_eq!(
        point.parameters().iter().map(|t| t.clone().data_type()).collect::<Vec<_>>(),
        [DataType::Timestamp { precision: 6, zoned: true }]
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3d i to vi]
/// DROP PERIOD RESTRICT is refused while a view, a CHECK, an assertion,
/// a routine or a trigger references the period.
#[test]
fn drop_period_restrict_is_refused_by_a_reference_11_28() {
    let cases: [(&str, &str); 5] = [
        ("CREATE VIEW V AS SELECT ROOM FROM BOOKINGS WHERE BOOKED CONTAINS DATE '2026-01-01'", "SR3d i"),
        ("ALTER TABLE BOOKINGS ADD CONSTRAINT IN_2026 CHECK (BOOKED PRECEDES PERIOD (DATE '2027-01-01', DATE '2027-01-02'))", "SR3d ii"),
        (
            "CREATE ASSERTION NONE_LATE CHECK (NOT EXISTS (SELECT * FROM BOOKINGS WHERE BOOKED SUCCEEDS PERIOD (DATE '2027-01-01', DATE '2027-01-02')))",
            "SR3d iii",
        ),
        (
            "CREATE FUNCTION BUSY (D DATE) RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM BOOKINGS WHERE BOOKED CONTAINS D)",
            "SR3d iv, SR3d v",
        ),
        (
            "CREATE TRIGGER G AFTER INSERT ON BOOKINGS REFERENCING NEW ROW AS N FOR EACH ROW \
          DELETE FROM ROOMS WHERE EXISTS (SELECT * FROM BOOKINGS WHERE BOOKED CONTAINS DATE '2030-01-01')",
            "SR3d vi",
        ),
    ];
    for (create, cited) in cases {
        let db = database();
        let s = db.session("A");
        ok(&s, &[create]);
        refused_by(&s, "ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED RESTRICT", &format!("§11.28 {cited}"));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 GR2f]
/// DROP PERIOD CASCADE drops what references the period, and keeps what
/// does not: a view, a CHECK, an assertion, a routine and a trigger
/// referencing it go; a view of its columns alone stays.
#[test]
fn drop_period_cascade_drops_its_references_11_28() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE VIEW V AS SELECT ROOM FROM BOOKINGS WHERE BOOKED CONTAINS DATE '2026-01-01'",
            "CREATE VIEW KEPT AS SELECT ROOM FROM BOOKINGS WHERE S <= DATE '2026-01-01'",
            "ALTER TABLE BOOKINGS ADD CONSTRAINT IN_2026 CHECK (BOOKED PRECEDES PERIOD (DATE '2027-01-01', DATE '2027-01-02'))",
            "CREATE ASSERTION NONE_LATE CHECK (NOT EXISTS (SELECT * FROM BOOKINGS WHERE BOOKED SUCCEEDS PERIOD (DATE '2027-01-01', DATE '2027-01-02')))",
            "CREATE FUNCTION BUSY (D DATE) RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM BOOKINGS WHERE BOOKED CONTAINS D)",
            "CREATE TRIGGER G AFTER INSERT ON BOOKINGS REFERENCING NEW ROW AS N FOR EACH ROW \
             DELETE FROM ROOMS WHERE EXISTS (SELECT * FROM BOOKINGS WHERE BOOKED CONTAINS DATE '2030-01-01')",
            "ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED CASCADE",
        ],
    );
    let count = |sql: &str| query(&s, sql).first().cloned().unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = 'A'"), "1");
    assert_eq!(query(&s, "SELECT ROOM FROM KEPT"), ["1"]);
    assert_eq!(count("SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE CONSTRAINT_NAME IN ('IN_2026', 'NONE_LATE')"), "0");
    assert_eq!(count("SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'"), "0");
    assert_eq!(count("SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_SCHEMA = 'A'"), "0");
    ok(&s, &["INSERT INTO BOOKINGS VALUES (9, DATE '2026-01-05', DATE '2026-01-01')"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.28 SR3d vii]
/// A generated column whose expression references the period keeps DROP
/// PERIOD from dropping it, under CASCADE too, as nothing drops the
/// column; once the column is dropped, the period is.
#[test]
fn a_generation_expression_keeps_its_period_11_28() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["ALTER TABLE BOOKINGS ADD COLUMN NEW_YEAR BOOLEAN GENERATED ALWAYS AS (BOOKED CONTAINS DATE '2026-01-01')"]);
    assert_eq!(query(&s, "SELECT ROOM, NEW_YEAR FROM BOOKINGS ORDER BY ROOM"), ["1,TRUE", "2,FALSE", "3,FALSE"]);
    for behavior in ["RESTRICT", "CASCADE"] {
        refused_by(&s, &format!("ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED {behavior}"), "§11.28 SR3d vii");
    }
    ok(&s, &["ALTER TABLE BOOKINGS DROP COLUMN NEW_YEAR RESTRICT", "ALTER TABLE BOOKINGS DROP PERIOD FOR BOOKED RESTRICT"]);
}

/// Verifies: [ISO/IEC 9075:1992 §8.11]
/// OVERLAPS of two rows of two values is still the <overlaps predicate>,
/// not a period predicate.
#[test]
fn overlaps_of_rows_is_unchanged() {
    let db = database();
    let s = db.session("A");
    assert_eq!(rooms(&s, "(S, E) OVERLAPS (DATE '2026-01-04', DATE '2026-01-06')"), ["1", "2"]);
}
