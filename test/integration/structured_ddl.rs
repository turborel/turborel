//! Structured types defined, altered and dropped [ISO/IEC 9075-2:2023
//! §11.51 to §11.60, §11.62; RULES.md D81]: CREATE TYPE with attributes,
//! a supertype, a reference representation and method specifications,
//! ALTER TYPE, CREATE METHOD, DROP of a method and DROP TYPE, each refused
//! by its rule with its SQLCODE and SQLSTATE. Casts, orderings and
//! transforms are tested in `structured_ddl_rules.rs`.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// A database of schema A, and a session of A.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    (db, a)
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The SQLCODE class, SQLSTATE and message of the refusal of `sql` in
/// `s`, rolled back.
fn refused(s: &Session, sql: &str) -> (Code, String, String) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    (e.code, e.sqlstate().as_str().to_owned(), e.to_string())
}

/// Asserts `sql` in `s` is refused with a Syntax Rule's SQLCODE and the
/// message naming `cited`.
fn refused_by(s: &Session, sql: &str, cited: &str) {
    let (code, _, msg) = refused(s, sql);
    assert_eq!(code, Code::SyntaxRule, "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR5, SR8d, GR1, GR3, §11.52 GR4]
/// CREATE TYPE of a <member list> makes a structured type, INSTANTIABLE by
/// default, a subtype inheriting its supertype's attributes first, and a
/// type of no attributes when no AS is written; its values are a column's.
#[test]
fn a_structured_type_is_defined_11_51() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TYPE POINT AS (X INTEGER, Y INTEGER DEFAULT 0) NOT FINAL",
            "CREATE TYPE POINT3 UNDER POINT AS (Z INTEGER) FINAL",
            "CREATE TYPE SHAPE NOT INSTANTIABLE NOT FINAL",
            "CREATE TABLE T (K INTEGER, P POINT)",
        ],
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR3, SR4, SR6, SR8a, SR8e]
/// CREATE TYPE of a structured type is refused: of a name a type has, an
/// option written twice, FINAL with NOT INSTANTIABLE, a cast option of a
/// distinct type, and no <finality>.
#[test]
fn a_definition_is_refused_by_its_options_11_51() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE POINT AS (X INTEGER) NOT FINAL"]);
    refused_by(&a, "CREATE TYPE POINT AS (Y INTEGER) NOT FINAL", "§11.51 SR3");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL NOT FINAL", "§11.51 SR4");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT INSTANTIABLE FINAL", "§11.51 SR6");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL CAST (SOURCE AS DISTINCT) WITH F", "§11.51 SR8a");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER)", "§11.51 SR8e");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR8f, SR8j i to iii, SR8j v,
/// §11.52 SR5] attributes are named apart, inherited ones too; a type is
/// no subtype of itself, nor of a type not there or FINAL; no attribute is
/// of the type itself, nor of its supertype.
#[test]
fn attributes_and_supertypes_are_checked_11_51() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE POINT AS (X INTEGER) NOT FINAL", "CREATE TYPE LEAF AS (X INTEGER) FINAL"]);
    refused_by(&a, "CREATE TYPE Q AS (X INTEGER, X INTEGER) NOT FINAL", "§11.51 SR8f");
    refused_by(&a, "CREATE TYPE Q UNDER POINT AS (X INTEGER) NOT FINAL", "§11.51 SR8j v");
    refused_by(&a, "CREATE TYPE Q UNDER Q AS (Y INTEGER) NOT FINAL", "§11.51 SR8j i");
    refused_by(&a, "CREATE TYPE Q UNDER NOWHERE AS (Y INTEGER) NOT FINAL", "§11.51 SR8j ii");
    refused_by(&a, "CREATE TYPE Q UNDER LEAF AS (Y INTEGER) NOT FINAL", "§11.51 SR8j iii");
    refused_by(&a, "CREATE TYPE Q AS (SELFISH Q) NOT FINAL", "§11.52 SR5");
    refused_by(&a, "CREATE TYPE Q UNDER POINT AS (P POINT) NOT FINAL", "§11.52 SR5");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 AR2, §12.3 SR4c] CREATE TYPE
