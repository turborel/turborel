//! A polymorphic table function's component procedures: a describe
//! component that gives the result row type and the columns it reads,
//! private parameters through start, fulfill and finish, and descriptor
//! parameters [ISO/IEC 9075-2:2023 §9.24, §9.25, §9.26, §20.4 to §20.6,
//! §20.28] (Features B205, B206, B208, B209).

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N, M), rows (1, 10, 100), (2, 20, 200).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER, M INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1, 10, 100), (2, 20, 200)").unwrap();
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

/// PICK (T TABLE PASS THROUGH WITH ROW SEMANTICS, C DESCRIPTOR), of no
/// column list: its describe component reads the one column C names and
/// gives the result one column, PICKED, of its type; its fulfill pipes
/// each row's value of it.
fn pick(a: &Session) {
    a.run(
        "CREATE PROCEDURE PICK_D (IN FR VARCHAR(128), IN RQ VARCHAR(128), IN CD VARCHAR(128), IN IR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL DETERMINISTIC \
         BEGIN DECLARE C, N, I, T INTEGER; DECLARE NM VARCHAR(128); DECLARE WANTED VARCHAR(128); \
         GET DESCRIPTOR PTF CD VALUE 1 WANTED = NAME; GET DESCRIPTOR PTF FR C = COUNT; SET I = 1; \
         WHILE I <= C DO GET DESCRIPTOR PTF FR VALUE I NM = NAME; IF NM = WANTED THEN SET N = I; END IF; SET I = I + 1; END WHILE; \
         IF N IS NULL THEN SET ST = '22023'; ELSE \
         COPY PTF FR VALUE N (NAME, TYPE) TO PTF RQ VALUE 1; \
         SET DESCRIPTOR PTF IR COUNT = 1; COPY PTF FR VALUE N (NAME, TYPE) TO PTF IR VALUE 1; \
         SET DESCRIPTOR PTF IR VALUE 1 NAME = 'PICKED'; END IF; END",
    )
    .unwrap();
    a.run(
        "CREATE PROCEDURE PICK_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN CD VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL \
         BEGIN DECLARE V INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO V, S; IF DONE THEN LEAVE L; END IF; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = V; SET DESCRIPTOR PTF RR VALUE 2 DATA = S; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION PICK (T TABLE PASS THROUGH WITH ROW SEMANTICS, C DESCRIPTOR) RETURNS TABLE CONTAINS SQL \
         DESCRIBE WITH PROCEDURE PICK_D FULFILL WITH PROCEDURE PICK_F",
    )
    .unwrap();
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.24 GR3, §9.18 SR12k, SR12l, §20.4, §20.6] (Features B205, B206, B209)
/// A describe component reads a descriptor argument and the full row
/// type, gives the requested row type and the initial result row type
/// they choose; the result's columns are its proper column and the
/// table's, passed through.
#[test]
fn a_describe_component_gives_the_result_row_type_9_24() {
    let db = defined();
    let a = db.session("A");
    pick(&a);
    let q = "SELECT P.PICKED, Q.K, Q.N, Q.M FROM TABLE (PICK (TABLE (T) AS Q, DESCRIPTOR (M))) AS P ORDER BY Q.K";
    assert_eq!(rows(&a, q), ["100,1,10,100", "200,2,20,200"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE (PICK (TABLE (T), DESCRIPTOR (N))) AS P ORDER BY 2"), ["10,1,10,100", "20,2,20,200"]);
    let e = a.open_cursor("SELECT * FROM TABLE (PICK (TABLE (T), DESCRIPTOR (Z))) AS P").err().expect("an unknown column refused");
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §9.18 SR12j")), "{e:?}");
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.25 GR4, GR17 to GR19, §9.26] (Feature B208)
/// Private parameters, their defaults, set by the start component, kept
/// through fulfill, and read by finish, which pipes a row of its own; of
/// an empty table of row semantics, none runs.
#[test]
fn private_parameters_pass_through_the_components_9_25() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE RUN_S (INOUT SEEN INTEGER, IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL SET SEEN = SEEN + 1000").unwrap();
    a.run(
        "CREATE PROCEDURE RUN_F (INOUT SEEN INTEGER, IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) \
         CONTAINS SQL \
         BEGIN DECLARE K, N, M INTEGER; DECLARE DONE BOOLEAN DEFAULT FALSE; DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO K, N, M; IF DONE THEN LEAVE L; END IF; SET SEEN = SEEN + N; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = SEEN; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run(
        "CREATE PROCEDURE RUN_E (IN SEEN INTEGER, IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE FINAL_R INTEGER; SET FINAL_R = - SEEN; SET DESCRIPTOR PTF RR VALUE 1 DATA = FINAL_R; PIPE ROW PTF RR; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION RUNNING (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (R INTEGER) CONTAINS SQL PRIVATE DATA (SEEN INTEGER DEFAULT 5) \
         START WITH PROCEDURE RUN_S FULFILL WITH PROCEDURE RUN_F FINISH WITH PROCEDURE RUN_E",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT P.R FROM TABLE (RUNNING (TABLE (SELECT * FROM T ORDER BY K))) AS P"), ["1015", "1035", "-1035"]);
    // [GR4] Of an empty table of row semantics, no component runs.
    assert_eq!(rows(&a, "SELECT P.R FROM TABLE (RUNNING (TABLE (SELECT * FROM T WHERE K > 9))) AS P"), Vec::<String>::new());
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.25 GR17c, GR20b, GR22]
/// A component that sets its status to an exception's SQLSTATE raises it,
/// and the query fails with it.
#[test]
fn a_status_raised_fails_the_query_9_25() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE BAD_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL SET ST = '22012'").unwrap();
    a.run("CREATE FUNCTION BAD (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (R INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE BAD_F").unwrap();
    let e = a.open_cursor("SELECT * FROM TABLE (BAD (TABLE (T))) AS P").and_then(|mut c| c.fetch().map(drop)).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22012", "{e:?}");
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §20.28 GR2, GR4]
/// PIPE ROW outside a PTF component's run, by a procedure CALLed or a
/// function a query evaluates, raises 0700H; and in a component, of a
/// name not the intermediate result row's, 33000.
#[test]
fn pipe_row_is_of_a_component_alone_20_28() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE LOOSE (IN RR VARCHAR(128)) CONTAINS SQL PIPE ROW PTF RR").unwrap();
    let e = a.run("CALL LOOSE ('X')").unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "0700H", "{e:?}");
    // Of a function a query evaluates, too.
    a.run("CREATE FUNCTION LOOSE_F () RETURNS INTEGER CONTAINS SQL BEGIN PIPE ROW PTF 'X'; RETURN 1; END").unwrap();
    let e = a.open_cursor("SELECT LOOSE_F () FROM T").and_then(|mut c| c.fetch().map(drop)).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "0700H", "{e:?}");
    a.run("CREATE PROCEDURE WRONG_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL PIPE ROW PTF CR").unwrap();
    a.run("CREATE FUNCTION WRONG (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (R INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE WRONG_F")
        .unwrap();
    let e = a.open_cursor("SELECT * FROM TABLE (WRONG (TABLE (T))) AS P").and_then(|mut c| c.fetch().map(drop)).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "33000", "{e:?}");
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §7.6 SR7d i, SR15a; §9.25 GR20] (Feature B205)
/// A function of RETURNS ONLY PASS THROUGH: its rows its table argument's
/// rows it pipes, of no proper column and no correlation name, its range
/// variable naming them. [DESIGN] The Information Schema shows it a
/// function, its result of no data type; [EXTENSION] its generic table
/// parameter of DATA_TYPE TABLE.
#[test]
fn only_pass_through_gives_the_rows_piped_7_6() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE KEEP_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE K, N, M INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO K, N, M, S; IF DONE THEN LEAVE L; END IF; \
         IF N > 15 THEN SET DESCRIPTOR PTF RR VALUE 1 DATA = S; PIPE ROW PTF RR; END IF; END LOOP L; END",
    )
    .unwrap();
    a.run("CREATE FUNCTION KEPT (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS ONLY PASS THROUGH CONTAINS SQL FULFILL WITH PROCEDURE KEEP_F")
        .unwrap();
    assert_eq!(rows(&a, "SELECT * FROM TABLE (KEPT (TABLE (T)))"), ["2,20,200"]);
    assert_eq!(rows(&a, "SELECT Q.M FROM TABLE (KEPT (TABLE (T) AS Q))"), ["200"]);
    let Err(e) = a.open_cursor("SELECT * FROM TABLE (KEPT (TABLE (T))) AS P") else { panic!("a correlation name taken") };
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §7.6 SR7d i")), "{e:?}");
    let shown = rows(
        &a,
        "SELECT ROUTINE_NAME, ROUTINE_TYPE, COALESCE(DATA_TYPE, '-'), IS_DETERMINISTIC, SQL_DATA_ACCESS FROM INFORMATION_SCHEMA.ROUTINES \
         WHERE ROUTINE_NAME = 'KEPT'",
    );
    assert_eq!(shown, ["KEPT,FUNCTION,-,NO,CONTAINS SQL"]);
    let parameter =
        rows(&a, "SELECT PARAMETER_NAME, COALESCE(DATA_TYPE, '-') FROM INFORMATION_SCHEMA.PARAMETERS WHERE SPECIFIC_NAME LIKE 'KEPT%'");
    assert_eq!(parameter, ["T,TABLE"]);
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.18 SR12i, §9.24 GR3] [DESIGN]
/// A describe component that reads SQL-data reads it as the statement is
/// bound, in the transaction the statement runs in; in a view's
/// definition, bound where no statement runs, it is valid SQL not
/// implemented, SQLCODE -900.
#[test]
fn a_describe_component_reads_sql_data_9_24() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE CHOICE (C VARCHAR(128))").unwrap();
    a.run("INSERT INTO CHOICE VALUES ('M')").unwrap();
    a.run(
        "CREATE PROCEDURE CHOSEN_D (IN FR VARCHAR(128), IN RQ VARCHAR(128), IN IR VARCHAR(128), INOUT ST CHAR(5)) \
         READS SQL DATA DETERMINISTIC \
         BEGIN DECLARE C, N, I INTEGER; DECLARE NM, WANTED VARCHAR(128); \
         SELECT MAX(CHOICE.C) INTO WANTED FROM CHOICE; GET DESCRIPTOR PTF FR C = COUNT; SET I = 1; \
         WHILE I <= C DO GET DESCRIPTOR PTF FR VALUE I NM = NAME; IF NM = WANTED THEN SET N = I; END IF; SET I = I + 1; END WHILE; \
         COPY PTF FR VALUE N (NAME, TYPE) TO PTF RQ VALUE 1; \
         SET DESCRIPTOR PTF IR COUNT = 1; COPY PTF FR VALUE N (NAME, TYPE) TO PTF IR VALUE 1; END",
    )
    .unwrap();
    a.run(
        "CREATE PROCEDURE CHOSEN_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE V INTEGER; DECLARE DONE BOOLEAN DEFAULT FALSE; DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO V; IF DONE THEN LEAVE L; END IF; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = V; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION CHOSEN (T TABLE WITH ROW SEMANTICS) RETURNS TABLE READS SQL DATA \
         DESCRIBE WITH PROCEDURE CHOSEN_D FULFILL WITH PROCEDURE CHOSEN_F",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT P.M FROM TABLE (CHOSEN (TABLE (T))) AS P ORDER BY 1"), ["100", "200"]);
    a.run("UPDATE CHOICE SET C = 'N'").unwrap();
    assert_eq!(rows(&a, "SELECT * FROM TABLE (CHOSEN (TABLE (T))) AS P ORDER BY 1"), ["10", "20"]);
    let e = a.run("CREATE VIEW V AS SELECT * FROM TABLE (CHOSEN (TABLE (T))) AS P").unwrap_err();
    assert_eq!(e.code, Code::Unsupported, "{e:?}");
}
