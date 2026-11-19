//! [ISO/IEC 9075-2:2023 §7.6, §7.17, §7.19] Where a data change delta
//! table stands, in a WITH list element, in an INSERT's query and as a
//! MERGE's source, never in a subquery or a view; of an updatable view;
//! of a typed table, its subtables' rows by their superrows (RULES.md
//! D189).

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};

/// A session of `A` whose schema holds `T (K, N)` with rows 1 to 3, an
/// empty `ARCHIVE` like it, and `V`, a view of T WITH CASCADED CHECK
/// OPTION whose columns are T's in the other order.
fn session(db: &Database) -> Session {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N CHAR(2))
           CREATE TABLE ARCHIVE (K INTEGER, N CHAR(2))
           CREATE VIEW V (NV, KV) AS SELECT N, K FROM T WHERE K > 0 WITH CASCADED CHECK OPTION
           CREATE VIEW L AS SELECT K, N FROM T WHERE K > 0",
    )
    .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 'A'), (2, 'B'), (3, 'C')").unwrap();
    s.commit().unwrap();
    s
}

/// The rows `sql` gives in `s`, each its values joined by commas.
fn rows(s: &Session, sql: &str) -> Result<Vec<String>, SqlError> {
    let mut c = s.open_cursor(sql)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch()? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.17 SR3d iv, GR1a] a delta table in
/// each WITH list element, their statements run in the order the elements
/// are written, then the query reads them as it reads any query name.
#[test]
fn delta_tables_in_with_list_elements_d189() {
    let db = Database::new();
    let s = session(&db);
    let sql = "WITH GONE AS (SELECT K FROM OLD TABLE (DELETE FROM T WHERE K = 1)), \
               MADE (K) AS (SELECT K FROM NEW TABLE (INSERT INTO T SELECT K + 10, N FROM T)) \
               SELECT 'GONE', K FROM GONE UNION ALL SELECT 'MADE', K FROM MADE ORDER BY 1, 2";
    assert_eq!(rows(&s, sql).unwrap(), vec!["GONE,1", "MADE,12", "MADE,13"], "the DELETE ran before the INSERT");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), vec!["4"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.11; §14.12] the delta table of an
/// INSERT's query, and a MERGE's source, run before it, in a routine too;
/// a delta table of a statement whose own query reads one gives its rows.
#[test]
fn delta_tables_in_insert_and_merge_d189() {
    let db = Database::new();
    let s = session(&db);
    s.run("INSERT INTO ARCHIVE SELECT K, N FROM OLD TABLE (DELETE FROM T WHERE K <= 2)").unwrap();
    assert_eq!(rows(&s, "SELECT K, N FROM ARCHIVE ORDER BY K").unwrap(), vec!["1,A ", "2,B "]);
    assert_eq!(rows(&s, "SELECT K FROM T").unwrap(), vec!["3"]);
    let merge = "MERGE INTO ARCHIVE AS R USING NEW TABLE (UPDATE T SET N = 'Z') AS D ON R.K = D.K \
                 WHEN NOT MATCHED THEN INSERT VALUES (D.K, D.N)";
    s.run(merge).unwrap();
    assert_eq!(rows(&s, "SELECT K, N FROM ARCHIVE ORDER BY K").unwrap(), vec!["1,A ", "2,B ", "3,Z "]);
    let nested = "SELECT K FROM NEW TABLE (INSERT INTO T SELECT K, N FROM OLD TABLE (DELETE FROM ARCHIVE WHERE K = 1))";
    assert_eq!(rows(&s, nested).unwrap(), vec!["1"]);
    s.run("CREATE PROCEDURE KEEP () MODIFIES SQL DATA INSERT INTO ARCHIVE SELECT K, N FROM OLD TABLE (DELETE FROM T WHERE K = 3)").unwrap();
    s.run("CALL KEEP ()").unwrap();
    assert_eq!(
        rows(&s, "SELECT COUNT(*) FROM ARCHIVE WHERE K = 3").unwrap(),
        vec!["2"],
        "a routine's INSERT runs its delta table's DELETE"
    );
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K").unwrap(), vec!["1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.5 SR3, §7.19 SR1, §11.32 SR7] a delta
/// table in a subquery, a derived table, beside another table reference,
/// or in a view's query is refused, and its statement does not run.
#[test]
fn delta_tables_elsewhere_are_refused_d189() {
    let db = Database::new();
    let s = session(&db);
    let refused = [
        "SELECT (SELECT COUNT(*) FROM OLD TABLE (DELETE FROM T)) FROM ARCHIVE",
        "SELECT K FROM T WHERE K IN (SELECT K FROM OLD TABLE (DELETE FROM T))",
        "SELECT K FROM (SELECT K FROM OLD TABLE (DELETE FROM T)) AS D",
        "SELECT D.K FROM OLD TABLE (DELETE FROM T) AS D, ARCHIVE",
    ];
    for sql in refused {
        assert_eq!(rows(&s, sql).unwrap_err().code, Code::SyntaxRule, "{sql}");
    }
    assert_eq!(s.run("CREATE VIEW D AS SELECT K FROM OLD TABLE (DELETE FROM T)").unwrap_err().code, Code::SyntaxRule);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), vec!["3"], "no DELETE ran");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR11b] a delta table of an updatable
