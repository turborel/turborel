//! The rules a polymorphic table function's definition and invocation
//! keep, its privileges and what depends on it [ISO/IEC 9075-2:2023 §7.6
//! SR7d, §9.18 SR5e, SR12, §11.60 SR10, AR3, §11.62] (Features B200,
//! B203, B205).

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::Scratch;

/// Schema A: T (K, N), rows (1, 10), (2, 20); U (K, W), row (1, 7); the
/// procedure ECHO_F, fulfill of ECHO (T TABLE PASS THROUGH WITH SET
/// SEMANTICS) RETURNS TABLE (E INTEGER), which pipes each row's K.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER) CREATE TABLE U (K INTEGER, W INTEGER)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("INSERT INTO U VALUES (1, 7)").unwrap();
    a.run(
        "CREATE PROCEDURE ECHO_F (IN CR VARCHAR(128), IN PT VARCHAR(128), IN OD VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), \
         INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE K, N INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO K, N, S; IF DONE THEN LEAVE L; END IF; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = K; SET DESCRIPTOR PTF RR VALUE 2 DATA = S; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run("CREATE FUNCTION ECHO (T TABLE PASS THROUGH WITH SET SEMANTICS) RETURNS TABLE (E INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE ECHO_F")
        .unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// Asserts that opening `query` in `s` fails with `code` under `rule`.
fn refused(s: &Session, query: &str, code: Code, rule: &str) {
    let Err(e) = s.open_cursor(query) else { panic!("{query} opened") };
    assert_eq!((e.code, e.rule.as_deref()), (code, Some(rule)), "{query}: {e:?}");
}

/// Asserts that running `statement` in `s` fails with `code` under `rule`.
fn refused_run(s: &Session, statement: &str, code: Code, rule: &str) {
    let e = s.run(statement).unwrap_err();
    assert_eq!((e.code, e.rule.as_deref()), (code, Some(rule)), "{statement}: {e:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.6 SR7d ii, SR7d ii 1, SR7d ii 3 A I, SR7d ii 4]
/// A `<PTF derived table>` of proper result columns has a correlation
/// name, not one of a table argument's range variables, and a derived
/// column list of one name for each proper column, each SQLCODE -103.
#[test]
fn a_ptf_derived_table_is_named_7_6() {
    let db = defined();
    let a = db.session("A");
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §7.6 {r}");
    refused(&a, "SELECT * FROM TABLE (ECHO (TABLE (T)))", Code::SyntaxRule, &rule("SR7d ii"));
    refused(&a, "SELECT * FROM TABLE (ECHO (TABLE (T) AS P)) AS P", Code::SyntaxRule, &rule("SR7d ii 1"));
    refused(&a, "SELECT * FROM TABLE (ECHO (TABLE (T))) AS P (X, Y)", Code::SyntaxRule, &rule("SR7d ii 3 A I"));
    assert_eq!(rows(&a, "SELECT P.X, T.N FROM TABLE (ECHO (TABLE (T))) AS P (X) ORDER BY 1"), ["1,10", "2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR12b iii 1, SR12b iii 2, SR12b vi 1, SR12c, SR12d]
/// PARTITION BY and ORDER BY of a table argument of set semantics alone,
/// of its own columns; no two range variables alike; co-partitioned
/// tables partitioned alike, each SQLCODE -103.
#[test]
fn table_arguments_keep_their_rules_9_18() {
    let db = defined();
    let a = db.session("A");
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §9.18 {r}");
    a.run("CREATE PROCEDURE ROW_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL SET ST = '00000'")
        .unwrap();
    a.run("CREATE FUNCTION BY_ROW (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (E INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE ROW_F")
        .unwrap();
    refused(&a, "SELECT * FROM TABLE (BY_ROW (TABLE (T) PARTITION BY K)) AS P", Code::SyntaxRule, &rule("SR12b iii 1"));
    refused(&a, "SELECT * FROM TABLE (BY_ROW (TABLE (T) ORDER BY K)) AS P", Code::SyntaxRule, &rule("SR12b vi 1"));
    refused(&a, "SELECT * FROM TABLE (ECHO (TABLE (T) PARTITION BY Z)) AS P", Code::SyntaxRule, &rule("SR12b iii 2"));
    refused(&a, "SELECT * FROM TABLE (ECHO (TABLE (T) PARTITION BY (K, K))) AS P", Code::SyntaxRule, &rule("SR12b iii 2"));
    a.run(
        "CREATE PROCEDURE TWO_F (IN C1 VARCHAR(128), IN P1 VARCHAR(128), IN O1 VARCHAR(128), IN U1 VARCHAR(128), \
         IN C2 VARCHAR(128), IN P2 VARCHAR(128), IN O2 VARCHAR(128), IN U2 VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL SET ST = '00000'",
    )
    .unwrap();
    a.run("CREATE FUNCTION TWO (A TABLE, B TABLE) RETURNS TABLE (E INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE TWO_F").unwrap();
    refused(&a, "SELECT * FROM TABLE (TWO (TABLE (T), TABLE (T))) AS P", Code::SyntaxRule, &rule("SR12c"));
    let q = "SELECT * FROM TABLE (TWO (TABLE (T) AS X PARTITION BY K, TABLE (U) AS Y PARTITION BY (K, W) COPARTITION (X, Y))) AS P";
    refused(&a, q, Code::SyntaxRule, &rule("SR12d ii 3"));
    let q = "SELECT * FROM TABLE (TWO (TABLE (T) AS X PARTITION BY K, TABLE (U) AS Y COPARTITION (X, Y))) AS P";
    refused(&a, q, Code::SyntaxRule, &rule("SR12d ii 3"));
    assert_eq!(rows(&a, "SELECT * FROM TABLE (TWO (TABLE (T) AS X, TABLE (T) AS Y)) AS P"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR10h, SR10j, SR10m]
/// A component procedure conforms to its signature and modifies no
/// SQL-data; a function of no column list nor ONLY PASS THROUGH has a
/// describe component, each SQLCODE -103.
#[test]
fn component_procedures_conform_11_60() {
    let db = defined();
    let a = db.session("A");
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §11.60 {r}");
    let wrong = "CREATE FUNCTION W (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (E INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE ECHO_F";
    refused_run(&a, wrong, Code::SyntaxRule, &rule("SR10h"));
    a.run(
        "CREATE PROCEDURE CHANGE_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) MODIFIES SQL DATA \
         DELETE FROM T",
    )
    .unwrap();
    let modifies =
        "CREATE FUNCTION M (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (E INTEGER) MODIFIES SQL DATA FULFILL WITH PROCEDURE CHANGE_F";
    let e = a.run(modifies).unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e:?}");
    let undescribed = "CREATE FUNCTION D (T TABLE WITH ROW SEMANTICS) RETURNS TABLE CONTAINS SQL FULFILL WITH PROCEDURE CHANGE_F";
    refused_run(&a, undescribed, Code::SyntaxRule, &rule("SR10m"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR5e, §10.4 SR9a]
/// A polymorphic table function of a generic table parameter is no
/// candidate of a function invoked in a value expression.
#[test]
fn a_ptf_is_no_scalar_function_9_18() {
    let db = defined();
    let a = db.session("A");
    a.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    let Err(e) = a.open_cursor("SELECT ECHO (CAST (K AS VARCHAR(128))) FROM T") else { panic!("ECHO invoked as a value") };
    assert_eq!(e.code, Code::SyntaxRule, "{e:?}");
    assert!(e.msg.contains("no function named ECHO"), "{e:?}");
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §7.6 SR15a, §9.18 SR12b ii 1] (Feature B205)
/// A `<PTF derived table>` as a table argument of another: the outer's
/// columns its own, then the inner's passed through, of their range
/// variables.
#[test]
fn a_nested_invocation_is_a_table_argument_7_6() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE OUTER_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE E, K, N, W INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO E, K, N, S; IF DONE THEN LEAVE L; END IF; SET W = E + N; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = W; SET DESCRIPTOR PTF RR VALUE 2 DATA = S; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run("CREATE FUNCTION OUTER_T (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS TABLE (O INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE OUTER_F")
        .unwrap();
    let q = "SELECT P.O, I.E, T.N FROM TABLE (OUTER_T (ECHO (TABLE (T)) AS I)) AS P ORDER BY T.K";
    assert_eq!(rows(&a, q), ["11,1,10", "22,2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 AR1, §11.60 AR3, §12.3, §11.62 SR4b]
/// Invoking a polymorphic table function needs EXECUTE on it, not on its
/// components; its owner holds EXECUTE on them; a component procedure is
/// not dropped RESTRICT while the function is.
#[test]
fn privileges_and_dependencies_12_3() {
    let db = defined();
    let a = db.session("A");
    let b = db.session("B");
    a.run("GRANT SELECT ON T TO B").unwrap();
    a.run("COMMIT").unwrap();
    let q = "SELECT P.E FROM TABLE (A.ECHO (TABLE (A.T))) AS P ORDER BY 1";
    let Err(e) = b.open_cursor(q) else { panic!("B invoked ECHO without EXECUTE") };
    assert_eq!(e.code, Code::Privilege, "{e:?}");
    b.run("ROLLBACK").unwrap();
    a.run("GRANT EXECUTE ON FUNCTION ECHO TO B").unwrap();
    a.run("COMMIT").unwrap();
    assert_eq!(rows(&b, q), ["1", "2"]);
    b.run("COMMIT").unwrap();
    let e = a.run("DROP PROCEDURE ECHO_F RESTRICT").unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e:?}");
    a.run("DROP PROCEDURE ECHO_F CASCADE").unwrap();
    a.run("COMMIT").unwrap();
    assert!(b.open_cursor(q).is_err());
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §11.32 SR10, §11.62 SR4b; §4.35.4]
/// A view over a `<PTF derived table>` gives its rows; the function is not
/// dropped RESTRICT while the view is; the function, its components and
/// the view are kept as the database is reopened, from the log and from a
/// checkpoint.
#[test]
fn a_ptf_is_kept_in_views_and_across_reopening_11_32() {
    let scratch = Scratch::new("ptf-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
        a.run(
            "CREATE PROCEDURE ECHO_F (IN CR VARCHAR(128), IN PT VARCHAR(128), IN OD VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), \
             INOUT ST CHAR(5)) CONTAINS SQL \
             BEGIN DECLARE K, N INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
             DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
             L: LOOP FETCH NEXT FROM PTF CU INTO K, N, S; IF DONE THEN LEAVE L; END IF; \
             SET DESCRIPTOR PTF RR VALUE 1 DATA = K; SET DESCRIPTOR PTF RR VALUE 2 DATA = S; PIPE ROW PTF RR; END LOOP L; END",
        )
        .unwrap();
        a.run(
            "CREATE FUNCTION ECHO (T TABLE PASS THROUGH WITH SET SEMANTICS) RETURNS TABLE (E INTEGER) CONTAINS SQL \
             FULFILL WITH PROCEDURE ECHO_F",
        )
        .unwrap();
        a.run("CREATE VIEW V AS SELECT P.E, T.N FROM TABLE (ECHO (TABLE (T) PARTITION BY K)) AS P").unwrap();
        let e = a.run("DROP FUNCTION ECHO RESTRICT").unwrap_err();
        assert_eq!(e.code, Code::SyntaxRule, "{e:?}");
        a.commit().unwrap();
        db.close().unwrap();
    }
    let check = |db: &Database| assert_eq!(rows(&db.session("A"), "SELECT E, N FROM V ORDER BY E"), ["1,10", "2,20"]);
    {
        let db = Database::open(&dir).unwrap();
        check(&db);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    check(&db);
    db.close().unwrap();
}
