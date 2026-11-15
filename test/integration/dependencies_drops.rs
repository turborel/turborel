//! What depends on a table constraint by a functional dependency
//! [ISO/IEC 9075-2:2023 §11.26 SR4 to SR7, SR13, GR1, GR3, GR4, Feature
//! T301]: the views, CHECKs, domain constraints and assertions whose
//! grouped queries read a column the constraint makes dependent on their
//! grouping columns, refused under RESTRICT and dropped under CASCADE.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N, J), its key KEYED of K and J; K is NOT NULL by
/// K_SET, so its UNIQUE U_K is a key too.
fn keyed() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER CONSTRAINT K_SET NOT NULL, N INTEGER, J INTEGER,
             CONSTRAINT KEYED PRIMARY KEY (K, J), CONSTRAINT U_K UNIQUE (K))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10, 0), (2, 20, 0)").unwrap();
    a.commit().unwrap();
    db
}

/// Asserts `sql` is refused in `a` with SQLCODE -103, citing `cited`.
fn refused(a: &Session, sql: &str, cited: &str) {
    let e = a.run(sql).unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e}");
    assert!(e.to_string().contains(cited), "{e}");
}

/// Whether `query` reads in `a`: its tables are there and it binds.
fn reads(a: &Session, query: &str) -> bool {
    a.open_cursor(query).is_ok()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR4, SR5, SR13c, GR3]
/// a view whose grouped query reads a column only a UNIQUE makes
/// dependent on its grouping column depends on the UNIQUE: RESTRICT is
/// refused, and CASCADE drops it with the views over it. A view that
/// needs no constraint, or another key that still holds, is kept.
#[test]
fn a_view_that_needs_a_key_is_dropped_with_it() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K, N FROM T GROUP BY K").unwrap();
    a.run("CREATE VIEW W AS SELECT N FROM V").unwrap();
    a.run("CREATE VIEW KEPT AS SELECT K, N, J FROM T GROUP BY K, J").unwrap();
    a.run("CREATE VIEW PLAIN AS SELECT K, COUNT(*) AS C FROM T GROUP BY K").unwrap();
    a.commit().unwrap();
    refused(&a, "ALTER TABLE T DROP CONSTRAINT U_K RESTRICT", "§11.26 SR13c");
    a.run("ALTER TABLE T DROP CONSTRAINT U_K CASCADE").unwrap();
    a.commit().unwrap();
    assert!(!reads(&a, "SELECT * FROM V"));
    assert!(!reads(&a, "SELECT * FROM W"));
    assert!(reads(&a, "SELECT * FROM KEPT"));
    assert!(reads(&a, "SELECT * FROM PLAIN"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR4, SR13c, §4.26.3]
/// a NOT NULL that makes a UNIQUE's column a key is a constraint a view
/// depends on: dropping it, RESTRICT, is refused while the view needs it.
#[test]
fn a_view_that_needs_a_not_null_depends_on_it() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER CONSTRAINT K_SET NOT NULL, N INTEGER, UNIQUE (K))").unwrap();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K, N FROM T GROUP BY K").unwrap();
    a.commit().unwrap();
    refused(&a, "ALTER TABLE T DROP CONSTRAINT K_SET RESTRICT", "§11.26 SR13c");
    a.run("DROP VIEW V RESTRICT").unwrap();
    a.run("ALTER TABLE T DROP CONSTRAINT K_SET RESTRICT").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR7, SR13a, GR1] a
/// CHECK of another table whose subquery needs the key depends on it:
/// RESTRICT is refused, and CASCADE drops the CHECK alone, its table kept.
#[test]
fn a_check_that_needs_a_key_is_dropped_with_it() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE TABLE C (X INTEGER, CONSTRAINT IN_T CHECK (X IN (SELECT N FROM T GROUP BY K)))").unwrap();
    a.run("INSERT INTO C VALUES (10)").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("INSERT INTO C VALUES (99)").unwrap_err().code, Code::Constraint);
    refused(&a, "ALTER TABLE T DROP CONSTRAINT U_K RESTRICT", "§11.26 SR13a");
    a.run("ALTER TABLE T DROP CONSTRAINT U_K CASCADE").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO C VALUES (99)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR7, SR13e, GR4] an
/// assertion whose query needs the key depends on it: RESTRICT is
/// refused, and CASCADE drops the assertion.
#[test]
fn an_assertion_that_needs_a_key_is_dropped_with_it() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE ASSERTION POSITIVE CHECK (NOT EXISTS (SELECT K FROM T GROUP BY K HAVING N < 0))").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("INSERT INTO T VALUES (3, -1, 0)").unwrap_err().code, Code::Constraint);
    refused(&a, "ALTER TABLE T DROP CONSTRAINT U_K RESTRICT", "§11.26 SR13e");
    a.run("ALTER TABLE T DROP CONSTRAINT U_K CASCADE").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T VALUES (3, -1, 0)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR7, SR13e, NOTE 568,
/// §11.39] a domain constraint whose query needs the key depends on it:
/// RESTRICT is refused, and CASCADE drops the constraint from the domain,
/// the domain and its columns kept.
#[test]
fn a_domain_constraint_that_needs_a_key_is_dropped_with_it() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE DOMAIN LISTED AS INTEGER CONSTRAINT IN_T CHECK (VALUE IN (SELECT N FROM T GROUP BY K))").unwrap();
    a.run("CREATE TABLE C (X LISTED)").unwrap();
    a.commit().unwrap();
    assert_eq!(a.run("INSERT INTO C VALUES (99)").unwrap_err().code, Code::Constraint);
    refused(&a, "ALTER TABLE T DROP CONSTRAINT U_K RESTRICT", "§11.26 SR13e");
    a.run("ALTER TABLE T DROP CONSTRAINT U_K CASCADE").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO C VALUES (99)").unwrap();
    assert!(reads(&a, "SELECT X FROM C"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR13] a constraint
/// nothing depends on is dropped with RESTRICT, though grouped views read
/// its table.
#[test]
fn a_constraint_nothing_needs_is_dropped_with_restrict() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K, N FROM T GROUP BY K").unwrap();
    a.run("ALTER TABLE T DROP CONSTRAINT KEYED RESTRICT").unwrap();
    a.commit().unwrap();
    assert!(reads(&a, "SELECT * FROM V"));
}
