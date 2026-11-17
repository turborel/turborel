//! Privileges on a structured type and on what is defined of it [ISO/IEC
//! 9075-2:2023 §6.1, §11.60, §11.64, §11.67 to §11.71, §12.2, §12.3,
//! §12.7]. USAGE and UNDER are granted and revoked, RESTRICT refusing what
//! a revoke abandons (2B000) and CASCADE dropping it. A reference type
//! needs USAGE on its type. A table/method privilege names a method of the
//! table's type its grantor executes. Transforms are checked by their
//! rules, and a method or cast is defined or dropped by its schema's owner.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schemas A, B and C. Schema A holds structured type PT of attribute X,
/// with the type-preserving method FROMINT and the method GREET, its
/// subtype SUBA, type OTHER of the method HELLO, distinct type MONEY,
/// typed tables PTS and SUBS, and table PLAIN of one row.
fn database() -> Database {
    let db = Database::new();
    for auth in ["A", "B", "C"] {
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION {auth}")).unwrap();
    }
    ok(
        &db.session("A"),
        &[
            "CREATE TYPE PT AS (X INTEGER) NOT FINAL REF IS SYSTEM GENERATED \
             METHOD FROMINT (V INTEGER) RETURNS PT SELF AS RESULT DETERMINISTIC CONTAINS SQL, \
             METHOD GREET () RETURNS INTEGER CONTAINS SQL",
            "CREATE METHOD GREET () RETURNS INTEGER FOR PT RETURN 1",
            "CREATE TYPE SUBA UNDER PT AS (Y INTEGER) NOT FINAL",
            "CREATE TYPE OTHER AS (Z INTEGER) NOT FINAL METHOD HELLO () RETURNS INTEGER CONTAINS SQL",
            "CREATE METHOD HELLO () RETURNS INTEGER FOR OTHER RETURN 2",
            "CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL",
            "CREATE TABLE PTS OF PT (REF IS ID SYSTEM GENERATED)",
            "CREATE TABLE SUBS OF SUBA UNDER PTS",
            "CREATE TABLE PLAIN (K INTEGER)",
            "INSERT INTO PLAIN VALUES (1)",
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

/// The SQLSTATE `sql` completes with in `s`, committed.
fn completed(s: &Session, sql: &str) -> String {
    let state = s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}")).sqlstate().as_str().to_owned();
    s.commit().unwrap();
    state
}

/// The rows of `sql` in `s`, which must succeed.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Asserts `sql` in `s` is refused with SQLCODE class `code`, SQLSTATE
/// `state` and a message naming each of `cited`; rolls back.
fn refused(s: &Session, sql: &str, (code, state): (Code, &str), cited: &[&str]) {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    for c in cited {
        assert!(msg.contains(c), "{sql}: {msg} does not cite {c}");
    }
}

/// The privileges on PT `s` sees but its owner's own, as grantor,
/// grantee, privilege and grantability.
fn udt_privileges(s: &Session) -> Vec<String> {
    query(
        s,
        "SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.UDT_PRIVILEGES \
         WHERE UDT_NAME = 'PT' AND GRANTOR <> '_SYSTEM' ORDER BY 1, 2, 3",
    )
}

/// The SQLCODE class and SQLSTATE of a Syntax Rule broken.
const SR: (Code, &str) = (Code::SyntaxRule, "2A000");
/// The SQLCODE class and SQLSTATE of a privilege not held.
const HELD: (Code, &str) = (Code::Privilege, "2A000");
/// The SQLCODE class and SQLSTATE of a revoke that abandons what depends
/// on it.
const DEPENDENT: (Code, &str) = (Code::SyntaxRule, "2B000");

/// Verifies: [ISO/IEC 9075-2:2023 §12.3 SR4a, SR4c, §12.2 AR1, GR9] Of a
/// structured type, USAGE and UNDER alone are granted or revoked (-103).
/// One who holds no privilege named grants none (-104). One who holds a
/// privilege without the grant option grants it with a warning (01007),
/// as ALL PRIVILEGES grants those held grantably alone. Granted again WITH
/// GRANT OPTION, a grant becomes grantable.
#[test]
fn usage_and_under_are_granted_on_a_structured_type_12_2() {
    let db = database();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    refused(&a, "GRANT SELECT ON TYPE PT TO B", SR, &["§12.3 SR4a, SR4c"]);
    refused(&a, "REVOKE SELECT ON TYPE PT FROM B RESTRICT", SR, &["§12.3 SR4a, SR4c"]);
    refused(&c, "GRANT USAGE ON TYPE A.PT TO B", HELD, &["§12.2 AR1", "C holds no privilege"]);
    ok(&a, &["GRANT USAGE ON TYPE PT TO B", "GRANT UNDER ON TYPE PT TO B"]);
    assert_eq!(udt_privileges(&a), ["A,B,TYPE USAGE,NO", "A,B,UNDER,NO"]);
    assert_eq!(completed(&b, "GRANT UNDER ON TYPE A.PT TO C"), "01007");
    ok(&a, &["GRANT USAGE ON TYPE PT TO B WITH GRANT OPTION"]);
    assert_eq!(udt_privileges(&a), ["A,B,TYPE USAGE,YES", "A,B,UNDER,NO"]);
    assert_eq!(completed(&b, "GRANT ALL PRIVILEGES ON TYPE A.PT TO C"), "01007");
    assert_eq!(udt_privileges(&c), ["B,C,TYPE USAGE,NO"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR4, GR12h ii, GR12q, GR12z] A
/// revoke of USAGE that leaves a column of the type in a schema whose
/// owner holds no USAGE is refused with RESTRICT (-103, 2B000). GRANT
/// OPTION FOR takes the grant option alone. A grantee holding no grant of
/// the revoker's is warned (01006). CASCADE drops the column and keeps
/// its table's rows.
#[test]
fn revoke_of_usage_on_a_type_drops_its_columns_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B WITH GRANT OPTION"]);
    ok(&b, &["CREATE TABLE U (K INTEGER, P A.PT)", "INSERT INTO U (K) VALUES (7)"]);
    refused(&a, "REVOKE USAGE ON TYPE PT FROM B RESTRICT", DEPENDENT, &["§12.7 GR12q", "abandons column B.U.P"]);
    assert_eq!(completed(&a, "REVOKE GRANT OPTION FOR USAGE ON TYPE PT FROM B RESTRICT"), "00000");
    assert_eq!(udt_privileges(&a), ["A,B,TYPE USAGE,NO"]);
    assert_eq!(completed(&a, "REVOKE USAGE ON TYPE PT FROM C CASCADE"), "01006");
    assert_eq!(completed(&a, "REVOKE USAGE ON TYPE PT FROM B CASCADE"), "00000");
    assert_eq!(udt_privileges(&a), Vec::<String>::new());
    assert_eq!(query(&b, "SELECT * FROM U"), ["7"], "column P gone, its row kept");
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR12q, GR12ag, §11.51 AR2] A
/// revoke of UNDER that leaves a subtype in a schema whose owner holds no
/// UNDER is refused with RESTRICT (-103, 2B000), naming the subtype. With
/// CASCADE the subtype is dropped as DROP TYPE CASCADE drops it, and its
/// owner defines no other.
#[test]
fn revoke_of_under_on_a_type_drops_its_subtypes_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT UNDER ON TYPE PT TO B"]);
    ok(&b, &["CREATE TYPE SUB UNDER A.PT AS (Y INTEGER) NOT FINAL"]);
    let own = "SELECT USER_DEFINED_TYPE_NAME FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE USER_DEFINED_TYPE_SCHEMA = 'B'";
    assert_eq!(query(&b, own), ["SUB"]);
    refused(&a, "REVOKE UNDER ON TYPE PT FROM B RESTRICT", DEPENDENT, &["§12.7 GR12q", "abandons type B.SUB"]);
    assert_eq!(completed(&a, "REVOKE UNDER ON TYPE PT FROM B CASCADE"), "00000");
    assert_eq!(query(&b, own), Vec::<String>::new());
    refused(&b, "CREATE TYPE SUB UNDER A.PT AS (Y INTEGER) NOT FINAL", HELD, &["§11.51 AR2"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR12, GR12q] A revoke of USAGE
/// that abandons a view whose query names the type is refused with
/// RESTRICT (-103, 2B000), naming the view. With CASCADE the view is
/// dropped.
#[test]
fn revoke_of_usage_on_a_type_drops_a_view_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B", "GRANT SELECT ON PLAIN TO B"]);
    ok(&b, &["CREATE VIEW W AS SELECT K FROM A.PLAIN WHERE CAST(NULL AS A.PT) IS NULL"]);
    assert_eq!(query(&b, "SELECT K FROM W"), ["1"]);
    let own = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'B'";
    assert_eq!(query(&b, own), ["W"]);
    refused(&a, "REVOKE USAGE ON TYPE PT FROM B RESTRICT", DEPENDENT, &["§12.7 GR12q", "abandons view B.W"]);
    assert_eq!(completed(&a, "REVOKE USAGE ON TYPE PT FROM B CASCADE"), "00000");
    assert_eq!(query(&b, own), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1 SR44, SR45, AR1] REF of a type
/// that is no structured type is refused (-103), as is a scope that is no
/// typed table of the referenced type. One who holds no USAGE on the type
/// is refused (-104). Once granted it, the column is defined.
#[test]
fn a_reference_type_needs_usage_on_its_structured_type_6_1() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    refused(&b, "CREATE TABLE R (R REF(A.PT))", HELD, &["§6.1 AR1", "B holds no USAGE on type A.PT"]);
    ok(&a, &["GRANT USAGE ON TYPE PT TO B"]);
    refused(&b, "CREATE TABLE R (R REF(A.MONEY))", SR, &["§6.1 SR44"]);
    refused(&b, "CREATE TABLE R (R REF(A.PT) SCOPE A.PLAIN)", SR, &["§6.1 SR45", "A.PLAIN"]);
    ok(&b, &["CREATE TABLE R (K INTEGER, R REF(A.PT) SCOPE A.PTS)"]);
    assert_eq!(query(&b, "SELECT COUNT(*) FROM R"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.3 SR6, §12.2 AR3, §11.57 GR3]
/// SELECT of a `<privilege method list>` on a table of no structured
/// type, or of a method of another type, is refused (-103). An observer
/// is named by its specific name. A grantor holding no EXECUTE on the
/// method is refused (-104). A method overriding one of a supertype takes
/// over the table/method privileges of the subtype's tables.
#[test]
fn table_method_privileges_name_methods_of_the_tables_type_12_3() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    refused(&a, "GRANT SELECT (METHOD GREET () FOR PT) ON PLAIN TO B", SR, &["§12.3 SR6", "no table of a structured type"]);
    refused(&a, "GRANT SELECT (METHOD HELLO () FOR OTHER) ON PTS TO B", SR, &["§12.3 SR6", "is no method of the table's"]);
    ok(
        &a,
        &[
            "GRANT SELECT (SPECIFIC METHOD PT_X_OBSERVER) ON PTS TO C",
            "GRANT SELECT (METHOD GREET () FOR PT) ON SUBS TO C",
            "GRANT SELECT ON PTS TO B WITH GRANT OPTION",
        ],
    );
    refused(&b, "GRANT SELECT (SPECIFIC METHOD A.PT_X_OBSERVER) ON A.PTS TO C", HELD, &["§12.2 AR3", "B holds no EXECUTE"]);
    let shown = "SELECT TABLE_NAME, SPECIFIC_NAME FROM INFORMATION_SCHEMA.TABLE_METHOD_PRIVILEGES WHERE GRANTEE = 'C' ORDER BY 1";
    let before = query(&a, shown);
    assert_eq!(before.len(), 2, "{before:?}");
    assert_eq!(before[0], "PTS,PT_X_OBSERVER");
    assert!(before[1].starts_with("SUBS,PT_GREET"), "{before:?}");
    ok(&a, &["ALTER TYPE SUBA ADD OVERRIDING METHOD GREET () RETURNS INTEGER"]);
    let after = query(&a, shown);
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0], "PTS,PT_X_OBSERVER");
    assert!(after[1].starts_with("SUBS,SUBA_GREET"), "{after:?}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.67 SR1, SR4b, SR4c, SR4e, SR4f,
/// SR4h, §11.69 SR5, SR8b, §11.70 GR1, §11.71 GR1] CREATE TRANSFORM of no
/// user-defined type, of a group a supertype has, of a function not
/// deterministic, of a to-sql function not type-preserving, or of to-sql
/// and from-sql functions of incompatible types is refused (-103). ALTER
/// TRANSFORM adding a function a group has is refused. A distinct type's
/// group and a to-sql method that is type-preserving are defined. ALTER
/// TRANSFORM drops a group's to-sql function, and DROP TRANSFORM CASCADE
/// drops a group.
#[test]
fn transforms_are_checked_by_their_rules_11_67() {
    let db = database();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE SPECIFIC METHOD PT_FROMINT_1 RETURN NEW PT().X(V)",
            "CREATE FUNCTION PT_INT (P PT) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 1",
            "CREATE FUNCTION PT_CHR (P PT) RETURNS CHARACTER(3) DETERMINISTIC CONTAINS SQL RETURN 'A'",
            "CREATE FUNCTION INT_PT (V INTEGER) RETURNS PT DETERMINISTIC CONTAINS SQL RETURN NEW PT().X(V)",
            "CREATE FUNCTION INT_INT (V INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN V",
            "CREATE FUNCTION PT_ANY (P PT) RETURNS INTEGER NOT DETERMINISTIC CONTAINS SQL RETURN 1",
            "CREATE FUNCTION M_DEC (M MONEY) RETURNS DECIMAL(9, 2) DETERMINISTIC CONTAINS SQL RETURN CAST(M AS DECIMAL(9, 2))",
            "CREATE TRANSFORM FOR PT G1 (FROM SQL WITH FUNCTION PT_INT (PT))",
            "CREATE TRANSFORM FOR MONEY G1 (FROM SQL WITH FUNCTION M_DEC (MONEY))",
        ],
    );
    refused(&a, "CREATE TRANSFORM FOR SUBA G1 (FROM SQL WITH FUNCTION PT_INT (PT))", SR, &["§11.67 SR4e", "A.PT"]);
    refused(&a, "CREATE TRANSFORM FOR NOWHERE G2 (FROM SQL WITH FUNCTION PT_INT (PT))", SR, &["§11.67 SR1"]);
    refused(&a, "ALTER TRANSFORM FOR PT G1 (ADD (FROM SQL WITH FUNCTION PT_INT (PT)))", SR, &["§11.67 SR4b, §11.69 SR5"]);
    let incompatible = "CREATE TRANSFORM FOR PT G3 (FROM SQL WITH FUNCTION PT_CHR (PT), TO SQL WITH FUNCTION INT_PT (INTEGER))";
    refused(&a, incompatible, SR, &["§11.67 SR4h"]);
    refused(&a, "CREATE TRANSFORM FOR PT G4 (FROM SQL WITH FUNCTION PT_ANY (PT))", SR, &["§11.67 SR4c", "A.PT_ANY"]);
    refused(&a, "CREATE TRANSFORM FOR PT G5 (TO SQL WITH FUNCTION INT_INT (INTEGER))", SR, &["§11.67 SR4f, §11.69 SR8b"]);
    ok(
        &a,
        &[
            "CREATE TRANSFORM FOR PT G6 (TO SQL WITH METHOD FROMINT (INTEGER) FOR PT)",
            "CREATE TRANSFORM FOR PT G7 (TO SQL WITH FUNCTION INT_PT (INTEGER))",
            "ALTER TRANSFORM FOR PT G1 (ADD (TO SQL WITH SPECIFIC METHOD PT_FROMINT_1))",
        ],
    );
    let groups =
        "SELECT UDT_NAME, GROUP_NAME, TRANSFORM_TYPE FROM INFORMATION_SCHEMA.TRANSFORMS WHERE GROUP_NAME LIKE 'G_' ORDER BY 1, 2, 3";
    assert_eq!(query(&a, groups), ["MONEY,G1,FROM SQL", "PT,G1,FROM SQL", "PT,G1,TO SQL", "PT,G6,TO SQL", "PT,G7,TO SQL"]);
    ok(&a, &["ALTER TRANSFORM FOR PT G1 (DROP (TO SQL RESTRICT))", "DROP TRANSFORM G7 FOR PT CASCADE"]);
    assert_eq!(query(&a, groups), ["MONEY,G1,FROM SQL", "PT,G1,FROM SQL", "PT,G6,TO SQL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 SR8a iii, AR1, §11.64 AR1]
/// CREATE METHOD by a specific name no method has is refused (-103). By a
/// method's specific name it is of the type that has the method, and is
/// refused to one who does not own its schema (-104). DROP CAST is
/// refused to one who does not own the schema of the cast's function,
/// though holding USAGE on its type.
#[test]
fn a_method_and_a_cast_are_of_their_schemas_owner_11_60() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B"]);
    refused(&a, "CREATE SPECIFIC METHOD NOPE RETURN 1", SR, &["§11.60 SR8a iii", "A.NOPE"]);
    refused(&b, "CREATE SPECIFIC METHOD A.PT_FROMINT_1 RETURN NEW A.PT().X(V)", HELD, &["§11.60 AR1", "which B does not own"]);
    ok(
        &a,
        &[
            "CREATE SPECIFIC METHOD PT_FROMINT_1 RETURN NEW PT().X(V)",
            "CREATE FUNCTION PT_INT (P PT) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN P.X",
            "CREATE CAST (PT AS INTEGER) WITH FUNCTION PT_INT (PT)",
        ],
    );
    assert_eq!(query(&a, "SELECT CAST(NEW PT().FROMINT(5) AS INTEGER) FROM PLAIN"), ["5"]);
    refused(&b, "DROP CAST (A.PT AS INTEGER) CASCADE", HELD, &["§11.64 AR1", "B does not own"]);
    ok(&a, &["DROP CAST (PT AS INTEGER) CASCADE"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR9, GR12q] A grant whose grantor
/// loses the grant option on a structured type is abandoned with it.
/// RESTRICT refuses that (2B000), of GRANT OPTION FOR and of the
/// privilege. CASCADE takes the dependent grant, and its grantee no longer
/// names the type.
#[test]
fn a_grant_depending_on_a_grant_option_is_abandoned_12_7() {
    let db = database();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B WITH GRANT OPTION"]);
    ok(&b, &["GRANT USAGE ON TYPE A.PT TO C"]);
    let dependent = (Code::SyntaxRule, "2B000");
    refused(&a, "REVOKE GRANT OPTION FOR USAGE ON TYPE PT FROM B RESTRICT", dependent, &["§12.7 GR12q", "a grant of it"]);
    refused(&a, "REVOKE USAGE ON TYPE PT FROM B RESTRICT", dependent, &["§12.7 GR12q"]);
    ok(&a, &["REVOKE GRANT OPTION FOR USAGE ON TYPE PT FROM B CASCADE"]);
    assert_eq!(udt_privileges(&a), ["A,B,TYPE USAGE,NO"]);
    refused(&c, "CREATE TABLE T (P A.PT)", (Code::Privilege, "2A000"), &["§6.1 AR1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR12o, GR12q] A routine of a
/// parameter of a type, or that casts to it, is abandoned when its owner
/// loses USAGE on the type, structured or distinct. RESTRICT refuses
/// (2B000), and CASCADE drops each routine.
#[test]
fn a_routine_using_a_type_is_abandoned_with_usage_on_it_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B", "GRANT USAGE ON TYPE MONEY TO B"]);
    ok(
        &b,
        &[
            "CREATE FUNCTION OF_PT (P A.PT) RETURNS INTEGER CONTAINS SQL RETURN 1",
            "CREATE FUNCTION TO_PT () RETURNS INTEGER CONTAINS SQL RETURN CASE WHEN CAST(NULL AS A.PT) IS NULL THEN 1 END",
            "CREATE FUNCTION OF_MONEY (M A.MONEY) RETURNS INTEGER CONTAINS SQL RETURN 1",
        ],
    );
    let routines = "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'B' ORDER BY 1";
    assert_eq!(query(&b, routines), ["OF_MONEY", "OF_PT", "TO_PT"]);
    let dependent = (Code::SyntaxRule, "2B000");
    refused(&a, "REVOKE USAGE ON TYPE PT FROM B RESTRICT", dependent, &["§12.7 GR12q"]);
    refused(&a, "REVOKE USAGE ON TYPE MONEY FROM B RESTRICT", dependent, &["§12.7 GR12q"]);
    ok(&a, &["REVOKE USAGE ON TYPE PT FROM B CASCADE"]);
    assert_eq!(query(&b, routines), ["OF_MONEY"]);
    ok(&a, &["REVOKE USAGE ON TYPE MONEY FROM B CASCADE"]);
    assert_eq!(query(&b, routines), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR12d, GR12q] A table's CHECK
/// constraint that casts to a structured type is abandoned when the
/// table's owner loses USAGE on the type. RESTRICT refuses (2B000), and
/// CASCADE drops the constraint and keeps the table.
#[test]
fn a_constraint_using_a_type_is_abandoned_with_usage_on_it_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B"]);
    ok(&b, &["CREATE TABLE T (K INTEGER, CONSTRAINT C CHECK (CAST(NULL AS A.PT) IS NULL OR K > 0))"]);
    refused(&a, "REVOKE USAGE ON TYPE PT FROM B RESTRICT", (Code::SyntaxRule, "2B000"), &["§12.7 GR12q"]);
    ok(&a, &["REVOKE USAGE ON TYPE PT FROM B CASCADE"]);
    let constraints = "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = 'B'";
    assert_eq!(query(&b, constraints), Vec::<String>::new());
    assert_eq!(query(&b, "SELECT COUNT(*) FROM T"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR6, §10.6, §12.2, §12.7 GR9,
/// GR12q] EXECUTE on an attribute's observer or mutator is granted by its
/// specific name or as METHOD x FOR its type, of no parameter or the
/// attribute's type. It is held through a subtype's inherited attribute,
/// shown in ROUTINE_PRIVILEGES and revoked, RESTRICT refusing a view
/// that needs it (2B000) and CASCADE dropping the view.
#[test]
fn an_observer_and_a_mutator_are_granted_and_revoked_12_2() {
    let db = database();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    ok(&a, &["GRANT USAGE ON TYPE PT TO B", "GRANT USAGE ON TYPE SUBA TO B"]);
    ok(&b, &["CREATE TABLE T (P A.PT, S A.SUBA)", "INSERT INTO T VALUES (NULL, NULL)"]);
    assert_eq!(rows(&b, "SELECT T.P.X FROM T"), Err(Code::Privilege));
    refused(&a, "GRANT EXECUTE ON METHOD X FOR PT TO B", SR, &["§10.6 SR3c", "both the observer and the mutator"]);
    refused(&a, "GRANT SELECT ON SPECIFIC FUNCTION PT_X_OBSERVER TO B", SR, &["§12.3 SR4d"]);
    ok(&a, &["GRANT EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER TO B"]);
    assert_eq!(query(&b, "SELECT T.P.X, T.S.X FROM T"), ["NULL,NULL"]);
    refused(&b, "UPDATE T SET P.X = 1", HELD, &["§9.18 SR6"]);
    ok(&a, &["GRANT EXECUTE ON METHOD X (INTEGER) FOR PT TO B"]);
    ok(&b, &["UPDATE T SET P.X = 1 WHERE P IS NOT NULL"]);
    assert_eq!(completed(&b, "GRANT EXECUTE ON SPECIFIC METHOD A.PT_X_OBSERVER TO C"), "01007");
    refused(&c, "GRANT EXECUTE ON SPECIFIC FUNCTION A.PT_X_OBSERVER TO B", HELD, &["§12.2 AR1", "observer of attribute X"]);
    let privileges = "SELECT GRANTEE, SPECIFIC_NAME FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES \
                      WHERE SPECIFIC_NAME LIKE 'PT_X_%' AND GRANTOR = 'A' ORDER BY 2";
    assert_eq!(query(&a, privileges), ["B,PT_X_MUTATOR", "B,PT_X_OBSERVER"]);
    ok(&b, &["CREATE VIEW V AS SELECT T.P.X AS X FROM T"]);
    refused(&a, "REVOKE EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER FROM B RESTRICT", (Code::SyntaxRule, "2B000"), &["§12.7 GR12q"]);
    ok(
        &a,
        &[
            "REVOKE EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER FROM B CASCADE",
            "REVOKE EXECUTE ON METHOD X (INTEGER) FOR PT FROM B RESTRICT",
        ],
    );
    assert_eq!(query(&a, privileges), Vec::<String>::new());
    assert_eq!(query(&b, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'V'"), ["0"]);
    assert_eq!(rows(&b, "SELECT T.S.X FROM T"), Err(Code::Privilege));
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.7 GR9] A grant of EXECUTE on an
/// observer made by a grantee whose grant option goes is abandoned,
/// RESTRICT refusing that (2B000).
#[test]
fn an_observer_grant_depending_on_a_grant_option_is_abandoned_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    ok(&a, &["GRANT EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER TO B WITH GRANT OPTION"]);
    ok(&b, &["GRANT EXECUTE ON SPECIFIC FUNCTION A.PT_X_OBSERVER TO C"]);
    let dependent = (Code::SyntaxRule, "2B000");
    refused(&a, "REVOKE GRANT OPTION FOR EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER FROM B RESTRICT", dependent, &["a grant of it"]);
    assert_eq!(completed(&a, "REVOKE EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER FROM C CASCADE"), "01006");
    ok(&a, &["REVOKE GRANT OPTION FOR EXECUTE ON SPECIFIC FUNCTION PT_X_OBSERVER FROM B CASCADE"]);
    let privileges = "SELECT GRANTOR, GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.ROUTINE_PRIVILEGES \
                      WHERE SPECIFIC_NAME = 'PT_X_OBSERVER' AND GRANTOR <> '_SYSTEM'";
    assert_eq!(query(&a, privileges), ["A,B,NO"]);
}
