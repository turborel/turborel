//! Unit tests of the Definition Schema: every table's rows keep the
//! contract of a system table, and those of the tables no view of the
//! Information Schema reads are as §21.3 describes them (RULES.md
//! R-TEST-1).

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

/// Verifies: [DESIGN] RULES.md D37, `SystemTable`: each table of the
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

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.3.3; ISO/IEC 9075-2:2023
/// §4.42.3]: USERS has each role, and each grantor and grantee of a role
/// authorization, as authorization identifiers known.
#[test]
fn users_has_roles_and_their_grantees() {
    let db = database();
    db.create_user("O", b"x").unwrap();
    let o = db.session("O");
    o.run("CREATE ROLE RR").unwrap();
    o.run("GRANT RR TO Q").unwrap();
    o.commit().unwrap();
    let users = rows_of(&db, "USERS");
    assert!(["O", "Q", "RR"].iter().all(|u| users.iter().any(|x| x == u)), "{users:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §6.34 Description 1
/// to 4; ISO/IEC 9075-2:2023 §12.4 GR2]: ROLE_AUTHORIZATION_DESCRIPTORS has
/// a row of each role authorization: the one a role's creator holds of
/// "_SYSTEM", grantable, and each grant, PUBLIC the grantee of one granted
/// to all, grantable WITH ADMIN OPTION alone.
#[test]
fn role_authorizations_are_described() {
    let db = database();
    db.create_user("O", b"x").unwrap();
    let o = db.session("O");
    for sql in ["CREATE ROLE RR", "GRANT RR TO PUBLIC", "GRANT RR TO Q WITH ADMIN OPTION"] {
        o.run(sql).unwrap();
    }
    o.commit().unwrap();
    let mut described = rows_of(&db, "ROLE_AUTHORIZATION_DESCRIPTORS");
    described.sort();
    assert_eq!(described, ["RR|O|_SYSTEM|YES", "RR|PUBLIC|O|NO", "RR|Q|O|YES"]);
}