/// UNDER a type of another schema needs UNDER on it (SQLCODE -104), which
/// its owner grants; UNDER is no privilege of a distinct type.
#[test]
fn under_is_needed_of_the_supertype_11_51() {
    let (db, a) = database();
    ok(&a, &["CREATE TYPE POINT AS (X INTEGER) NOT FINAL"]);
    let b = db.session("B");
    let (code, _, msg) = refused(&b, "CREATE TYPE B.Q UNDER A.POINT AS (Y INTEGER) NOT FINAL");
    assert_eq!(code, Code::Privilege, "{msg}");
    ok(&a, &["GRANT UNDER ON TYPE POINT TO B"]);
    ok(&b, &["CREATE TYPE B.Q UNDER A.POINT AS (Y INTEGER) NOT FINAL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR8b, SR8h, SR8i, SR8k] a
/// reference representation: REF USING an exact numeric type or a short
/// character string, REF FROM its attributes, REF IS SYSTEM GENERATED;
/// refused of a subtype, of another type, of a name no attribute's, and
/// casts to a reference type of no user-defined one.
#[test]
fn reference_representations_are_checked_11_51() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TYPE U AS (X INTEGER) NOT FINAL REF USING INTEGER",
            "CREATE TYPE D AS (X INTEGER) NOT FINAL REF FROM (X)",
            "CREATE TYPE S AS (X INTEGER) NOT FINAL REF IS SYSTEM GENERATED",
        ],
    );
    refused_by(&a, "CREATE TYPE Q UNDER U AS (Y INTEGER) NOT FINAL REF IS SYSTEM GENERATED", "§11.51 SR8b");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL REF USING DATE", "§11.51 SR8h");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL REF USING VARCHAR(2000)", "§11.51 SR8h");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL REF FROM (Z)", "§11.51 SR8i");
    refused_by(&a, "CREATE TYPE Q AS (Y INTEGER) NOT FINAL CAST (SOURCE AS REF) WITH F", "§11.51 SR8k");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR9b ii, SR9b iii, SR9b viii,
/// SR9b xi, SR9b xii, SR9b vii] method specifications are checked: STATIC
/// not SELF AS RESULT, a constructor named as its type, parameters named
/// apart and none SELF, SELF AS RESULT returning the type, no duplicate,
/// none of an attribute's observer's signature, specific names apart.
#[test]
fn method_specifications_are_checked_11_51() {
    let (_db, a) = database();
    let t = "CREATE TYPE Q AS (X INTEGER) NOT FINAL ";
    ok(&a, &[&format!("{t} METHOD M (N INTEGER) RETURNS INTEGER CONTAINS SQL, STATIC METHOD S () RETURNS INTEGER CONTAINS SQL")]);
    let r = "CREATE TYPE R AS (X INTEGER) NOT FINAL ";
    refused_by(&a, &format!("{r} STATIC METHOD M () RETURNS R SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b ii");
    refused_by(&a, &format!("{r} CONSTRUCTOR METHOD OTHER () RETURNS R SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b iii");
    refused_by(&a, &format!("{r} METHOD M (N INTEGER, N INTEGER) RETURNS INTEGER CONTAINS SQL"), "§11.51 SR9b viii");
    refused_by(&a, &format!("{r} METHOD M (SELF INTEGER) RETURNS INTEGER CONTAINS SQL"), "§11.51 SR9b viii");
    refused_by(&a, &format!("{r} METHOD M () RETURNS INTEGER SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b xi");
    refused_by(&a, &format!("{r} METHOD M () RETURNS INTEGER CONTAINS SQL, METHOD M () RETURNS DATE CONTAINS SQL"), "§11.51 SR9b xii");
    refused_by(&a, &format!("{r} METHOD X () RETURNS INTEGER CONTAINS SQL"), "§11.51 SR9b xii");
    refused_by(
        &a,
        &format!("{r} METHOD M () RETURNS INTEGER SPECIFIC Q_M CONTAINS SQL, METHOD N () RETURNS INTEGER SPECIFIC Q_M CONTAINS SQL"),
        "§11.51 SR9b vii",
    );
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR9b xiv 8, SR9b xv 3, SR9b xv 5,
/// SR9b xv 6, SR9b xv 7] a subtype redeclares an inherited method only
/// OVERRIDING; an overriding one overrides an original one of its name and
/// parameter types, no observer, names its parameters alike and returns
/// what it does.
#[test]
fn overriding_methods_are_checked_11_51() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE P AS (X INTEGER) NOT FINAL METHOD M (N INTEGER) RETURNS INTEGER CONTAINS SQL"]);
    ok(&a, &["CREATE TYPE C UNDER P AS (Y INTEGER) NOT FINAL OVERRIDING METHOD M (N INTEGER) RETURNS INTEGER"]);
    refused_by(&a, "CREATE TYPE D UNDER P AS (Y INTEGER) NOT FINAL METHOD M (N INTEGER) RETURNS INTEGER CONTAINS SQL", "§11.51 SR9b xiv 8");
    refused_by(&a, "CREATE TYPE D UNDER P AS (Y INTEGER) NOT FINAL OVERRIDING METHOD Z (N INTEGER) RETURNS INTEGER", "§11.51 SR9b xv 3");
    refused_by(&a, "CREATE TYPE D UNDER P AS (Y INTEGER) NOT FINAL OVERRIDING METHOD X () RETURNS INTEGER", "§11.51 SR9b xv 5");
    refused_by(&a, "CREATE TYPE D UNDER P AS (Y INTEGER) NOT FINAL OVERRIDING METHOD M (K INTEGER) RETURNS INTEGER", "§11.51 SR9b xv 6");
    refused_by(&a, "CREATE TYPE D UNDER P AS (Y INTEGER) NOT FINAL OVERRIDING METHOD M (N INTEGER) RETURNS DATE", "§11.51 SR9b xv 7");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 SR8b, SR20b, GR4] CREATE METHOD
/// gives a specification its body, once; it finds no specification of
/// another name or kind.
#[test]
fn a_method_gets_its_body_11_60() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TYPE P AS (X INTEGER) NOT FINAL METHOD M (N INTEGER) RETURNS INTEGER CONTAINS SQL",
            "CREATE METHOD M (N INTEGER) RETURNS INTEGER FOR P RETURN N",
        ],
    );
    refused_by(&a, "CREATE METHOD M (N INTEGER) RETURNS INTEGER FOR P RETURN N", "§11.60 SR20b");
    refused_by(&a, "CREATE METHOD Z (N INTEGER) RETURNS INTEGER FOR P RETURN N", "§11.60 SR8b iii");
    refused_by(&a, "CREATE STATIC METHOD M (N INTEGER) RETURNS INTEGER FOR P RETURN N", "§11.60 SR8b iii");
    refused_by(&a, "CREATE METHOD M (K INTEGER) RETURNS INTEGER FOR P RETURN K", "§11.60 SR8b vi");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.54, §11.55, RULES.md D81] ALTER TYPE
