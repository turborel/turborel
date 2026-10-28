//! Temporary tables [ISO/IEC 9075:1992 §4.9, §11.3, §14.3 GR3]: each
//! session's own rows of a table defined in a schema, what a commit does
//! to them, and the rules temporary tables keep.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::{count, exec, sorted};

/// Schema A: G, global temporary, ON COMMIT DELETE ROWS implicit; L,
/// created local temporary, ON COMMIT PRESERVE ROWS; both keyed by K.
fn temporary() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE GLOBAL TEMPORARY TABLE G (K INTEGER NOT NULL UNIQUE)
           CREATE LOCAL TEMPORARY TABLE L (K INTEGER NOT NULL UNIQUE) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9] each session has rows of
/// its own in a temporary table, empty at its start, which no other sees;
/// its constraints hold of each session's rows alone.
#[test]
fn each_session_has_its_own_rows_4_9() {
    let db = temporary();
    let (one, two) = (db.session("A"), db.session("A"));
    one.run("INSERT INTO L VALUES (1), (2)").unwrap();
    two.run("INSERT INTO L VALUES (1)").unwrap();
    assert_eq!(sorted(&one, "SELECT K FROM L"), ["1", "2"]);
    assert_eq!(sorted(&two, "SELECT K FROM L"), ["1"], "its own 1: UNIQUE holds of each session's rows");
    assert_eq!(exec(&two, "INSERT INTO L VALUES (1)").err(), Some(Code::Constraint));
    one.commit().unwrap();
    two.commit().unwrap();
    assert_eq!(count(&db.session("A"), "SELECT K FROM L"), 0, "empty in a new session");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.3 SR4, §14.3 GR3] ON COMMIT
/// DELETE ROWS, implicit, empties the table as its transaction commits;
/// PRESERVE ROWS keeps the rows across commits; a rollback undoes either's
/// changes.
#[test]
fn what_a_commit_does_to_the_rows_14_3() {
    let db = temporary();
    let s = db.session("A");
    s.run("INSERT INTO G VALUES (1)").unwrap();
    s.run("INSERT INTO L VALUES (1)").unwrap();
    assert_eq!((count(&s, "SELECT K FROM G"), count(&s, "SELECT K FROM L")), (1, 1));
    s.commit().unwrap();
    assert_eq!((count(&s, "SELECT K FROM G"), count(&s, "SELECT K FROM L")), (0, 1), "G emptied, L kept");
    s.run("INSERT INTO G VALUES (2)").unwrap();
    s.run("DELETE FROM L").unwrap();
    s.rollback().unwrap();
    assert_eq!((count(&s, "SELECT K FROM G"), count(&s, "SELECT K FROM L")), (0, 1), "rolled back");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.8 GR1, §13.10 GR1, §13.7
/// GR1] a READ ONLY transaction writes a temporary table, and no
/// persistent one (SQLCODE -917).
#[test]
fn a_read_only_transaction_writes_temporary_tables_13_8() {
    let db = temporary();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE P (K INTEGER)").unwrap();
    let s = db.session("A");
    s.run("SET TRANSACTION READ ONLY").unwrap();
    s.run("INSERT INTO L VALUES (1)").unwrap();
    s.run("UPDATE L SET K = 2").unwrap();
    s.run("DELETE FROM L").unwrap();
    assert_eq!(exec(&s, "INSERT INTO B.P VALUES (1)").err(), Some(Code::TransactionState));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.8 SR4, SR5; §11.9 SR3,
/// SR4] a foreign key, or a CHECK, of a table refers only to tables of the
/// kinds it may: a global temporary table to global ones, a created local
/// one to those or created local ones, a persistent one to none
/// temporary; and one whose rows a commit keeps to none whose rows it
/// deletes; else refused (SQLCODE -103).
#[test]
fn which_tables_a_temporary_table_refers_to_11_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE P (K INTEGER NOT NULL UNIQUE)
           CREATE GLOBAL TEMPORARY TABLE G (K INTEGER NOT NULL UNIQUE)
           CREATE GLOBAL TEMPORARY TABLE GP (K INTEGER NOT NULL UNIQUE) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let s = db.session("A");
    let refused = [
        "CREATE GLOBAL TEMPORARY TABLE X (K INTEGER REFERENCES P (K))",
        "CREATE TABLE X (K INTEGER REFERENCES G (K))",
        "CREATE GLOBAL TEMPORARY TABLE X (K INTEGER REFERENCES G (K)) ON COMMIT PRESERVE ROWS",
        "CREATE TABLE X (K INTEGER CHECK (K IN (SELECT K FROM GP)))",
        "CREATE LOCAL TEMPORARY TABLE X (K INTEGER CHECK (K IN (SELECT K FROM G))) ON COMMIT PRESERVE ROWS",
    ];
    for sql in refused {
        assert_eq!(exec(&s, sql).err(), Some(Code::SyntaxRule), "{sql}");
    }
    s.run("CREATE GLOBAL TEMPORARY TABLE C (K INTEGER REFERENCES G (K))").unwrap();
    s.run("CREATE LOCAL TEMPORARY TABLE D (K INTEGER REFERENCES GP (K), CHECK (K IN (SELECT K FROM G)))").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "INSERT INTO C VALUES (9)").err(), Some(Code::Constraint), "G has no 9");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.3 SR2, §11.3 SR3, §21.3.8]
