//! \[EXTENSION\] SQL-schema statements of ISO/IEC 9075:1992 in transactions
//! (RULES.md D33): ALTER TABLE [§11.10] and DROP TABLE [§11.18],
//! their changes their transaction's until it commits, and gone if it rolls
//! back.

use turborel::db::{Completion, Database, Session};
use turborel::error::Code;

use crate::common::{code, count, exec, rows, setup, with_views};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28, §11.18 GR2] a table
/// dropped is gone for its transaction at once, and for others when it
/// commits; meanwhile others read it as before, and its exclusive lock
/// refuses their writes (no-wait); a ROLLBACK brings it back; a change of
/// data in the same transaction commits with the drop.
#[test]
fn a_drop_is_its_transactions_until_it_commits() {
    let db = setup();
    let (s, other) = (db.session("APP"), db.session("APP"));
    s.run("INSERT INTO T (ID) VALUES (7)").unwrap();
    s.run("DROP TABLE U RESTRICT").unwrap();
    assert_eq!(code(&s, "SELECT ID FROM U"), Code::SyntaxRule, "gone for its transaction");
    assert_eq!(count(&other, "SELECT ID FROM U"), 2, "not for another");
    assert_eq!(exec(&other, "DELETE FROM U").map(drop), Err(Code::Conflict), "its writes wait for the drop");
    other.rollback().unwrap();
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT ID FROM U"), 2, "rolled back");
    assert_eq!(count(&s, "SELECT ID FROM T WHERE ID = 7"), 0);
    s.run("INSERT INTO T (ID) VALUES (8)").unwrap();
    s.run("DROP TABLE U CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(code(&other, "SELECT ID FROM U"), Code::SyntaxRule, "gone for all");
    assert_eq!(count(&other, "SELECT ID FROM T WHERE ID = 8"), 1, "committed with it");
    assert_eq!(db.index_entries("APP.U.UNIQUE0").map_err(|e| e.code), Err(Code::Api), "its constraint's index gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 SR4, GR1] RESTRICT
/// refuses a table a view reads; CASCADE drops it with the views reading
/// it, directly or through another; a view over another table stays; its
/// index goes with it.
#[test]
fn restrict_and_cascade_11_18() {
    let db = with_views();
    let s = db.session("D");
    assert_eq!(exec(&s, "DROP TABLE T RESTRICT").err(), Some(Code::SyntaxRule), "V reads it");
    s.run("DROP TABLE T CASCADE").unwrap();
    s.commit().unwrap();
    for gone in ["SELECT K FROM T", "SELECT K FROM V", "SELECT K FROM W"] {
        assert_eq!(code(&s, gone), Code::SyntaxRule, "{gone}");
    }
    assert_eq!(count(&s, "SELECT K FROM X"), 3, "X reads U");
    assert_eq!(db.index_entries("T_N").map_err(|e| e.code), Err(Code::Api), "its index gone");
    s.run("DROP TABLE U CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(code(&s, "SELECT K FROM X"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 SR1, SR2, AR1]; [DESIGN]
/// RULES.md D33: refused, the transaction going on: a table not defined, a
/// view, another owner's table, and one a cursor of the transaction reads;
/// a DROP without its behavior is not a statement [§11.18 Format].
#[test]
fn what_drop_table_refuses_11_18() {
    let db = with_views();
    let s = db.session("D");
    assert_eq!(exec(&s, "DROP TABLE NOPE CASCADE").err(), Some(Code::SyntaxRule), "SR1");
    assert_eq!(exec(&s, "DROP TABLE V CASCADE").err(), Some(Code::SyntaxRule), "SR2");
    assert_eq!(exec(&s, "DROP TABLE U").err(), Some(Code::Syntax), "no <drop behavior>");
    assert_eq!(exec(&db.session("P"), "DROP TABLE D.T CASCADE").err(), Some(Code::Privilege), "AR1");
    let mut cursor = s.open_cursor("SELECT K FROM U").unwrap();
    assert_eq!(exec(&s, "DROP TABLE U CASCADE").err(), Some(Code::Cursor), "U is read by a cursor");
    assert!(cursor.fetch().unwrap().is_some(), "the cursor goes on");
    s.commit().unwrap();
    assert_eq!(count(&s, "SELECT K FROM U"), 3);
}

/// Verifies: [DESIGN] RULES.md D33, D27: definitions another
/// publishes while a transaction has dropped a table are seen by its next
/// statement, with its own drop: a row it inserts after an index is
/// published has its entry; both stand once it commits. An index is not
/// created on a table being dropped (no-wait: SQLCODE -911).
#[test]
fn a_drop_sees_what_others_publish_meanwhile() {
    let db = with_views();
    let s = db.session("D");
    s.run("DROP TABLE U CASCADE").unwrap();
    assert_eq!(db.create_index("U_K", "D.U", &["K"], "btree").map_err(|e| e.code), Err(Code::Conflict));
    db.create_index("T_K2", "D.T", &["K"], "hash").unwrap();
    s.run("INSERT INTO T VALUES (4, 'N4')").unwrap();
    assert_eq!(code(&s, "SELECT K FROM U"), Code::SyntaxRule, "its own drop kept");
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_K2").unwrap(), 4, "the row inserted after it has its entry");
    assert_eq!(rows(&db.session("D"), "SELECT N FROM T WHERE K = 2"), Ok(vec!["N2  ".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR1, GR2, GR4] a column
/// added is the table's last, null in every row; another transaction sees
/// it when the change commits, and one that began before is refused at its
/// next read of the table (SQLCODE -911); rows keep their index entries; a
/// view of all of the table keeps its columns [GR2, note]; a ROLLBACK
/// takes the column away.
#[test]
fn a_column_added_11_11() {
    let db = with_views();
    let (s, other, earlier) = (db.session("D"), db.session("D"), db.session("D"));
    assert_eq!(count(&earlier, "SELECT K FROM T"), 3);
    s.run("ALTER TABLE T ADD COLUMN C INTEGER").unwrap();
    assert_eq!(rows(&s, "SELECT K, C FROM T WHERE K = 2"), Ok(vec!["2,NULL".into()]));
    s.run("INSERT INTO T VALUES (4, 'N4', 40)").unwrap();
    assert_eq!(code(&other, "SELECT C FROM T"), Code::SyntaxRule, "not yet for another");
    other.commit().unwrap();
    s.rollback().unwrap();
    assert_eq!(code(&s, "SELECT C FROM T"), Code::SyntaxRule, "rolled back");
    assert_eq!(count(&s, "SELECT K FROM T"), 3);
    s.commit().unwrap();
    s.run("ALTER TABLE T ADD C INTEGER").unwrap();
    s.run("INSERT INTO T VALUES (4, 'N4', 40)").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&other, "SELECT K, C FROM T WHERE C IS NOT NULL"), Ok(vec!["4,40".into()]));
    assert_eq!(rows(&other, "SELECT K FROM T WHERE N = 'N3'"), Ok(vec!["3".into()]), "through its index");
    assert_eq!(db.index_entries("T_N").unwrap(), 4);
    assert_eq!(rows(&other, "SELECT * FROM S WHERE K = 4"), Ok(vec!["4,N4  ".into()]), "S keeps its two columns");
    assert_eq!(code(&earlier, "SELECT K FROM T"), Code::Conflict, "began before the change");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.4 SR1, §11.11 GR2] refused:
/// a column of a name the table has, and one NOT NULL in a table that has
/// rows, which would be null there (SQLCODE -201); NOT NULL UNIQUE in an
/// empty table is enforced as any UNIQUE constraint [X3.135-1986 §6.4].
#[test]
fn what_add_column_refuses_11_11() {
    let db = with_views();
    let s = db.session("D");
    assert_eq!(exec(&s, "ALTER TABLE T ADD N INTEGER").err(), Some(Code::SyntaxRule), "N exists");
    assert_eq!(exec(&s, "ALTER TABLE T ADD C INTEGER NOT NULL").err(), Some(Code::Constraint), "T has rows");
    assert_eq!(exec(&s, "ALTER TABLE V ADD C INTEGER").err(), Some(Code::SyntaxRule), "a view");
    s.run("DELETE FROM U").unwrap();
    s.run("ALTER TABLE U ADD C INTEGER NOT NULL UNIQUE").unwrap();
    s.run("INSERT INTO U VALUES (1, 7)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO U VALUES (2, 7)").err(), Some(Code::Constraint), "UNIQUE");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT K, C FROM U"), Ok(vec!["1,7".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.15 SR3, SR4, GR2, GR4,
/// GR6] a column dropped: its values with it, the columns after it one
/// place on, their indexes with them; its own UNIQUE constraint with it; RESTRICT
/// refused while a view refers to it, CASCADE dropping those views and
/// those over them; not the only column.
#[test]
fn a_column_dropped_11_15() {
    let db = with_views();
    let s = db.session("D");
    assert_eq!(exec(&s, "ALTER TABLE T DROP COLUMN N RESTRICT").err(), Some(Code::SyntaxRule), "S selects all of T");
    assert_eq!(exec(&s, "ALTER TABLE T DROP K RESTRICT").err(), Some(Code::SyntaxRule), "V refers to K");
    assert_eq!(exec(&s, "ALTER TABLE U DROP COLUMN K CASCADE").err(), Some(Code::SyntaxRule), "U's only column");
    s.run("ALTER TABLE T DROP COLUMN K CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT * FROM T ORDER BY N"), Ok(vec!["N1  ".into(), "N2  ".into(), "N3  ".into()]));
    for gone in ["SELECT K FROM V", "SELECT K FROM W", "SELECT * FROM S"] {
        assert_eq!(code(&s, gone), Code::SyntaxRule, "{gone}");
    }
    assert_eq!(count(&s, "SELECT K FROM X"), 3, "X reads U");
    assert_eq!(rows(&s, "SELECT N FROM T WHERE N = 'N2'"), Ok(vec!["N2  ".into()]), "T_N, moved");
    s.run("INSERT INTO T VALUES ('N2')").unwrap();
    assert_eq!(count(&s, "SELECT N FROM T"), 4, "K's UNIQUE constraint went with it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.10, §11.11 GR2, GR3,
/// §11.15 GR1, GR6]: a view of all of a table through a correlation name
/// keeps its columns when one is added; an updatable view's columns move
/// with the table's, so writes through it go on; UPDATE of every column
/// extends to a column added, and UPDATE of a column dropped is revoked; a
/// view whose subquery takes all of a table it compares with keeps taking
/// the columns it took.
#[test]
fn views_and_privileges_follow_their_tables_11_10() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION E
           CREATE TABLE T (K INTEGER NOT NULL, N CHAR(4), M INTEGER)
           CREATE TABLE U (K INTEGER)
           CREATE VIEW A AS SELECT * FROM E.T Q WHERE Q.K > 0
           CREATE VIEW B AS SELECT N, M FROM T
           CREATE VIEW Z AS SELECT K FROM U WHERE K IN (SELECT * FROM U)
           GRANT UPDATE ON T TO P
           GRANT UPDATE (K, M) ON T TO R",
    )
    .unwrap();
    let s = db.session("E");
    s.run("INSERT INTO T VALUES (1, 'A', 10)").unwrap();
    s.run("ALTER TABLE T ADD COLUMN C INTEGER").unwrap();
    assert_eq!(rows(&s, "SELECT * FROM A"), Ok(vec!["1,A   ,10".into()]), "A keeps its three columns");
    s.run("ALTER TABLE T DROP COLUMN K RESTRICT").unwrap_err();
    s.run("ALTER TABLE T DROP COLUMN K CASCADE").unwrap();
    s.run("UPDATE B SET M = 11").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT N, M, C FROM T"), Ok(vec!["A   ,11,NULL".into()]));
    assert_eq!(code(&s, "SELECT * FROM A"), Code::SyntaxRule, "A refers to K, dropped");
    assert_eq!(exec(&db.session("P"), "UPDATE E.T SET C = 1").err(), None, "UPDATE of every column covers C");
    assert_eq!(exec(&db.session("R"), "UPDATE E.T SET M = 2").err(), None, "M moved, still granted");
    assert_eq!(exec(&db.session("R"), "UPDATE E.T SET C = 2").err(), Some(Code::Privilege), "not C");
    s.run("INSERT INTO U VALUES (7)").unwrap();
    assert_eq!(exec(&s, "ALTER TABLE U ADD COLUMN C INTEGER").err(), None, "Z's subquery takes U's K alone");
    assert_eq!(rows(&s, "SELECT K FROM Z"), Ok(vec!["7".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR2 note, §11.15 SR4]: a
/// view's implicit column references are those of the columns its tables
/// had when it was defined, so a column added changes none: a NATURAL
/// join's common columns, an unqualified name's table, CORRESPONDING's
/// columns, the * of a joined or derived table. A column it saw by * is
/// one it refers to, which RESTRICT does not drop; one added after it, it
/// does not see.
#[test]
fn views_keep_their_implicit_columns_11_11() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE T (K INTEGER, N CHAR(2))
           CREATE TABLE U (K INTEGER, X INTEGER)
           CREATE VIEW NAT AS SELECT * FROM T NATURAL JOIN U
           CREATE VIEW NAMED AS SELECT N, X FROM T, U WHERE T.K = U.K
           CREATE VIEW CORR AS SELECT * FROM T UNION CORRESPONDING SELECT * FROM U
           CREATE VIEW JOINED (K1, N, K2, X) AS SELECT * FROM T JOIN U ON T.K = U.K
           CREATE VIEW DERIVED AS SELECT * FROM (SELECT * FROM U) AS D
           CREATE VIEW STAR AS SELECT * FROM T",
    )
    .unwrap();
    let s = db.session("H");
    s.run("INSERT INTO T VALUES (1, 'A'), (2, 'B')").unwrap();
    s.run("INSERT INTO U VALUES (1, 10), (2, 20)").unwrap();
    let all = |s: &Session| {
        ["NAT", "NAMED", "CORR", "JOINED", "DERIVED", "STAR"].map(|v| {
            let mut found = rows(s, &format!("SELECT * FROM {v}")).unwrap();
            found.sort();
            found
        })
    };
    let before = all(&s);
    s.run("ALTER TABLE U ADD COLUMN N CHAR(2)").unwrap();
    s.run("ALTER TABLE T ADD COLUMN X INTEGER").unwrap();
    s.run("UPDATE U SET N = 'Z'").unwrap();
    assert_eq!(all(&s), before, "N and X, now of both, are none of the views'");
    assert_eq!(before[0], vec!["1,A ,10", "2,B ,20"], "NAT joins on K alone");
    assert_eq!(exec(&s, "ALTER TABLE T DROP COLUMN X RESTRICT"), Ok(Completion::Success), "no view saw X");
    assert_eq!(exec(&s, "ALTER TABLE T DROP COLUMN N RESTRICT"), Err(Code::SyntaxRule), "STAR saw N");
    assert_eq!(exec(&s, "ALTER TABLE T DROP COLUMN N CASCADE"), Ok(Completion::Success));
    assert_eq!(code(&s, "SELECT * FROM STAR"), Code::SyntaxRule, "dropped with N");
}
