//! Polymorphic table functions invoked in a `<PTF derived table>`
//! [ISO/IEC 9075-2:2023 §4.35.4, §7.6, §9.24, §9.25] (Features B200 to
//! B209).

use turborel::db::{Database, Session};

/// Schema A: T (K, N), rows (1, 10), (2, 20), (3, 30).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
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

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §9.25 GR20, §20.13, §20.28]
/// A fulfill component procedure fetches each row of its PTF cursor and
/// pipes a row of it doubled, its pass-through columns given back by the
/// surrogate.
#[test]
fn a_fulfill_procedure_pipes_its_rows_9_25() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE DOUBLE_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE V, W INTEGER; DECLARE S BIGINT; DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; \
         L: LOOP FETCH NEXT FROM PTF CU INTO V, S; IF DONE THEN LEAVE L; END IF; \
         SET W = V * 2; SET DESCRIPTOR PTF RR VALUE 1 DATA = W; SET DESCRIPTOR PTF RR VALUE 2 DATA = S; PIPE ROW PTF RR; END LOOP L; END",
    )
    .unwrap();
    a.run(
        "CREATE PROCEDURE ASK (IN FR VARCHAR(128), IN RQ VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL DETERMINISTIC \
         BEGIN SET DESCRIPTOR PTF RQ COUNT = 1; SET DESCRIPTOR PTF RQ VALUE 1 NAME = 'N', TYPE = 4; END",
    )
    .unwrap();
    a.run(
        "CREATE FUNCTION DOUBLED (T TABLE PASS THROUGH WITH ROW SEMANTICS) RETURNS TABLE (D INTEGER) CONTAINS SQL \
         DESCRIBE WITH PROCEDURE ASK FULFILL WITH PROCEDURE DOUBLE_F",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT P.D, X.K FROM TABLE (DOUBLED (TABLE (T) AS X)) AS P ORDER BY X.K"), ["20,1", "40,2", "60,3"]);
}
