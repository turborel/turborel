//! User-defined casts, orderings and transforms, and the session's
//! transform groups [ISO/IEC 9075-2:2023 §11.63 to §11.71, §19.13]: each
//! defined, dropped, and refused by its rule with its SQLCODE and
//! SQLSTATE.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A, holding structured type PT of one attribute and its functions
/// PT_INT, PT_MAP, PT_CMP and PT_ANY (not deterministic), and
/// distinct type MONEY; schema B, empty; a session of A.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE TYPE PT AS (X INTEGER) NOT FINAL",
        "CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL",
        "CREATE FUNCTION PT_INT (P PT) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 1",
        "CREATE FUNCTION PT_MAP (P PT) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 1",
        "CREATE FUNCTION PT_CMP (P PT, Q PT) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 0",
        "CREATE FUNCTION PT_ANY (P PT) RETURNS INTEGER NOT DETERMINISTIC CONTAINS SQL RETURN 1",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.commit().unwrap();
    (db, a)
}

/// Runs `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &str) {
    s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    s.commit().unwrap();
}

/// Asserts `sql` in `s` is refused with SQLCODE class `code` and a
/// message naming `cited`, and rolls back.
fn refused_with(s: &Session, sql: &str, (code, cited): (Code, &str)) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    assert_eq!(e.code, code, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// Asserts `sql` in `s` is refused by the Syntax Rule `cited` (-103).
fn refused_by(s: &Session, sql: &str, cited: &str) {
    refused_with(s, sql, (Code::SyntaxRule, cited));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.63 SR3, SR4, SR10a, SR10b, SR10d,
/// GR1, §11.64 SR2] CREATE CAST of a structured type with its function;
/// refused twice, of no user-defined type, of a function of other
/// parameters, returning another type or not deterministic; DROP CAST
/// of none is refused.
#[test]
fn casts_are_created_and_dropped_11_63() {
    let (_db, a) = database();
    refused_by(&a, "CREATE CAST (PT AS INTEGER) WITH FUNCTION PT_ANY (PT)", "§11.63 SR10d");
    ok(&a, "CREATE CAST (PT AS INTEGER) WITH FUNCTION PT_INT (PT)");
    refused_by(&a, "CREATE CAST (PT AS INTEGER) WITH FUNCTION PT_INT (PT)", "§11.63 SR3");
    refused_by(&a, "CREATE CAST (DATE AS INTEGER) WITH FUNCTION PT_INT (PT)", "§11.63 SR4");
    refused_by(&a, "CREATE CAST (PT AS DATE) WITH FUNCTION PT_INT (PT)", "§11.63 SR10b");
    refused_by(&a, "CREATE CAST (INTEGER AS PT) WITH FUNCTION PT_INT (PT)", "§11.63 SR10a");
    ok(&a, "DROP CAST (PT AS INTEGER) RESTRICT");
    ok(&a, "CREATE CAST (PT AS INTEGER) WITH FUNCTION PT_MAP (PT) AS ASSIGNMENT");
    ok(&a, "DROP CAST (PT AS INTEGER) RESTRICT");
    refused_by(&a, "DROP CAST (PT AS INTEGER) RESTRICT", "§11.64 SR2");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.63 AR2] CREATE CAST of a type of a
/// schema another owns is refused with SQLCODE -104.
#[test]
fn a_cast_is_of_its_types_owner_11_63() {
    let (db, _a) = database();
    let b = db.session("B");
    refused_with(&b, "CREATE CAST (A.PT AS INTEGER) WITH FUNCTION A.PT_INT (A.PT)", (Code::Privilege, "§11.63 AR2"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.65 SR2, SR6a ii, SR6b, SR7, SR8,
/// §11.66 SR2] CREATE ORDERING by MAP, by RELATIVE and by STATE; refused
/// of a type ordered already, STATE ORDER FULL, a function not
/// deterministic, RELATIVE of one parameter, MAP of two; DROP ORDERING of
/// none is refused.
#[test]
fn orderings_are_created_and_dropped_11_65() {
    let (_db, a) = database();
    ok(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PT_MAP (PT)");
    refused_by(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PT_MAP (PT)", "§11.65 SR2");
    ok(&a, "DROP ORDERING FOR PT RESTRICT");
    refused_by(&a, "DROP ORDERING FOR PT RESTRICT", "§11.66 SR2");
    refused_by(&a, "CREATE ORDERING FOR PT ORDER FULL BY STATE", "§11.65 SR6a ii");
    refused_by(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PT_ANY (PT)", "§11.65 SR6b");
    refused_by(&a, "CREATE ORDERING FOR PT ORDER FULL BY RELATIVE WITH FUNCTION PT_MAP (PT)", "§11.65 SR7");
    refused_by(&a, "CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PT_CMP (PT, PT)", "§11.65 SR8");
    ok(&a, "CREATE ORDERING FOR PT ORDER FULL BY RELATIVE WITH FUNCTION PT_CMP (PT, PT)");
    ok(&a, "DROP ORDERING FOR PT RESTRICT");
    ok(&a, "CREATE ORDERING FOR PT EQUALS ONLY BY STATE");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 GR2, §11.65 SR2] a distinct
/// type is ordered FULL by MAP when it is defined, so CREATE ORDERING of it
/// is refused until DROP ORDERING.
#[test]
fn a_distinct_type_is_ordered_when_defined_11_51() {
    let (_db, a) = database();
    refused_by(&a, "CREATE ORDERING FOR MONEY EQUALS ONLY BY STATE", "§11.65 SR2");
    ok(&a, "DROP ORDERING FOR MONEY RESTRICT");
    refused_by(&a, "CREATE ORDERING FOR MONEY EQUALS ONLY BY STATE", "§11.65 SR6a i");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.67 SR2, SR4f, SR4g, §11.68 SR3b,
/// §11.70 SR3, §11.71 SR2] CREATE TRANSFORM of a group of a from-sql
/// function; refused of a group the type has, a to-sql function not
/// returning the type; ALTER TRANSFORM of a group not there, DROP of a
/// function the group has not; DROP TRANSFORM of a group not there.
#[test]
fn transforms_are_created_altered_and_dropped_11_67() {
    let (_db, a) = database();
    ok(&a, "CREATE TRANSFORM FOR PT G1 (FROM SQL WITH FUNCTION PT_INT (PT))");
    refused_by(&a, "CREATE TRANSFORM FOR PT G1 (FROM SQL WITH FUNCTION PT_MAP (PT))", "§11.67 SR2");
    refused_by(&a, "CREATE TRANSFORM FOR PT G2 (TO SQL WITH FUNCTION PT_INT (PT))", "§11.67 SR4f");
    refused_by(&a, "CREATE TRANSFORM FOR PT G2 (FROM SQL WITH FUNCTION PT_CMP (PT, PT))", "§11.67 SR4g");
    refused_by(&a, "ALTER TRANSFORM FOR PT G9 (DROP (FROM SQL RESTRICT))", "§11.68 SR3b");
    refused_by(&a, "ALTER TRANSFORM FOR PT G1 (DROP (TO SQL RESTRICT))", "§11.70 SR3");
    ok(&a, "ALTER TRANSFORM FOR PT G1 (DROP (FROM SQL RESTRICT))");
    refused_by(&a, "DROP TRANSFORM G9 FOR PT RESTRICT", "§11.71 SR2");
    ok(&a, "DROP TRANSFORM ALL FOR PT RESTRICT");
}

/// Verifies: [ISO/IEC 9075-2:2023 §19.13 GR1, SR1] SET TRANSFORM GROUP of
/// a group name, for a type or by default; of no <group name> refused with
/// SQLSTATE 0S000, for a type not there with SQLCODE -103.
#[test]
fn transform_groups_are_set_19_13() {
    let (_db, a) = database();
    a.run("SET DEFAULT TRANSFORM GROUP 'G1'").unwrap();
    a.run("SET TRANSFORM GROUP FOR TYPE PT ' G2 '").unwrap();
    let e = a.run("SET DEFAULT TRANSFORM GROUP 'TWO WORDS'").err().unwrap();
    assert_eq!(e.sqlstate().as_str(), "0S000", "{e}");
    refused_by(&a, "SET TRANSFORM GROUP FOR TYPE NOWHERE G1", "§19.13 SR1");
}
