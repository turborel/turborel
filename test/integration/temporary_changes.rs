//! Changes to a temporary table's definition while sessions hold rows in
//! it (RULES.md D49): ALTER TABLE [ISO/IEC 9075:1992 §11.10] adding or
//! dropping a column of each session's instance [§11.11 GR2, §11.15 GR4],
//! and CREATE INDEX and DROP INDEX, whose entries each session makes of
//! its own rows.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{code, exec, rows, sorted};

/// Schema A: L, created local temporary, ON COMMIT PRESERVE ROWS, keyed by
/// K; G, global temporary, ON COMMIT DELETE ROWS; P, persistent.
fn temporary() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE LOCAL TEMPORARY TABLE L (K INTEGER NOT NULL UNIQUE, V CHAR(4)) ON COMMIT PRESERVE ROWS
           CREATE GLOBAL TEMPORARY TABLE G (K INTEGER)
           CREATE TABLE P (K INTEGER)",
    )
    .unwrap();
    db
}

/// Its rows of L, K and its other columns, in order of K.
fn of_l(s: &Session) -> Vec<String> {
    rows(s, "SELECT * FROM L ORDER BY K").unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR2, §11.15 GR4];
/// [DESIGN] RULES.md D49: a column added to a temporary table is added to
/// each session's rows, every value of it its default, the altering
/// session's own and another's; a column dropped goes from them; while the
/// transaction altering it is open, no other reads or writes an instance
/// of it (SQLSTATE TB911); its UNIQUE constraint still holds of each
/// session's rows; a rollback leaves the rows as they were.
#[test]
fn a_column_is_added_to_and_dropped_from_each_sessions_rows_11_11() {
    let db = temporary();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1, 'a'), (2, 'b')").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO L VALUES (5, 'e')").unwrap();
    b.commit().unwrap();
    b.run("ALTER TABLE L ADD COLUMN M INTEGER DEFAULT 7").unwrap();
    assert_eq!(of_l(&b), ["5,e   ,7"], "its own rows, in its transaction");
    assert_eq!(code(&a, "SELECT K FROM L"), Code::Conflict, "being altered");
    a.rollback().unwrap();
    b.run("ALTER TABLE L ADD COLUMN N CHAR(2) DEFAULT 'n'").unwrap();
    b.commit().unwrap();
    assert_eq!(of_l(&a), ["1,a   ,7,n ", "2,b   ,7,n "], "another's rows, of the new definition");
    assert_eq!(exec(&a, "INSERT INTO L (K) VALUES (1)").err(), Some(Code::Constraint), "UNIQUE of its rows");
    a.run("INSERT INTO L (K) VALUES (3)").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE L DROP COLUMN V RESTRICT").unwrap();
    a.rollback().unwrap();
    assert_eq!(of_l(&b), ["5,e   ,7,n "], "rolled back");
    b.commit().unwrap();
    a.run("ALTER TABLE L DROP COLUMN V RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(of_l(&a), ["1,7,n ", "2,7,n ", "3,7,n "]);
    assert_eq!(of_l(&b), ["5,7,n "]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR2, GR4]; [DESIGN]
/// RULES.md D49: the constraints of a column added hold of each session's
/// rows, its own and another's, or the ALTER TABLE is refused (SQLCODE
/// -201): a NOT NULL column whose default is null, a CHECK its default
/// breaks; rows a session's transaction has open, of a table whose rows a
/// commit deletes, are its own to alter.
#[test]
fn a_column_added_is_checked_of_each_sessions_rows_11_11() {
    let db = temporary();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1, 'a')").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD COLUMN M INTEGER NOT NULL").err(), Some(Code::Constraint), "another's row");
    b.rollback().unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD COLUMN M INTEGER DEFAULT 5 CHECK (M < 5)").err(), Some(Code::Constraint));
    b.rollback().unwrap();
    b.run("INSERT INTO G VALUES (1)").unwrap();
    assert_eq!(exec(&b, "ALTER TABLE G ADD COLUMN M INTEGER CHECK (M > 0)").err(), None, "null is not false");
    assert_eq!(exec(&b, "ALTER TABLE G ADD COLUMN N INTEGER NOT NULL").err(), Some(Code::Constraint), "its own row");
    assert_eq!(rows(&b, "SELECT * FROM G"), Ok(vec!["1,NULL".into()]), "the statement undone");
    b.commit().unwrap();
    assert_eq!(rows(&b, "SELECT * FROM G"), Ok(vec![]), "emptied by the commit");
    b.run("ALTER TABLE G ADD UNIQUE (K)").unwrap();
    b.run("INSERT INTO G VALUES (1, 1)").unwrap();
    assert_eq!(exec(&b, "INSERT INTO G VALUES (1, 2)").err(), Some(Code::Constraint), "the UNIQUE added, in its transaction");
    b.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D49: an ALTER TABLE of a temporary table is
