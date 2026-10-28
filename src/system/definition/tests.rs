//! Unit tests of the Definition Schema: every table's rows keep the
//! contract of a system table, and those of the tables no view of the
//! Information Schema reads are as §21.3 describes them (RULES.md
//! R-TEST-1). Split from definition.rs (RULES.md R-SIZE-4).

use super::*;
use crate::db::Database;
use crate::system::SystemRead;

/// A schema with a key, a referential constraint, a CHECK reading another
/// table, and a privilege granted.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE P (K INTEGER PRIMARY KEY)
           CREATE TABLE C (A INTEGER REFERENCES P, CONSTRAINT IN_P CHECK (A IN (SELECT K FROM P)))
           GRANT SELECT ON C TO R",
    )
    .unwrap();
    db
}

/// The rows of the Definition Schema's table `name`, as user `S` reads it,
/// each value with its trailing spaces cut.
fn rows_of(db: &Database, name: &str) -> Vec<String> {
    let st = db.snapshot().unwrap();
    let table = tables().into_iter().find(|t| t.name() == name).unwrap();
    let read = SystemRead { table, user: "S".into(), own: crate::settings::Own::default() };
    let shown = |r: Row| r.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join("|");
    read.rows(&st).unwrap().into_iter().map(shown).collect()
}

/// Verifies: [DESIGN] RULES.md D30, `SystemTable`: each table of the
/// Definition Schema gives rows of its columns' degree and types, none null
/// in a NOT NULL column (`SystemRead::rows` checks).
#[test]
fn every_table_keeps_the_contract() {
    let db = database();
    for table in tables() {
        assert_eq!(table.schema(), DEFINITION_SCHEMA);
        rows_of(&db, table.name());
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.3.3, §21.3.17]: USERS has
/// each `<authorization identifier>` known, of schemas, grantors and
/// grantees, "_SYSTEM" and PUBLIC among them; CHECK_TABLE_USAGE each table
/// a CHECK references, its own and those its subqueries read, and the
/// table of a NOT NULL constraint.
#[test]
fn users_and_check_tables_are_described() {
    let db = database();
    assert_eq!(rows_of(&db, "USERS"), ["DEFINITION_SCHEMA", "INFORMATION_SCHEMA", "PUBLIC", "R", "S", "_SYSTEM"]);
    // In the order the CHECK's references are bound: its IN's subquery's
    // first [ISO/IEC 9075:1992 §17.6 GR5q].
    assert_eq!(rows_of(&db, "CHECK_TABLE_USAGE"), ["TURBOREL|S|IN_P|TURBOREL|S|P", "TURBOREL|S|IN_P|TURBOREL|S|C"]);
}
