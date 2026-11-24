//! Referenceable views and subviews [ISO/IEC 9075-2:2023 §11.32 SR22 to
//! SR24, AR2, GR1, GR2, GR8 to GR10; §9.38; §7.6 GR3b; §11.33 SR4, GR1]:
//! CREATE VIEW OF a structured type with a user-generated or derived
//! self-referencing column, a subview UNDER a superview, a superview read
//! with its subviews' rows and with ONLY its own, SELECT WITH HIERARCHY
//! OPTION on a superview, DROP VIEW of a superview, the Information
//! Schema's rows, and each rule's refusal with its SQLCODE and SQLSTATE.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: PERSON of K, NAME and AGE, REF IS SYSTEM GENERATED; EMPLOYEE
/// under it of PAY; BOSS under EMPLOYEE of BONUS; VPERSON of NAME and AGE,
/// REF USING INTEGER; VEMP under it of PAY; VBOSS under VEMP of BONUS;
/// VODD under VPERSON of OID; DP of REF FROM (X, Y); VLINK of TARGET, a
/// REF(PERSON), and N. Tables PEOPLE of PERSON, K UNIQUE, with STAFF of
/// EMPLOYEE under it and CHIEFS of BOSS under STAFF; T2 of X and Y, its
/// PRIMARY KEY (X, Y). Schema S, also A's; schema B, of B.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE PERSON AS (K INTEGER, NAME CHARACTER VARYING(20), AGE INTEGER) NOT FINAL REF IS SYSTEM GENERATED
           CREATE TYPE EMPLOYEE UNDER PERSON AS (PAY INTEGER) NOT FINAL
           CREATE TYPE BOSS UNDER EMPLOYEE AS (BONUS INTEGER) NOT FINAL
           CREATE TYPE VPERSON AS (NAME CHARACTER VARYING(20), AGE INTEGER) NOT FINAL REF USING INTEGER
           CREATE TYPE VEMP UNDER VPERSON AS (PAY INTEGER) NOT FINAL
           CREATE TYPE VBOSS UNDER VEMP AS (BONUS INTEGER) NOT FINAL
           CREATE TYPE VODD UNDER VPERSON AS (OID INTEGER) NOT FINAL
           CREATE TYPE DP AS (X INTEGER, Y INTEGER) NOT FINAL REF FROM (X, Y)
           CREATE TYPE VLINK AS (TARGET REF(PERSON), N INTEGER) NOT FINAL REF USING INTEGER",
    )
    .unwrap();
    db.schema("CREATE SCHEMA S AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    ok(
        &a,
        &[
            "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED, K WITH OPTIONS UNIQUE)",
            "CREATE TABLE STAFF OF EMPLOYEE UNDER PEOPLE",
            "CREATE TABLE CHIEFS OF BOSS UNDER STAFF",
            "CREATE TABLE T2 (X INTEGER, Y INTEGER, PRIMARY KEY (X, Y))",
            "INSERT INTO PEOPLE (K, NAME, AGE) VALUES (1, 'ANN', 30), (2, 'BOB', 40)",
            "INSERT INTO STAFF (K, NAME, AGE, PAY) VALUES (3, 'CY', 50, 100)",
            "INSERT INTO CHIEFS (K, NAME, AGE, PAY, BONUS) VALUES (4, 'DI', 60, 200, 5)",
            "INSERT INTO T2 VALUES (1, 2), (2, 1)",
        ],
    );
    (db, a)
}

