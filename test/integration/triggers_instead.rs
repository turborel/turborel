//! INSTEAD OF triggers on views [ISO/IEC 9075-2:2023 §11.49, §15.10,
//! §15.13, §15.16] (Feature T213): what the statement would change, given
//! to the trigger, nothing written through the view, and the privileges
//! the trigger gives its schema's owner [§11.49 GR6, §11.50 GR1].

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlState};

/// Schema A: T (K, N); V, the grouped, read-only view of T's rows by K,
/// SUM of N; L, a log.
fn viewed() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, N INTEGER)
           CREATE VIEW V (K, N) AS SELECT K, SUM(N) FROM T GROUP BY K
           CREATE TABLE L (WHAT VARCHAR(10), K INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (1, 5), (2, 20)").unwrap();
    a.commit().unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.10] an INSERT into a read-only view
/// with an INSTEAD OF INSERT trigger runs the trigger for each row, a
/// column not named null, and writes no row through the view.
#[test]
fn an_insert_into_a_view_runs_its_trigger() {
    let db = viewed();
    let a = db.session("A");
    a.run("CREATE TRIGGER INTO_V INSTEAD OF INSERT ON V REFERENCING NEW AS NR FOR EACH ROW INSERT INTO T VALUES (NR.K, COALESCE(NR.N, 0))")
        .unwrap();
    a.run("INSERT INTO V (K) VALUES (3)").unwrap();
    a.run("INSERT INTO V VALUES (4, 40)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM V ORDER BY K"), ["1,15", "2,20", "3,0", "4,40"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.13] an UPDATE of a view with an
/// INSTEAD OF UPDATE trigger gives it each row the WHERE clause chooses,
/// before and after its SET clauses.
#[test]
fn an_update_of_a_view_runs_its_trigger() {
    let db = viewed();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER ON_V INSTEAD OF UPDATE ON V REFERENCING OLD AS O NEW AS NW FOR EACH ROW
           INSERT INTO L VALUES ('UPD', O.N, NW.N)",
    )
    .unwrap();
    a.run("UPDATE V SET N = N + 1 WHERE K = 1").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, N FROM L"), ["UPD,15,16"]);
    assert_eq!(rows(&a, "SELECT K, N FROM V ORDER BY K"), ["1,15", "2,20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.16] a DELETE of a view with an
/// INSTEAD OF DELETE statement trigger gives it the rows chosen as its old
/// transition table.
#[test]
fn a_delete_of_a_view_runs_its_trigger() {
    let db = viewed();
    let a = db.session("A");
    a.run("CREATE TRIGGER OFF_V INSTEAD OF DELETE ON V REFERENCING OLD TABLE AS GONE DELETE FROM T WHERE K IN (SELECT K FROM GONE)")
        .unwrap();
    a.run("DELETE FROM V WHERE N > 16").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM T"), ["1,10", "1,5"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR5a, SR15a, SR15b, SR5b] an
/// INSTEAD OF trigger is on a view, one for each event, with no WHEN
/// condition or column list; a BEFORE or AFTER trigger is on a base table.
#[test]
fn instead_of_triggers_are_on_views_alone() {
    let db = viewed();
    let a = db.session("A");
    let refused = |sql: &str| a.run(sql).unwrap_err();
    assert_eq!(refused("CREATE TRIGGER X INSTEAD OF DELETE ON T DELETE FROM L").code, Code::SyntaxRule);
    assert_eq!(refused("CREATE TRIGGER X AFTER DELETE ON V DELETE FROM L").code, Code::SyntaxRule);
    a.run("CREATE TRIGGER X INSTEAD OF DELETE ON V DELETE FROM L").unwrap();
    assert_eq!(refused("CREATE TRIGGER Y INSTEAD OF DELETE ON V DELETE FROM L").code, Code::SyntaxRule);
    assert_eq!(refused("CREATE TRIGGER Y INSTEAD OF UPDATE OF N ON V DELETE FROM L").code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 GR6, §11.50 GR1] an INSTEAD OF
/// trigger gives its schema's owner the privilege of its event on the
/// view, grantable, so the owner may grant it; dropping the trigger takes
/// it, and what was granted of it, away.
#[test]
fn an_instead_of_trigger_gives_its_privileges() {
    let db = viewed();
    let a = db.session("A");
    assert_eq!(a.run("GRANT DELETE ON V TO B").unwrap(), Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED));
    a.run("CREATE TRIGGER X INSTEAD OF DELETE ON V DELETE FROM L").unwrap();
    a.run("GRANT DELETE ON V TO B").unwrap();
    a.commit().unwrap();
    let held = "SELECT GRANTOR, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE TABLE_NAME = 'V' AND PRIVILEGE_TYPE = 'DELETE' ORDER BY GRANTOR";
    assert_eq!(rows(&a, held), ["A,DELETE,NO", "_SYSTEM,DELETE,YES"]);
    let b = db.session("B");
    b.run("DELETE FROM A.V").unwrap();
    b.commit().unwrap();
    a.run("DROP TRIGGER X").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, held), Vec::<String>::new());
    assert_eq!(b.run("DELETE FROM A.V").unwrap_err().code, Code::SyntaxRule);
}