/// ADD and DROP ATTRIBUTE: refused while a column is of the type; DROP of
/// an inherited attribute, of the only one, or of one not there.
#[test]
fn attributes_are_added_and_dropped_11_54() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE P AS (X INTEGER) NOT FINAL", "CREATE TYPE C UNDER P AS (Y INTEGER) NOT FINAL"]);
    ok(&a, &["ALTER TYPE P ADD ATTRIBUTE Z INTEGER", "ALTER TYPE P DROP ATTRIBUTE Z RESTRICT"]);
    refused_by(&a, "ALTER TYPE P DROP ATTRIBUTE X RESTRICT", "§11.55 SR3");
    refused_by(&a, "ALTER TYPE C DROP ATTRIBUTE X RESTRICT", "§11.55 SR3");
    refused_by(&a, "ALTER TYPE P ADD ATTRIBUTE Y INTEGER", "§11.51 SR8j v");
    ok(&a, &["CREATE TABLE T (K C)"]);
    refused_by(&a, "ALTER TYPE P ADD ATTRIBUTE W INTEGER", "§11.54 SR4");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.56, §11.58] ALTER TYPE ADD and DROP
/// of a method specification; DROP refused while its body exists.
#[test]
fn methods_are_added_and_dropped_11_56() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE P AS (X INTEGER) NOT FINAL", "ALTER TYPE P ADD METHOD M () RETURNS INTEGER"]);
    ok(&a, &["CREATE METHOD M () RETURNS INTEGER FOR P RETURN 1"]);
    refused_by(&a, "ALTER TYPE P DROP METHOD M () RESTRICT", "§11.58 SR4");
    ok(&a, &["DROP METHOD M () FOR P RESTRICT", "ALTER TYPE P DROP METHOD M () RESTRICT"]);
    refused_by(&a, "ALTER TYPE P DROP METHOD M () RESTRICT", "§11.58 SR3c");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.59 SR4b, SR4c, GR9] DROP TYPE of a
/// structured type with RESTRICT is refused while a subtype or a column is
/// of it; with CASCADE the subtype goes too.
#[test]
fn a_structured_type_is_dropped_11_59() {
    let (_db, a) = database();
    ok(&a, &["CREATE TYPE P AS (X INTEGER) NOT FINAL", "CREATE TYPE C UNDER P AS (Y INTEGER) NOT FINAL"]);
    refused_by(&a, "DROP TYPE P RESTRICT", "§11.59 SR4c");
    ok(&a, &["DROP TYPE P CASCADE"]);
    ok(&a, &["CREATE TYPE P AS (X INTEGER) NOT FINAL", "CREATE TABLE T (K P)"]);
    refused_by(&a, "DROP TYPE P RESTRICT", "§11.59 SR4");
}
