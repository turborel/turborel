//! \[DESIGN\] What a statement's end checks, found once for the catalog's
//! tables (RULES.md D415): no table's foreign keys or reading CHECKs
//! looked for where none has one, and each row read once for all its
//! checks.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::exec;

/// Verifies: [ISO/IEC 9075:1992 §10.6 GR3, §11.8, §11.6 GR3]; [DESIGN]
/// RULES.md D415: statements ended where no table had a foreign key or a
/// CHECK that reads tables, a FOREIGN KEY then added by ALTER TABLE, a
/// table made with one, and a CHECK reading another table added by ALTER
/// TABLE are each checked at the next statement's end.
#[test]
fn constraints_added_after_statements_ended_without_any_are_checked() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION E
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (A INTEGER)
           CREATE TABLE Q (K INTEGER)",
    )
    .unwrap();
    let s = db.session("E");
    s.run("INSERT INTO P VALUES (1), (2), (3), (4)").unwrap();
    s.run("INSERT INTO C VALUES (1)").unwrap();
    s.run("UPDATE P SET K = K + 10 WHERE K = 4").unwrap();
    s.run("DELETE FROM P WHERE K = 14").unwrap();
    s.commit().unwrap();
    s.run("ALTER TABLE C ADD CONSTRAINT C_P FOREIGN KEY (A) REFERENCES P").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE K = 1"), Err(Code::Constraint), "the foreign key added holds");
    assert_eq!(exec(&s, "UPDATE P SET K = 9 WHERE K = 1"), Err(Code::Constraint));
    s.run("CREATE TABLE D (B INTEGER REFERENCES P)").unwrap();
    s.run("INSERT INTO D VALUES (2)").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE K = 2"), Err(Code::Constraint), "a table made with one");
    s.run("INSERT INTO Q VALUES (3)").unwrap();
    s.run("ALTER TABLE Q ADD CONSTRAINT Q_P CHECK (K IN (SELECT K FROM P))").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE K = 3"), Err(Code::Constraint), "the CHECK added holds");
}

/// Verifies: [X3.135-1986 §6.3 GR1]; [ISO/IEC 9075:1992 §11.6 GR3];
/// [DESIGN] RULES.md D415: of rows inserted together, each row's NOT NULL
/// and CHECK constraints are checked of that row, read once for them all:
/// a null or a false CHECK in a later row refuses the statement.
#[test]
fn each_row_is_checked_of_its_own_values() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE N (A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER CHECK (C > 0))").unwrap();
    let s = db.session("E");
    assert_eq!(exec(&s, "INSERT INTO N VALUES (1, 1, 1), (2, NULL, 1)"), Err(Code::Constraint), "a null in the second row");
    assert_eq!(exec(&s, "INSERT INTO N VALUES (1, 1, 1), (2, 2, -1)"), Err(Code::Constraint), "a false CHECK in the second");
    assert!(exec(&s, "INSERT INTO N VALUES (1, 1, 1), (2, 2, 2)").is_ok());
    assert_eq!(exec(&s, "UPDATE N SET B = NULL WHERE A = 2"), Err(Code::Constraint), "an update of one");
}
