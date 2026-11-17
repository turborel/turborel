//! Methods of distinct types [ISO/IEC 9075-2:2023 §11.51 SR9, CR12,
//! §11.53 SR4, §11.56, §11.58, §11.60, §6.18, §6.19, §9.19 SR3]: a
//! distinct type is a user-defined type, so it may have original method
//! specifications, instance and static, given bodies by CREATE METHOD,
//! invoked of its values, added and dropped by ALTER TYPE.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: D of INTEGER with the instance method TWICE and the static
/// method ZERO, each with a body; table T of one value D(4).
fn typed() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TYPE D AS INTEGER FINAL METHOD TWICE () RETURNS INTEGER CONTAINS SQL, STATIC METHOD ZERO () RETURNS D CONTAINS SQL",
            "CREATE METHOD TWICE () RETURNS INTEGER FOR D RETURN CAST(SELF AS INTEGER) * 2",
            "CREATE STATIC METHOD ZERO () RETURNS D FOR D RETURN D(0)",
            "CREATE TABLE T (X D)",
            "INSERT INTO T VALUES (D(4))",
        ],
    );
    (db, a)
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Asserts `sql` in `s`, read by a cursor if a query, is refused with
/// SQLCODE -103 citing `cited`; rolls back.
fn refused(s: &Session, sql: &str, cited: &str) {
    let e = if sql.starts_with("SELECT") { s.open_cursor(sql).err() } else { s.run(sql).err() };
    let e = e.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!(e.code, Code::SyntaxRule, "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR9, §11.60 SR8, §6.18, §6.19,
/// §9.19 SR3, §6.6 SR6] A distinct type's instance method is invoked of a
/// value of it, written as a `<value expression primary>`, and its static
/// method of the type; a column of it is not refinable, so `X.TWICE()`
/// is a routine invocation of a schema X.
#[test]
fn methods_of_a_distinct_type_are_invoked_6_18() {
    let (_db, a) = typed();
    assert_eq!(query(&a, "SELECT (X).TWICE(), D::ZERO() FROM T"), ["8,0"]);
    refused(&a, "SELECT X.TWICE() FROM T", "§9.18 SR10b");
    refused(&a, "SELECT (X).TWICE(1) FROM T", "§9.18 SR10");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR9b iv, SR9b xv 1, SR9b xii]
/// A distinct type has no constructor method, its methods are original,
/// and two of one name and parameters are refused.
#[test]
fn a_distinct_type_has_original_methods_alone_11_51() {
    let (_db, a) = typed();
    refused(&a, "CREATE TYPE E AS INTEGER FINAL CONSTRUCTOR METHOD E () RETURNS E SELF AS RESULT CONTAINS SQL", "§11.51 SR9b iv");
    refused(&a, "CREATE TYPE E AS INTEGER FINAL OVERRIDING METHOD M () RETURNS INTEGER", "§11.51 SR9b xv 1");
    refused(
        &a,
        "CREATE TYPE E AS INTEGER FINAL METHOD M () RETURNS INTEGER CONTAINS SQL, METHOD M () RETURNS INTEGER CONTAINS SQL",
        "§11.51 SR9b xii",
    );
    assert_eq!(query(&a, "SELECT 1 FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE USER_DEFINED_TYPE_NAME = 'E'"), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.53 SR4, §11.56, §11.58 SR4] ALTER
/// TYPE of a distinct type adds an original method specification and drops
/// one of no body; an attribute, or an overriding method, is of a
/// structured type alone.
#[test]
fn methods_are_added_to_a_distinct_type_and_dropped_11_56() {
    let (_db, a) = typed();
    ok(
        &a,
        &[
            "ALTER TYPE D ADD METHOD THRICE () RETURNS INTEGER CONTAINS SQL",
            "CREATE METHOD THRICE () RETURNS INTEGER FOR D RETURN CAST(SELF AS INTEGER) * 3",
        ],
    );
    assert_eq!(query(&a, "SELECT (X).THRICE() FROM T"), ["12"]);
    refused(&a, "ALTER TYPE D DROP METHOD THRICE () RESTRICT", "§11.58 SR4");
    ok(&a, &["DROP METHOD THRICE () FOR D RESTRICT", "ALTER TYPE D DROP METHOD THRICE () RESTRICT"]);
    refused(&a, "SELECT (X).THRICE() FROM T", "§9.18 SR10");
    refused(&a, "ALTER TYPE D ADD ATTRIBUTE Z INTEGER", "§11.53 SR4");
    refused(&a, "ALTER TYPE D ADD OVERRIDING METHOD TWICE () RETURNS INTEGER", "§11.53 SR4");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.59 SR4g, GR1, ISO/IEC 9075-11
/// METHOD_SPECIFICATIONS, ROUTINES] A distinct type's methods are
/// described as a structured type's; a method created by CREATE METHOD is
/// no routine dependent on the type, so DROP TYPE RESTRICT is refused while
/// it is there and CASCADE drops it.
#[test]
fn methods_of_a_distinct_type_are_described_and_dropped_11_59() {
    let (_db, a) = typed();
    assert_eq!(
        query(&a, "SELECT METHOD_NAME, IS_STATIC FROM INFORMATION_SCHEMA.METHOD_SPECIFICATIONS WHERE UDT_NAME = 'D' ORDER BY 1"),
        ["TWICE,NO", "ZERO,YES"]
    );
    let routines = "SELECT ROUTINE_NAME, ROUTINE_TYPE FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' AND ROUTINE_TYPE <> 'FUNCTION' ORDER BY 1";
    assert_eq!(query(&a, routines), ["TWICE,INSTANCE METHOD", "ZERO,STATIC METHOD"]);
    ok(&a, &["DROP TABLE T CASCADE"]);
    refused(&a, "DROP TYPE D RESTRICT", "§11.59 SR4g");
    ok(&a, &["DROP TYPE D CASCADE"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'"), ["0"]);
}