/// VP of VPERSON over PEOPLE; VE of VEMP under it over STAFF; VB of VBOSS
/// under VE over CHIEFS.
fn views(a: &Session) {
    ok(
        a,
        &[
            "CREATE VIEW VP OF VPERSON (REF IS OID USER GENERATED) AS SELECT CAST(K AS REF(VPERSON)), NAME, AGE FROM ONLY (PEOPLE)",
            "CREATE VIEW VE OF VEMP UNDER VP AS SELECT CAST(K AS REF(VEMP)), NAME, AGE, PAY FROM ONLY (STAFF)",
            "CREATE VIEW VB OF VBOSS UNDER VE AS SELECT CAST(K AS REF(VBOSS)), NAME, AGE, PAY, BONUS FROM ONLY (CHIEFS)",
        ],
    );
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

/// Asserts `sql` in `s` is refused with SQLCODE class `code`, SQLSTATE
/// `state` and a message citing `cited`: a query opened as a cursor, any
/// other statement run; rolls back.
fn refused(s: &Session, sql: &str, (code, state, cited): (Code, &str, &str)) {
    let outcome = if sql.starts_with("SELECT") { s.open_cursor(sql).err() } else { s.run(sql).err() };
    let e = outcome.unwrap_or_else(|| panic!("{sql} was not refused"));
    let _ = s.rollback();
    let msg = e.to_string();
    assert_eq!((e.code, e.sqlstate().as_str()), (code, state), "{sql}: {msg}");
    assert!(msg.contains(cited), "{sql}: {msg} does not cite {cited}");
}

/// A Syntax Rule's refusal citing `cited`.
fn syntax(cited: &str) -> (Code, &str, &str) {
    (Code::SyntaxRule, "42000", cited)
}

/// A refusal of a privilege, citing `cited`.
fn privilege(cited: &str) -> (Code, &str, &str) {
    (Code::Privilege, "42000", cited)
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 SR23, GR1e ii, GR2b] A
/// referenceable view of a user-defined reference representation: its
/// self-referencing column first, REF(VPERSON) SCOPE VP, not nullable,
/// then a column of each attribute; its rows are its query's, each
/// self-referencing value the REF its key's value is cast to, distinct of
/// distinct keys and equal of equal ones.
#[test]
fn a_referenceable_view_has_a_self_referencing_column_11_32() {
    let (_db, a) = database();
    views(&a);
    let columns = "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, SCOPE_NAME, IS_SELF_REFERENCING FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_NAME = 'VP' ORDER BY ORDINAL_POSITION";
    assert_eq!(query(&a, columns), ["OID,REF,NO,VP,YES", "NAME,CHARACTER VARYING,YES,NULL,NO", "AGE,INTEGER,YES,NULL,NO"]);
    assert_eq!(query(&a, "SELECT NAME, AGE FROM ONLY (VP) ORDER BY AGE"), ["ANN,30", "BOB,40"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT OID), COUNT(OID) FROM ONLY (VP)"), ["2,2"]);
    ok(&a, &["CREATE VIEW VQ OF VPERSON (REF IS QID) AS SELECT CAST(K AS REF(VPERSON)), NAME, AGE FROM ONLY (PEOPLE)"]);
    let joined = "SELECT P.NAME FROM ONLY (VP) AS P JOIN VQ AS Q ON P.OID = Q.QID ORDER BY P.NAME";
    assert_eq!(query(&a, joined), ["ANN", "BOB"], "a key's REF is the same in both");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 GR3b, §11.32 GR2b ii, iii, §9.38
/// GR3] A superview named without ONLY is its own rows and each proper
/// subview's, cut to its columns; with ONLY, its own rows alone; a
/// subview's own columns are read through it.
#[test]
fn a_superview_is_read_with_its_subviews_7_6() {
    let (_db, a) = database();
    views(&a);
    assert_eq!(query(&a, "SELECT NAME FROM VP ORDER BY NAME"), ["ANN", "BOB", "CY", "DI"]);
    assert_eq!(query(&a, "SELECT NAME FROM ONLY (VP) ORDER BY NAME"), ["ANN", "BOB"]);
    assert_eq!(query(&a, "SELECT NAME, PAY FROM VE ORDER BY NAME"), ["CY,100", "DI,200"]);
    assert_eq!(query(&a, "SELECT NAME, PAY FROM ONLY (VE)"), ["CY,100"]);
    assert_eq!(query(&a, "SELECT NAME, BONUS FROM VB"), ["DI,5"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT OID) FROM VP"), ["4"]);
    let joined = "SELECT P.NAME FROM VP AS P JOIN VE AS E ON P.OID = E.OID ORDER BY P.NAME";
    assert_eq!(query(&a, joined), ["CY", "DI"], "a subview's row is its superrow's");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 GR1b i, SR23s ii] A referenceable
/// view of a derived representation: its self-referencing column, IDV,
/// derived from its derivational attributes' values as a typed table's is,
/// so equal for the same values; SR23s ii 2, 3: each derivational column a
/// column reference, a candidate key among them.
#[test]
fn a_derived_view_derives_its_references_11_32() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE VIEW VD OF DP (REF IS R DERIVED) AS SELECT X, Y FROM T2",
            "CREATE TABLE TD OF DP (REF IS R DERIVED, PRIMARY KEY (X, Y))",
            "INSERT INTO TD (X, Y) VALUES (1, 2)",
        ],
    );
    assert_eq!(query(&a, "SELECT X, Y FROM VD ORDER BY X"), ["1,2", "2,1"]);
    assert_eq!(query(&a, "SELECT COUNT(DISTINCT R) FROM VD"), ["2"]);
    assert_eq!(query(&a, "SELECT V.X FROM VD AS V JOIN TD AS T ON V.R = T.R"), ["1"]);
    refused(&a, "CREATE VIEW V OF DP (REF IS R DERIVED) AS SELECT CAST(X AS INTEGER), Y FROM T2", syntax("§11.32 SR23s ii 2"));
    ok(&a, &["CREATE TABLE T3 (X INTEGER, Y INTEGER)"]);
    refused(&a, "CREATE VIEW V OF DP (REF IS R DERIVED) AS SELECT X, Y FROM T3", syntax("§11.32 SR23s ii 3"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 SR23c, AR2, SR23h iv, SR24b ii]
/// CREATE VIEW OF is refused of a name of no structured type (SQLCODE
/// -103), of a type its owner holds no USAGE on (-104), of USER GENERATED
/// of a derived representation, of DERIVED of a user-defined one, and of
/// a self-referencing column named as an attribute.
#[test]
fn a_referenceable_views_type_is_checked_11_32() {
    let (db, a) = database();
    let q = "AS SELECT CAST(K AS REF(VPERSON)), NAME, AGE FROM ONLY (PEOPLE)";
    refused(&a, &format!("CREATE VIEW V OF NOTHING (REF IS OID) {q}"), syntax("§11.32 SR23c"));
    ok(&a, &["GRANT SELECT ON T2 TO B"]);
    refused(&db.session("B"), "CREATE VIEW V OF A.DP (REF IS R DERIVED) AS SELECT X, Y FROM A.T2", privilege("§11.32 AR2"));
    refused(&a, "CREATE VIEW V OF DP (REF IS R USER GENERATED) AS SELECT X, Y FROM T2", syntax("§11.32 SR23h iv 1 A"));
    refused(&a, &format!("CREATE VIEW V OF VPERSON (REF IS OID DERIVED) {q}"), syntax("§11.32 SR23h iv 2 A"));
    refused(&a, &format!("CREATE VIEW V OF VPERSON (REF IS NAME) {q}"), syntax("§11.32 SR24b ii"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 SR23m to SR23s i] The query of a
/// referenceable view is refused if it is no single query specification,
/// of more than one table, of no table name, of a referenceable table
/// without ONLY, grouped, of an aggregate function in a subquery, of a
/// degree other than n+m, of a column of another type than its attribute,
/// of a basis table of no candidate key of one column, of a first column
/// that casts no such key, or of a first column of REF of another type.
#[test]
fn a_referenceable_views_query_is_checked_11_32() {
    let (_db, a) = database();
    let v = "CREATE VIEW V OF VPERSON (REF IS OID) AS";
    let first = "SELECT CAST(K AS REF(VPERSON)), NAME, AGE";
    refused(&a, &format!("{v} {first} FROM ONLY (PEOPLE) UNION {first} FROM ONLY (STAFF)"), syntax("§11.32 SR23m"));
    refused(&a, &format!("{v} {first} FROM ONLY (PEOPLE), T2"), syntax("§11.32 SR23n"));
    refused(&a, &format!("{v} {first} FROM (SELECT * FROM ONLY (PEOPLE)) AS P"), syntax("§11.32 SR23o"));
    refused(&a, &format!("{v} {first} FROM PEOPLE"), syntax("§11.32 SR23p"));
    refused(&a, &format!("{v} {first} FROM ONLY (PEOPLE) GROUP BY K, NAME, AGE"), syntax("§11.32 SR23q"));
    let aggregated = format!("{v} {first} FROM ONLY (PEOPLE) WHERE AGE > (SELECT MIN(AGE) FROM PEOPLE)");
    refused(&a, &aggregated, syntax("§11.32 SR23r"));
    refused(&a, &format!("{v} SELECT CAST(K AS REF(VPERSON)), NAME FROM ONLY (PEOPLE)"), syntax("§11.32 SR23j"));
    refused(&a, &format!("{v} SELECT NAME, AGE FROM ONLY (PEOPLE)"), syntax("§11.32 SR23j"));
    refused(&a, &format!("{v} SELECT CAST(K AS REF(VPERSON)), NAME, CAST(AGE AS SMALLINT) FROM ONLY (PEOPLE)"), syntax("§11.32 SR23l"));
    ok(&a, &["CREATE TABLE T4 (K INTEGER, NAME CHARACTER VARYING(20), AGE INTEGER)"]);
    refused(&a, &format!("{v} {first} FROM T4"), syntax("§11.32 SR23s i 1"));
    refused(&a, &format!("{v} SELECT CAST(AGE AS REF(VPERSON)), NAME, AGE FROM ONLY (PEOPLE)"), syntax("§11.32 SR23s i 3"));
    refused(&a, &format!("{v} SELECT K, NAME, AGE FROM ONLY (PEOPLE)"), syntax("§11.32 SR23s i 3"));
    refused(&a, &format!("{v} SELECT CAST(K AS REF(VEMP)), NAME, AGE FROM ONLY (PEOPLE)"), syntax("§11.32 SR23s i 4"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 SR23i, SR23d, SR24b i, SR23t]
/// A subview is refused under a base table, a view not referenceable, or
/// none; of a type no direct subtype of its superview's; in another
/// schema; of a type its family has a view of already; of an attribute
/// named as its superview's self-referencing column; of a basis table no
/// proper subtable of its superview's; and of no column reference where
/// its superview's query has one.
#[test]
fn a_subviews_superview_is_checked_11_32() {
    let (_db, a) = database();
    views(&a);
    ok(&a, &["CREATE VIEW PLAIN AS SELECT * FROM T2"]);
    let q = "AS SELECT CAST(K AS REF(VEMP)), NAME, AGE, PAY FROM ONLY (STAFF)";
    refused(&a, &format!("CREATE VIEW V OF VEMP UNDER PEOPLE {q}"), syntax("§11.32 SR23i i"));
    refused(&a, &format!("CREATE VIEW V OF VEMP UNDER PLAIN {q}"), syntax("§11.32 SR23i i"));
    refused(&a, &format!("CREATE VIEW V OF VEMP UNDER NOTHING {q}"), syntax("§11.32 SR23i i"));
    refused(&a, &format!("CREATE VIEW V OF VBOSS UNDER VP {q}"), syntax("§11.32 SR23i ii"));
    refused(&a, &format!("CREATE VIEW S.V OF VEMP UNDER A.VP {q}"), syntax("§11.32 SR23i iii"));
    refused(&a, &format!("CREATE VIEW V OF VEMP UNDER VP {q}"), syntax("§11.32 SR23d"));
    refused(
        &a,
        "CREATE VIEW V OF VODD UNDER VP AS SELECT CAST(K AS REF(VODD)), NAME, AGE, PAY FROM ONLY (STAFF)",
        syntax("§11.32 SR24b i"),
    );
    ok(&a, &["DROP VIEW VE CASCADE"]);
    refused(
        &a,
        "CREATE VIEW V OF VEMP UNDER VP AS SELECT CAST(K AS REF(VEMP)), NAME, AGE, AGE FROM ONLY (PEOPLE)",
        syntax("§11.32 SR23t i"),
    );
    let renamed =
        "CREATE VIEW V OF VEMP UNDER VP AS SELECT CAST(K AS REF(VEMP)), CAST(NAME AS CHARACTER VARYING(20)), AGE, PAY FROM ONLY (STAFF)";
    refused(&a, renamed, syntax("§11.32 SR23t iii"));
    ok(&a, &["CREATE VIEW VE OF VEMP UNDER VP AS SELECT CAST(K AS REF(VEMP)), NAME, AGE, PAY FROM ONLY (STAFF)"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 SR23u, GR1e ii 2 C] A view column
/// option gives a column of a reference type a scope, a referenceable
/// table of its referenced type; it is refused of no originally-defined
/// attribute, twice, of an attribute of no reference type, of a scope no
/// referenceable table, and of a scope of another type, a subtable or a
/// view.
#[test]
fn view_column_options_scope_their_columns_11_32() {
    let (_db, a) = database();
    ok(
        &a,
        &[
            "CREATE TABLE LINKS (K INTEGER PRIMARY KEY, TARGET REF(PERSON), N INTEGER)",
            "CREATE VIEW VL OF VLINK (REF IS LID, TARGET WITH OPTIONS SCOPE PEOPLE) AS SELECT CAST(K AS REF(VLINK)), TARGET, N FROM LINKS",
        ],
    );
    let scope = "SELECT SCOPE_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'VL' AND COLUMN_NAME = 'TARGET'";
    assert_eq!(query(&a, scope), ["PEOPLE"]);
    let q = "AS SELECT CAST(K AS REF(VLINK)), TARGET, N FROM LINKS";
    refused(&a, &format!("CREATE VIEW V OF VLINK (REF IS LID, NOPE WITH OPTIONS SCOPE PEOPLE) {q}"), syntax("§11.32 SR23u i"));
    let twice = format!("CREATE VIEW V OF VLINK (REF IS LID, TARGET WITH OPTIONS SCOPE PEOPLE, TARGET WITH OPTIONS SCOPE PEOPLE) {q}");
    refused(&a, &twice, syntax("§11.32 SR23u ii"));
    refused(&a, &format!("CREATE VIEW V OF VLINK (REF IS LID, N WITH OPTIONS SCOPE PEOPLE) {q}"), syntax("§11.32 SR23u iii 1"));
    refused(&a, &format!("CREATE VIEW V OF VLINK (REF IS LID, TARGET WITH OPTIONS SCOPE T2) {q}"), syntax("§11.32 SR23u iii 2"));
    refused(&a, &format!("CREATE VIEW V OF VLINK (REF IS LID, TARGET WITH OPTIONS SCOPE STAFF) {q}"), syntax("§11.32 SR23u iii 3"));
    views(&a);
    refused(&a, &format!("CREATE VIEW V OF VLINK (REF IS LID, TARGET WITH OPTIONS SCOPE VP) {q}"), syntax("§11.32 SR23u iii 3"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 GR8a, GR9, GR10, §12.2 GR8, §7.6
/// AR1b ii 2] The owner of a referenceable view holds SELECT WITH
/// HIERARCHY OPTION on it, grantable. Granted it on a superview, a user
/// reads its subviews, and the superview with ONLY; granted SELECT alone,
/// a user reads the superview, its subviews' rows too, but no subview
/// alone, nor the superview with ONLY.
#[test]
fn select_with_hierarchy_option_reaches_subviews_11_32() {
    let (db, a) = database();
    views(&a);
    let held = "SELECT PRIVILEGE_TYPE, IS_GRANTABLE, WITH_HIERARCHY FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES \
                WHERE TABLE_NAME = 'VP' AND GRANTEE = 'A' AND PRIVILEGE_TYPE = 'SELECT'";
    assert_eq!(query(&a, held), ["SELECT,YES,YES"]);
    ok(&a, &["GRANT SELECT ON VP TO B WITH HIERARCHY OPTION", "GRANT SELECT ON VP TO C"]);
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT NAME FROM A.VB"), ["DI"]);
    assert_eq!(query(&b, "SELECT NAME FROM ONLY (A.VP) ORDER BY NAME"), ["ANN", "BOB"]);
    let c = db.session("C");
    assert_eq!(query(&c, "SELECT COUNT(*) FROM A.VP"), ["4"]);
    refused(&c, "SELECT NAME FROM A.VE", privilege("SELECT"));
    refused(&c, "SELECT NAME FROM ONLY (A.VP)", privilege("§7.6 AR1b ii 2"));
}

/// Verifies: [DESIGN] (RULES.md D81) A referenceable view is read-only
/// here. An INSERT, UPDATE or DELETE of it, and WITH CHECK OPTION of one,
/// are refused with SQLCODE -900.
#[test]
fn a_referenceable_view_is_read_only() {
    let (_db, a) = database();
    views(&a);
    let unsupported = (Code::Unsupported, "0A000", "RULES.md D81");
    refused(&a, "INSERT INTO VP (NAME, AGE) VALUES ('EVE', 1)", unsupported);
    refused(&a, "UPDATE VP SET AGE = 1", unsupported);
    refused(&a, "DELETE FROM VE", unsupported);
    let checked = "CREATE VIEW V OF VPERSON (REF IS OID) AS SELECT CAST(K AS REF(VPERSON)), NAME, AGE FROM ONLY (PEOPLE) WITH CHECK OPTION";
    refused(&a, checked, unsupported);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.33 SR4, GR1] DROP VIEW of a view with
/// a subview is refused with RESTRICT; with CASCADE, its subviews go too,
/// the deepest first.
#[test]
fn a_superview_is_dropped_with_its_subviews_11_33() {
    let (_db, a) = database();
    views(&a);
    refused(&a, "DROP VIEW VP RESTRICT", syntax("§11.33 SR4"));
    refused(&a, "DROP VIEW VE RESTRICT", syntax("§11.33 SR4"));
    ok(&a, &["DROP VIEW VB RESTRICT", "DROP VIEW VP CASCADE"]);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME IN ('VP', 'VE', 'VB')"), ["0"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32 GR1] The Information Schema's
/// TABLES row of a referenceable view names its self-referencing column,
/// its generation and its type, IS_TYPED YES; DIRECT_SUPERTABLES has a
/// row of each subview and its direct superview.
#[test]
fn the_information_schema_shows_referenceable_views_11_32() {
    let (_db, a) = database();
    views(&a);
    let tables = "SELECT TABLE_NAME, TABLE_TYPE, SELF_REFERENCING_COLUMN_NAME, REFERENCE_GENERATION, USER_DEFINED_TYPE_NAME, IS_TYPED \
                  FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME IN ('VP', 'VE') ORDER BY TABLE_NAME";
    assert_eq!(query(&a, tables), ["VE,VIEW,OID,USER GENERATED,VEMP,YES", "VP,VIEW,OID,USER GENERATED,VPERSON,YES"]);
    let supers =
        "SELECT TABLE_NAME, SUPERTABLE_NAME FROM INFORMATION_SCHEMA.DIRECT_SUPERTABLES WHERE TABLE_NAME LIKE 'V%' ORDER BY TABLE_NAME";
    assert_eq!(query(&a, supers), ["VB,VE", "VE,VP"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.22, §6.24, §6.17, §4.17.9] A
/// reference scoped by a referenceable view is dereferenced through the
/// view's family: `->` reads the row of the view or a subview, DEREF gives
/// it as a value of the most specific type of its view, and TREAT to a
/// reference of a subview's type checks the row is of one and gives a
/// reference of no scope.
#[test]
fn references_are_dereferenced_through_a_view_scope_6_24() {
    let (_db, a) = database();
    views(&a);
    ok(&a, &["CREATE TABLE HOLDS (R REF(VPERSON) SCOPE VP)", "INSERT INTO HOLDS SELECT OID FROM VP"]);
    assert_eq!(query(&a, "SELECT R -> NAME FROM HOLDS ORDER BY 1"), ["ANN", "BOB", "CY", "DI"]);
    let specific = "SELECT D.N, D.T FROM (SELECT R -> NAME, DEREF(R) IS OF (ONLY VPERSON) FROM HOLDS) AS D (N, T) ORDER BY 1";
    assert_eq!(query(&a, specific), ["ANN,TRUE", "BOB,TRUE", "CY,FALSE", "DI,FALSE"]);
    let boss = "SELECT D.N FROM (SELECT R -> NAME, DEREF(R) IS OF (VBOSS) FROM HOLDS) AS D (N, B) WHERE D.B";
    assert_eq!(query(&a, boss), ["DI"]);
    let treated = "SELECT COUNT(D.R) FROM (SELECT TREAT(R AS REF(VEMP)) FROM HOLDS WHERE R -> AGE >= 50) AS D (R)";
    assert_eq!(query(&a, treated), ["2"]);
    // [§6.22 SR1] TREAT gives the target type, of no scope.
    refused(&a, "SELECT TREAT(R AS REF(VEMP)) -> NAME FROM HOLDS", syntax("§6.22 SR1"));
}