/// privileges on a temporary table are granted as ALL PRIVILEGES alone; ON
/// COMMIT is of a temporary table alone; [DESIGN] one names no engine or
/// options, by CREATE TABLE or ALTER TABLE, nor is given an index; the
/// Information Schema gives each table's kind; one is dropped as any is.
#[test]
fn what_a_temporary_table_refuses_11_3() {
    let db = temporary();
    let s = db.session("A");
    assert_eq!(exec(&s, "GRANT SELECT ON L TO B").err(), Some(Code::SyntaxRule), "§10.3 SR2");
    s.run("GRANT ALL PRIVILEGES ON L TO B").unwrap();
    assert_eq!(exec(&s, "CREATE TABLE X (K INTEGER) ON COMMIT DELETE ROWS").err(), Some(Code::SyntaxRule), "§11.3 SR3");
    assert_eq!(exec(&s, "CREATE GLOBAL TEMPORARY TABLE X (K INTEGER) USING rowstore").err(), Some(Code::SyntaxRule));
    assert_eq!(exec(&s, "ALTER TABLE L SET (OUT_OF_LINE = 1)").err(), Some(Code::SyntaxRule), "SET");
    assert_eq!(exec(&s, "ALTER TABLE L USING rowstore").err(), Some(Code::SyntaxRule), "USING");
    s.commit().unwrap();
    assert_eq!(db.create_index("L_K", "A.L", &["K"], "btree").map_err(|e| e.code), Err(Code::SyntaxRule));
    let kinds = "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY TABLE_NAME";
    let kinds: Vec<String> =
        crate::common::rows(&s, kinds).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(kinds, ["G,GLOBAL TEMPORARY", "L,LOCAL TEMPORARY"]);
    s.run("DROP TABLE G CASCADE").unwrap();
    s.commit().unwrap();
    assert_eq!(crate::common::code(&s, "SELECT K FROM G"), Code::SyntaxRule);
}

/// The database in `dir`.
fn open(dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(&crate::common::os(), dir, &turborel::db::OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9] a temporary table's
/// definition is kept, through a crash and a clean close; its rows are
/// never: written under the Strict policy with a durable log, which
/// refuses a table its engine would not keep, and after either the table
/// is empty.
#[test]
fn definitions_last_and_rows_do_not_4_9() {
    let scratch = crate::common::Scratch::new("temporary");
    let dir = scratch.path("db");
    let db = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE GLOBAL TEMPORARY TABLE T (K INTEGER) ON COMMIT PRESERVE ROWS").unwrap();
    db.set_transaction_policy(turborel::txn::Policy::Strict).unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1), (2)").unwrap();
    s.commit().unwrap();
    assert_eq!(count(&s, "SELECT K FROM T"), 2);
    drop((s, db));
    let db = open(&dir);
    assert_eq!(count(&db.session("A"), "SELECT K FROM T"), 0, "recovered: the definition, not the rows");
    db.close().unwrap();
    drop(db);
    let db = open(&dir);
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (3)").unwrap();
    assert_eq!(count(&s, "SELECT K FROM T"), 1);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.11, §20.1] a declared local