/// view WITH CASCADED CHECK OPTION gives the rows its base table's change
/// made, as the view shows them; of a view without, it is refused.
#[test]
fn delta_tables_of_views_d189() {
    let db = Database::new();
    let s = session(&db);
    let old = rows(&s, "SELECT KV, NV FROM OLD TABLE (UPDATE V SET NV = 'X' WHERE KV = 2)").unwrap();
    assert_eq!(old, vec!["2,B "]);
    let fin = rows(&s, "SELECT * FROM FINAL TABLE (UPDATE V SET NV = 'Y' WHERE KV >= 2) ORDER BY KV").unwrap();
    assert_eq!(fin, vec!["Y ,2", "Y ,3"]);
    assert_eq!(rows(&s, "SELECT NV, KV FROM NEW TABLE (INSERT INTO V VALUES ('W', 4))").unwrap(), vec!["W ,4"]);
    assert_eq!(rows(&s, "SELECT KV FROM OLD TABLE (DELETE FROM V WHERE KV = 1)").unwrap(), vec!["1"]);
    assert_eq!(rows(&s, "SELECT K FROM OLD TABLE (DELETE FROM L)").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-2:2023 §14.9 GR4, §14.14 GR6] an UPDATE or
/// DELETE of a typed table named without ONLY changes its subtables' rows
/// too, and its delta table holds them, by their superrows; with ONLY, its
/// own rows alone.
#[test]
fn delta_tables_of_typed_tables_hold_subtable_rows_d189() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE PERSON AS (K INTEGER, AGE INTEGER) NOT FINAL REF IS SYSTEM GENERATED
           CREATE TYPE EMPLOYEE UNDER PERSON AS (PAY INTEGER) NOT FINAL",
    )
    .unwrap();
    let s = db.session("A");
    s.run("CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)").unwrap();
    s.run("CREATE TABLE STAFF OF EMPLOYEE UNDER PEOPLE").unwrap();
    s.run("INSERT INTO PEOPLE (K, AGE) VALUES (1, 30), (2, 40)").unwrap();
    s.run("INSERT INTO STAFF (K, AGE, PAY) VALUES (3, 50, 100)").unwrap();
    s.commit().unwrap();
    let new = rows(&s, "SELECT K, AGE FROM NEW TABLE (UPDATE PEOPLE SET AGE = AGE + 1 WHERE AGE >= 40) ORDER BY K").unwrap();
    assert_eq!(new, vec!["2,41", "3,51"]);
    let only = rows(&s, "SELECT K FROM NEW TABLE (UPDATE ONLY (PEOPLE) SET AGE = 0 WHERE K >= 2) ORDER BY K").unwrap();
    assert_eq!(only, vec!["2"]);
    let old = rows(&s, "SELECT K, AGE FROM OLD TABLE (DELETE FROM PEOPLE WHERE K >= 3)").unwrap();
    assert_eq!(old, vec!["3,51"]);
}