/// refused (SQLCODE -911) while another session's transaction has its rows
/// open; any other than adding or dropping a column, while a session's
/// instance holds a row; and adding or dropping a column while another
/// session holds rows and a constraint of the table reads another table,
/// which only that session's own instance gives.
#[test]
fn what_altering_a_temporary_table_waits_for_11_10() {
    let db = temporary();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION R
           CREATE GLOBAL TEMPORARY TABLE PK (K INTEGER UNIQUE) ON COMMIT PRESERVE ROWS
           CREATE GLOBAL TEMPORARY TABLE FK (K INTEGER REFERENCES PK (K)) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1, 'a')").unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD COLUMN M INTEGER").err(), Some(Code::Conflict), "A's transaction open");
    b.rollback().unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD CHECK (K > 0)").err(), Some(Code::Conflict), "A's row, kept");
    b.rollback().unwrap();
    let (r, other) = (db.session("R"), db.session("R"));
    r.run("INSERT INTO PK VALUES (1)").unwrap();
    r.run("INSERT INTO FK VALUES (1)").unwrap();
    r.commit().unwrap();
    assert_eq!(exec(&other, "ALTER TABLE FK ADD COLUMN M INTEGER").err(), Some(Code::Conflict), "its foreign key");
    other.rollback().unwrap();
    r.run("ALTER TABLE FK ADD COLUMN M INTEGER DEFAULT 2").unwrap();
    r.commit().unwrap();
    assert_eq!(rows(&r, "SELECT * FROM FK"), Ok(vec!["1,2".into()]), "its own rows, checked in its own session");
}

/// Verifies: [DESIGN] RULES.md D49, [ISO/IEC 9075:1992 §11.11 GR2]: an
/// ALTER TABLE refused by one session's rows, after another's were
/// copied, leaves no copy behind: a later ALTER TABLE of the same
/// transaction copies each session's rows as they are. Regression test
/// (R-TEST-7): the copies of the refused statement, emptied by its undo,
/// were taken as the rows of the session whose copy succeeded, which lost
/// them.
#[test]
fn a_refused_alter_leaves_no_copy_11_11() {
    let db = temporary();
    let (a, c, b) = (db.session("A"), db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (0, 'a')").unwrap();
    a.commit().unwrap();
    c.run("INSERT INTO L VALUES (5, 'c')").unwrap();
    c.commit().unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD COLUMN M INTEGER DEFAULT 1 CHECK (M > K)").err(), Some(Code::Constraint), "C's row");
    b.run("ALTER TABLE L ADD COLUMN N INTEGER DEFAULT 2").unwrap();
    b.commit().unwrap();
    assert_eq!(of_l(&a), ["0,a   ,2"]);
    assert_eq!(of_l(&c), ["5,c   ,2"]);
}

/// Verifies: [DESIGN] RULES.md D49: a transaction that began before an
/// ALTER TABLE of a temporary table committed reads its rows no more, its
/// snapshot not seeing them copied (SQLSTATE TB911); run again, it does.
#[test]
fn a_transaction_begun_before_reads_no_altered_rows_11_10() {
    let db = temporary();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1, 'a')").unwrap();
    a.commit().unwrap();
    rows(&a, "SELECT K FROM P").unwrap();
    b.run("ALTER TABLE L ADD COLUMN M INTEGER DEFAULT 3").unwrap();
    b.commit().unwrap();
    assert_eq!(code(&a, "SELECT K FROM L"), Code::Conflict);
    a.rollback().unwrap();
    assert_eq!(of_l(&a), ["1,a   ,3"]);
}

/// Statements of `s` that find K by V, and V by K, each in order.
fn by_index(s: &Session) -> (Vec<String>, Vec<String>) {
    (sorted(s, "SELECT K FROM L WHERE V = 'b'"), sorted(s, "SELECT V FROM L WHERE K = 2"))
}

/// Verifies: [EXTENSION] RULES.md D34, D49: CREATE INDEX on a temporary
/// table, of rows sessions hold, gives each session's instance entries of
/// its own rows, as a session made later, which the planner reads; a
/// session whose transaction had its rows open uses the index only once
/// it ends (SQLSTATE TB911); DROP INDEX takes the entries away, and an
/// index made again of other columns has entries of its own; a method not
/// built in is refused (SQLCODE -103).
#[test]
fn an_index_on_a_temporary_table_is_each_sessions_own() {
    let db = temporary();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1, 'a'), (2, 'b')").unwrap();
    for k in 10_i32..60_i32 {
        a.run(&format!("INSERT INTO L VALUES ({k}, 'x')")).unwrap();
    }
    a.run("UPDATE L SET V = 'b' WHERE K = 1").unwrap();
    a.run("UPDATE L SET V = 'a' WHERE K = 1").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO L VALUES (3, 'b')").unwrap();
    a.run("CREATE INDEX L_V ON L (V)").unwrap();
    assert!(a.explain("SELECT K FROM L WHERE V = 'b'", &Params::new()).unwrap().contains("IndexScan"));
    assert_eq!(by_index(&a), (vec!["2".to_owned()], vec!["b   ".to_owned()]));
    assert_eq!(exec(&b, "INSERT INTO L VALUES (4, 'b')").err(), Some(Code::Conflict), "its rows were open");
    b.commit().unwrap();
    assert_eq!(sorted(&b, "SELECT K FROM L WHERE V = 'b'"), ["3"]);
    let c = db.session("A");
    c.run("INSERT INTO L VALUES (2, 'b')").unwrap();
    assert_eq!(by_index(&c), (vec!["2".to_owned()], vec!["b   ".to_owned()]));
    c.commit().unwrap();
    a.commit().unwrap();
    a.run("DROP INDEX L_V").unwrap();
    a.run("CREATE INDEX L_V ON L (K) USING hash").unwrap();
    assert_eq!(by_index(&a), (vec!["2".to_owned()], vec!["b   ".to_owned()]));
    assert_eq!(sorted(&c, "SELECT V FROM L WHERE K = 2"), ["b   "]);
    a.commit().unwrap();
    assert_eq!(exec(&a, "CREATE INDEX L_X ON L (V) USING nope").err(), Some(Code::SyntaxRule));
}