/// temporary table is the session's alone, named MODULE.T, its columns
/// MODULE.T.C, with every privilege; its constraints hold, a foreign key
/// to a created local temporary table among them; ON COMMIT PRESERVE ROWS keeps
/// its rows, DELETE ROWS, implicit, empties them; it lasts the session,
/// through rollbacks, and is declared once [SR1].
#[test]
fn a_declared_table_is_the_sessions_13_11() {
    let db = temporary();
    let (s, other) = (db.session("A"), db.session("A"));
    s.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER NOT NULL UNIQUE REFERENCES A.L (K)) ON COMMIT PRESERVE ROWS").unwrap();
    s.run("DECLARE LOCAL TEMPORARY TABLE MODULE.E (K INTEGER)").unwrap();
    s.run("INSERT INTO L VALUES (1), (2)").unwrap();
    s.run("INSERT INTO MODULE.D VALUES (1), (2)").unwrap();
    s.run("INSERT INTO MODULE.E VALUES (5)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO MODULE.D VALUES (1)").err(), Some(Code::Constraint), "UNIQUE");
    assert_eq!(sorted(&s, "SELECT MODULE.D.K FROM MODULE.D WHERE MODULE.D.K > 1"), ["2"]);
    assert_eq!(crate::common::code(&other, "SELECT K FROM MODULE.D"), Code::SyntaxRule, "not another session's");
    s.commit().unwrap();
    assert_eq!((count(&s, "SELECT K FROM MODULE.D"), count(&s, "SELECT K FROM MODULE.E")), (2, 0), "D kept, E emptied");
    assert_eq!(exec(&s, "DECLARE LOCAL TEMPORARY TABLE MODULE.E (K INTEGER)").err(), Some(Code::SyntaxRule), "SR1");
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT K FROM MODULE.D"), 2, "still declared");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.3 SR1, §11.10 SR5, §11.18
/// SR3, §11.19 SR5, §13.11 GR5, §11.9 SR3a] refused (SQLCODE -103): a
/// grant or revoke on a declared local temporary table, altering or
/// dropping it, a view naming it, and a persistent table's CHECK reading
/// it; it is in no view of the Information Schema; [DESIGN] no schema is
/// named MODULE; a declaration is not prepared.
#[test]
fn what_a_declared_table_refuses_13_11() {
    let db = temporary();
    let s = db.session("A");
    s.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER)").unwrap();
    let refused = [
        "GRANT ALL PRIVILEGES ON MODULE.D TO B",
        "REVOKE ALL PRIVILEGES ON MODULE.D FROM B RESTRICT",
        "ALTER TABLE MODULE.D ADD COLUMN M INTEGER",
        "DROP TABLE MODULE.D CASCADE",
        "CREATE VIEW V AS SELECT K FROM MODULE.D",
        "CREATE TABLE P (K INTEGER CHECK (K IN (SELECT K FROM MODULE.D)))",
    ];
    for sql in refused {
        let error = s.run(sql).unwrap_err();
        assert_eq!(error.code, Code::SyntaxRule, "{sql}");
        assert!(error.msg.contains("declared local temporary"), "{sql}: {}", error.msg);
    }
    s.run("CREATE DOMAIN DD AS INTEGER").unwrap();
    let tables = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'MODULE'";
    assert_eq!(count(&s, tables), 0, "GR5");
    assert_eq!(db.schema("CREATE SCHEMA \"MODULE\" AUTHORIZATION A").err().map(|e| e.code), Some(Code::SyntaxRule));
    assert!(s.prepare("DECLARE LOCAL TEMPORARY TABLE MODULE.F (K INTEGER)").is_err());
    assert_eq!(count(&s, "SELECT K FROM MODULE.D"), 0);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.9, §7.3 GR3b] an instance not
/// yet written reads as empty, through its index too; a failed statement
/// on one is undone alone, the transaction going on; one is read and
/// written at every isolation level, by its index and its rows.
#[test]
fn an_instance_reads_and_writes_as_any_table_4_9() {
    let db = temporary();
    let s = db.session("A");
    assert_eq!(count(&s, "SELECT K FROM L WHERE K = 1"), 0, "by its index, never written");
    assert_eq!(count(&s, "SELECT K FROM L WHERE K > 0"), 0);
    s.run("INSERT INTO L VALUES (1), (2), (3)").unwrap();
    assert_eq!(exec(&s, "INSERT INTO L VALUES (4), (1)").err(), Some(Code::Constraint), "undone alone");
    assert_eq!(sorted(&s, "SELECT K FROM L WHERE K = 2 OR K = 3"), ["2", "3"]);
    s.run("UPDATE L SET K = K + 10 WHERE K = 1").unwrap();
    s.run("DELETE FROM L WHERE K = 2").unwrap();
    s.commit().unwrap();
    for level in ["READ COMMITTED", "REPEATABLE READ", "READ UNCOMMITTED"] {
        s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        assert_eq!(sorted(&s, "SELECT K FROM L"), ["11", "3"], "{level}");
        s.commit().unwrap();
    }
    s.run("INSERT INTO L SELECT K + 100 FROM L").unwrap();
    assert_eq!(count(&s, "SELECT K FROM L WHERE K > 100"), 2);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR6, SR8b; §13.11] the
