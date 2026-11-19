//! The SQL arguments of a polymorphic table function [ISO/IEC 9075-2:2023
//! §9.18 SR12, GR4, §10.4, §20.16] (Features B200, B206): a value of a
//! parameter of a `<data type>` given to the component procedures, of an
//! expression known as the function is compiled or not, NULL and DEFAULT,
//! a value not assignable refused, named arguments, and the
//! forms of a descriptor argument.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N), rows (1, 10), (2, 20), (3, 30); the procedure
/// SCALE_F, fulfill of SCALED (T TABLE WITH ROW SEMANTICS, F INTEGER)
/// RETURNS TABLE (R INTEGER), which pipes each row's K times F, or -1 if
/// F is null.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.run(
        "CREATE PROCEDURE SCALE_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN F INTEGER, IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL \
         BEGIN DECLARE K, N, R INTEGER; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO K, N; IF DONE THEN LEAVE L; END IF; \
         SET R = COALESCE(K * F, -1); SET DESCRIPTOR PTF RR VALUE 1 DATA = R; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION SCALED (T TABLE WITH ROW SEMANTICS, F INTEGER) RETURNS TABLE (R INTEGER) CONTAINS SQL \
         FULFILL WITH PROCEDURE SCALE_F",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap_or_else(|e| panic!("{query}: {e}"));
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class and rule of `query`'s refusal as it opens.
fn refused(s: &Session, query: &str) -> (Code, Option<String>) {
    let Err(e) = s.open_cursor(query) else { panic!("{query} opened") };
    (e.code, e.rule)
}

/// The rows of SCALED of T by the argument `f`, in order.
fn scaled(s: &Session, f: &str) -> Vec<String> {
    rows(s, &format!("SELECT P.R FROM TABLE (SCALED (TABLE (T), {f})) AS P ORDER BY 1"))
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.18 SR12e, GR4c i,
/// §9.27 GR3a] a value argument is given to the component procedures as
/// its parameter: a literal, an expression of literals, a CASE, a CAST
/// and a predicate's truth known as the function is compiled; NULL and
/// DEFAULT give the null value, as no parameter has a default; a column of
/// an outer query is not known then, and is given as it is when the rows
/// are.
#[test]
fn value_arguments_reach_the_component_procedures_9_18() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(scaled(&a, "2"), ["2", "4", "6"]);
    assert_eq!(scaled(&a, "1 + 2"), ["3", "6", "9"]);
    assert_eq!(scaled(&a, "CASE WHEN 1 < 2 THEN 10 ELSE 0 END"), ["10", "20", "30"]);
    assert_eq!(scaled(&a, "CAST('4' AS INTEGER)"), ["4", "8", "12"]);
    assert_eq!(scaled(&a, "CASE WHEN EXISTS (SELECT 1 FROM T) THEN 5 END"), ["5", "10", "15"]);
    assert_eq!(scaled(&a, "NULL"), ["-1", "-1", "-1"]);
    assert_eq!(scaled(&a, "DEFAULT"), ["-1", "-1", "-1"]);
    assert_eq!(scaled(&a, "CAST(NULL AS INTEGER)"), ["-1", "-1", "-1"]);
    let outer = "SELECT O.K, (SELECT MAX(P.R) FROM TABLE (SCALED (TABLE (T), O.K)) AS P) FROM T AS O ORDER BY 1";
    assert_eq!(rows(&a, outer), ["1,3", "2,6", "3,9"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10l iii, §10.4 CR4]
/// an argument not assignable to its parameter's type is refused (SQLCODE
/// -103), and so is one too many; a `<named argument specification>`
/// (Feature T524) gives its parameter the value, and one naming no
/// parameter, a name twice, or a list naming some arguments and not all
/// are refused (SQLCODE -103) [§9.18 SR4, SR5d].
#[test]
fn arguments_not_taken_are_refused_10_4() {
    let db = defined();
    let a = db.session("A");
    assert_eq!(refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T), DATE '2024-01-01')) AS P").0, Code::SyntaxRule);
    assert_eq!(refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T), 1, 2)) AS P").0, Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT P.R FROM TABLE (SCALED (F => 2, T => TABLE (T))) AS P ORDER BY 1"), ["2", "4", "6"]);
    assert_eq!(refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T), G => 1)) AS P").0, Code::SyntaxRule);
    assert_eq!(refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T), F => 1)) AS P").0, Code::SyntaxRule);
    assert_eq!(refused(&a, "SELECT * FROM TABLE (SCALED (T => TABLE (T), T => 1)) AS P").0, Code::SyntaxRule);
    let e = a.open_cursor("SELECT * FROM TABLE (SCALED (TABLE (T), 1 X)) AS P").err().map(|e| e.code);
    assert!(matches!(e, Some(Code::Syntax | Code::SyntaxRule)), "{e:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.4, §20.16 SR1] a
/// descriptor argument names no column twice (SQLCODE -103); its columns
/// may carry data types; CAST (NULL AS DESCRIPTOR) is the null descriptor.
#[test]
fn descriptor_arguments_20_16() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE TAKE_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN CD VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL \
         BEGIN DECLARE K, N INTEGER; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO K, N; IF DONE THEN LEAVE L; END IF; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = K; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run("CREATE FUNCTION TAKE (T TABLE WITH ROW SEMANTICS, C DESCRIPTOR) RETURNS TABLE (R INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE TAKE_F")
        .unwrap();
    assert_eq!(rows(&a, "SELECT P.R FROM TABLE (TAKE (TABLE (T), CAST (NULL AS DESCRIPTOR))) AS P ORDER BY 1"), ["1", "2", "3"]);
    assert_eq!(rows(&a, "SELECT P.R FROM TABLE (TAKE (TABLE (T), DESCRIPTOR (K INTEGER, N))) AS P ORDER BY 1"), ["1", "2", "3"]);
    let twice = refused(&a, "SELECT * FROM TABLE (TAKE (TABLE (T), DESCRIPTOR (K, K))) AS P");
    assert_eq!(twice, (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §20.16 SR1".to_owned())));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR12b ii 2 D I] a table
/// argument's range variable may name its columns, as many as the table
/// has and none twice; too few or one twice is refused (SQLCODE -103).
#[test]
fn table_arguments_name_their_columns_9_18() {
    let db = defined();
    let a = db.session("A");
    let named = "SELECT P.R FROM TABLE (SCALED (TABLE (T) AS Q (A, B), 1)) AS P ORDER BY 1";
    assert_eq!(rows(&a, named), ["1", "2", "3"]);
    let few = refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T) AS Q (A), 1)) AS P");
    assert_eq!(few, (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR12b ii 2 D I 1".to_owned())));
    let twice = refused(&a, "SELECT * FROM TABLE (SCALED (TABLE (T) AS Q (A, A), 1)) AS P");
    assert_eq!(twice, (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR12b ii 2 D I 2".to_owned())));
}