/// names without a schema's in a declaration are of the session's default
/// schema, MODULE.T naming a declared table: its domain, the table its
/// foreign key references, and those its CHECK reads, and a domain it
/// CASTs to.
#[test]
fn names_in_a_declaration_5_4() {
    let db = temporary();
    let a = db.session("A");
    a.run("CREATE DOMAIN AMOUNT AS INTEGER CHECK (VALUE > 0)").unwrap();
    a.commit().unwrap();
    let declared = "DECLARE LOCAL TEMPORARY TABLE MODULE.D (K AMOUNT REFERENCES L (K), CHECK (K IN (SELECT K FROM L)), CHECK (CAST (K AS AMOUNT) < 9))";
    a.run(declared).unwrap();
    a.run("INSERT INTO L VALUES (1)").unwrap();
    a.run("INSERT INTO MODULE.D VALUES (1)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO MODULE.D VALUES (0)").err(), Some(Code::Constraint), "AMOUNT");
    a.run("INSERT INTO L VALUES (2)").unwrap();
    a.run("DELETE FROM L WHERE K = 2").unwrap();
    assert_eq!(exec(&a, "INSERT INTO MODULE.D VALUES (2)").err(), Some(Code::Constraint), "the foreign key, and the CHECK");
    a.run("INSERT INTO L VALUES (9)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO MODULE.D VALUES (9)").err(), Some(Code::Constraint), "the CAST's CHECK");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.10, §4.9]; [DESIGN] RULES.md
/// D49: a temporary table is altered only while no session's instance of
/// it holds a row, its own's included (SQLSTATE TB911); while the
/// transaction altering it is open, no other writes an instance of it;
/// once it commits, each session's instance is of the new definition.
#[test]
fn a_temporary_table_is_altered_while_unused_11_10() {
    let db = temporary();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO L VALUES (1)").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&b, "ALTER TABLE L ADD COLUMN M INTEGER").err(), Some(Code::Conflict), "A's row, kept by its commit");
    b.rollback().unwrap();
    a.run("DELETE FROM L").unwrap();
    a.commit().unwrap();
    b.run("ALTER TABLE L ADD COLUMN M INTEGER DEFAULT 7").unwrap();
    assert_eq!(exec(&a, "INSERT INTO L VALUES (2)").err(), Some(Code::Conflict), "being altered");
    a.rollback().unwrap();
    b.commit().unwrap();
    a.run("INSERT INTO L (K) VALUES (2)").unwrap();
    assert_eq!(crate::common::rows(&a, "SELECT K, M FROM L"), Ok(vec!["2,7".into()]), "of the new definition");
    a.commit().unwrap();
    b.run("INSERT INTO G VALUES (1)").unwrap();
    assert_eq!(exec(&b, "ALTER TABLE G ADD COLUMN M INTEGER").err(), Some(Code::Conflict), "its own row");
    b.commit().unwrap();
    b.run("ALTER TABLE G ADD COLUMN M INTEGER").unwrap();
    b.commit().unwrap();
    assert_eq!(crate::common::rows(&a, "SELECT K, M FROM G"), Ok(vec![]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.3 GR3, GR4, GR5a]: at
/// COMMIT, a temporary table of ON COMMIT DELETE ROWS is emptied before the
/// deferred constraints are checked, so a row of it that breaks one does
/// not roll the transaction back; one of PRESERVE ROWS keeps its rows, so
/// such a row does (SQLSTATE 40002).
#[test]
fn delete_rows_comes_before_deferred_checks_14_3() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE GLOBAL TEMPORARY TABLE D (K INTEGER CHECK (K > 0) DEFERRABLE INITIALLY DEFERRED) ON COMMIT DELETE ROWS
           CREATE GLOBAL TEMPORARY TABLE P (K INTEGER CHECK (K > 0) DEFERRABLE INITIALLY DEFERRED) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO D VALUES (-1)").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO P VALUES (-1)").unwrap();
    let refused = a.commit().unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "40002", "{refused}");
    assert_eq!(crate::common::rows(&a, "SELECT K FROM P"), Ok(vec![]), "rolled back");
}
