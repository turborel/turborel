//! TRUNCATE TABLE [ISO/IEC 9075-2:2023 §14.10]: every row of a base table
//! deleted at once, its indexes emptied with it, and with RESTART IDENTITY
//! its identity column restarted; its Syntax, Access and General Rules; a
//! temporary table's rows of the session alone; the constraints that read
//! the table checked whole; what other transactions see; and recovery.

use std::sync::Arc;

use turborel::db::{Completion, Database, Direct, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};
use turborel::params::Params;
use turborel::value::{DataType, Value};

use crate::common::{Scratch, after_crash};

fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class of a refusal.
fn refused<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// The SQLSTATE of a refusal.
fn state<T>(result: Result<T, SqlError>) -> Option<String> {
    result.err().map(|e| e.sqlstate().as_str().to_owned())
}

/// Schema A: T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER) of rows 1 to
/// 3, N = 10 K, and U (K INTEGER).
fn filled() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER)
           CREATE TABLE U (K INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.commit().unwrap();
    db
}

/// The COMMAND_FUNCTION of the statement `s` ran last [ISO/IEC 9075:1992
/// §18.1 GR1c].
fn command(s: &Session) -> String {
    let mut p = Params::new().declare("F", DataType::varying(128), Value::Null);
    s.execute("GET DIAGNOSTICS F = COMMAND_FUNCTION", &mut p).unwrap();
    p.value("F").to_string().trim_end().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 GR5, GR7] TRUNCATE TABLE deletes
/// every row of the table, which a rollback restores and a commit shows
/// another session; its PRIMARY KEY's index is emptied with it, so a key
/// it held is free; of no row it is no data; COMMAND_FUNCTION names it
/// TRUNCATE TABLE [§23.1 Table 32]; prepared, it runs as written [§20.6].
#[test]
fn rows_deleted_14_10() {
    let db = filled();
    let a = db.session("A");
    assert_eq!(a.run("TRUNCATE TABLE T").unwrap(), Completion::Success);
    assert_eq!(command(&a), "TRUNCATE TABLE");
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["0"]);
    a.run("INSERT INTO T VALUES (1, 11)").unwrap();
    assert_eq!(query(&a, "SELECT * FROM T WHERE K = 1"), ["1,11"], "its index emptied, its key free");
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT K FROM T ORDER BY K"), ["1", "2", "3"], "rolled back");
    a.run("TRUNCATE TABLE A.T CONTINUE IDENTITY").unwrap();
    a.commit().unwrap();
    let b = db.session("A");
    assert_eq!(query(&b, "SELECT COUNT(*) FROM T"), ["0"], "committed");
    b.rollback().unwrap();
    assert_eq!(a.run("TRUNCATE TABLE T").unwrap(), Completion::NoData, "GR7");
    a.run("INSERT INTO T VALUES (4, 40)").unwrap();
    let mut p = Params::new().declare("TXT", DataType::varying(64), Value::Char("TRUNCATE TABLE T".to_owned()));
    a.execute("PREPARE S FROM TXT", &mut p).unwrap();
    a.execute("EXECUTE S", &mut p).unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["0"], "prepared");
    assert!(matches!(a.direct("TRUNCATE TABLE T").unwrap(), Direct::Done(Completion::NoData)), "direct");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 SR5, GR8; §9.37 SR9c, SR12]
/// CONTINUE IDENTITY, written or not, leaves the identity column's sequence
/// generator as it was; RESTART IDENTITY restarts it at its start value,
/// with the rows, and a rollback undoes both, its values never given
/// twice; a start value beyond the generator's bounds now refuses the
/// truncation, which deletes no row (SQLCODE -103).
#[test]
fn identity_restarted_14_10() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER GENERATED ALWAYS AS IDENTITY (START WITH 10), V CHAR(1))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (V) VALUES ('a'), ('b')").unwrap();
    a.run("TRUNCATE TABLE T").unwrap();
    a.run("INSERT INTO T (V) VALUES ('c')").unwrap();
    a.run("TRUNCATE TABLE T CONTINUE IDENTITY").unwrap();
    a.run("INSERT INTO T (V) VALUES ('d')").unwrap();
    assert_eq!(query(&a, "SELECT K, V FROM T"), ["13,d"], "SR5, CONTINUE");
    a.commit().unwrap();
    a.run("TRUNCATE TABLE T RESTART IDENTITY").unwrap();
    a.run("INSERT INTO T (V) VALUES ('e')").unwrap();
    assert_eq!(query(&a, "SELECT K, V FROM T"), ["10,e"], "GR8");
    a.rollback().unwrap();
    a.run("INSERT INTO T (V) VALUES ('f')").unwrap();
    assert_eq!(query(&a, "SELECT K, V FROM T ORDER BY K"), ["13,d", "14,f"], "rolled back, 10 not given again");
    a.commit().unwrap();
    a.run("ALTER TABLE T ALTER COLUMN K SET MINVALUE 12").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("TRUNCATE TABLE T RESTART IDENTITY")), Some(Code::SyntaxRule), "SR12");
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 SR2, SR3, SR4, AR2; §5.2] the
/// table is a base table of a schema: no table, a view, or a declared local
/// temporary table is refused (SQLCODE -103); so is one a foreign key
/// references, another table's or its own; one of a schema the session
/// does not own (-104); the `<identity column restart option>` is CONTINUE
/// or RESTART, then IDENTITY; TRUNCATE, which SQL-92 does not reserve,
/// names a column, and a table, which a query given directly may begin
/// with, as TRUNCATE TABLE alone does not.
#[test]
fn rules_14_10() {
    let db = filled();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K FROM T").unwrap();
    a.run("CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY)").unwrap();
    a.run("CREATE TABLE C (K INTEGER REFERENCES P)").unwrap();
    a.run("CREATE TABLE S (K INTEGER NOT NULL PRIMARY KEY, UP INTEGER REFERENCES S)").unwrap();
    a.run("CREATE TABLE W (TRUNCATE INTEGER)").unwrap();
    a.run("CREATE TABLE TRUNCATE (K INTEGER)").unwrap();
    a.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (K INTEGER)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("TRUNCATE TABLE NONE")), Some(Code::SyntaxRule), "SR2");
    assert_eq!(refused(a.run("TRUNCATE TABLE V")), Some(Code::SyntaxRule), "SR3");
    assert_eq!(refused(a.run("TRUNCATE TABLE MODULE.D")), Some(Code::SyntaxRule), "SR2, declared");
    assert_eq!(refused(a.run("TRUNCATE TABLE P")), Some(Code::SyntaxRule), "SR4");
    assert_eq!(refused(a.run("TRUNCATE TABLE S")), Some(Code::SyntaxRule), "SR4, its own");
    a.run("TRUNCATE TABLE C").unwrap();
    assert_eq!(refused(db.session("B").run("TRUNCATE TABLE A.T")), Some(Code::Privilege), "AR2");
    for wrong in ["TRUNCATE TABLE T RESTART", "TRUNCATE TABLE T IDENTITY", "TRUNCATE T", "TRUNCATE TABLE T CONTINUE"] {
        assert_eq!(refused(a.run(wrong)), Some(Code::Syntax), "{wrong}");
    }
    a.run("INSERT INTO W (TRUNCATE) VALUES (1)").unwrap();
    assert_eq!(query(&a, "SELECT TRUNCATE FROM W"), ["1"]);
    assert!(matches!(a.direct("TRUNCATE CROSS JOIN W").unwrap(), Direct::Rows(_)), "a query");
    a.run("TRUNCATE TABLE TRUNCATE").unwrap();
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["3"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 GR1, GR2, GR3] in a READ ONLY
/// transaction a persistent table is not truncated (SQLSTATE 25006), a
/// temporary one is; while a cursor of the transaction reads the table it
/// is not (36002), nor while one reads it through a view, but it is while
/// one reads another.
#[test]
fn read_only_and_cursors_14_10() {
    let db = filled();
    let a = db.session("A");
    a.run("CREATE GLOBAL TEMPORARY TABLE G (K INTEGER) ON COMMIT PRESERVE ROWS").unwrap();
    a.run("CREATE VIEW V AS SELECT K FROM T").unwrap();
    a.commit().unwrap();
    a.run("SET TRANSACTION READ ONLY").unwrap();
    assert_eq!(state(a.run("TRUNCATE TABLE T")).as_deref(), Some("25006"), "GR1");
    a.run("TRUNCATE TABLE G").unwrap();
    a.rollback().unwrap();
    let mut cursor = a.open_cursor("SELECT K FROM T").unwrap();
    assert_eq!(state(a.run("TRUNCATE TABLE T")).as_deref(), Some("36002"), "GR2, GR3");
    drop(cursor.fetch().unwrap());
    drop(cursor);
    let mut through = a.open_cursor("SELECT K FROM V").unwrap();
    assert_eq!(state(a.run("TRUNCATE TABLE T")).as_deref(), Some("36002"), "through a view");
    drop(through.fetch().unwrap());
    drop(through);
    let _other = a.open_cursor("SELECT K FROM U").unwrap();
    a.run("TRUNCATE TABLE T").unwrap();
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.6 GR3, §11.34 GR2;
/// ISO/IEC 9075-2:2023 §14.10 GR5] each constraint that reads the table is
/// checked whole once its rows go: an immediate assertion it breaks
/// refuses it (SQLCODE -201), the rows kept; a deferred one fails the
/// commit; another table's CHECK with a subquery refuses it. A deferred
/// UNIQUE constraint's checks of rows gone wait harmlessly.
#[test]
fn constraints_checked_whole_14_10() {
    let db = filled();
    let a = db.session("A");
    a.run("CREATE ASSERTION SOME_ROW CHECK (EXISTS (SELECT * FROM T))").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("TRUNCATE TABLE T")), Some(Code::Constraint), "immediate");
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T"), ["3"], "the statement undone");
    a.run("DROP ASSERTION SOME_ROW").unwrap();
    a.run("CREATE ASSERTION LATER CHECK (EXISTS (SELECT * FROM T)) INITIALLY DEFERRED").unwrap();
    a.commit().unwrap();
    a.run("TRUNCATE TABLE T").unwrap();
    assert_eq!(refused(a.commit()), Some(Code::CommitFailed), "deferred");
    a.run("DROP ASSERTION LATER").unwrap();
    a.run("CREATE TABLE R (K INTEGER CHECK (K IN (SELECT K FROM T)))").unwrap();
    a.run("INSERT INTO R VALUES (2)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("TRUNCATE TABLE T")), Some(Code::Constraint), "another table's CHECK");
    a.run("DROP TABLE R CASCADE").unwrap();
    a.run("CREATE TABLE Q (K INTEGER, CONSTRAINT ONCE UNIQUE (K) INITIALLY DEFERRED)").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO Q VALUES (1), (1)").unwrap();
    a.run("TRUNCATE TABLE Q").unwrap();
    a.run("INSERT INTO Q VALUES (1)").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT K FROM Q"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2] RULES.md D51 the
/// totals an assertion keeps are put right once the rows it reads are
/// truncated, immediate or deferred, so they are judged of the rows there
/// are: of none, then 90, and not of the 60 gone. MC/DC of the deferred
/// decision: deferred, the checks of rows of earlier statements waiting
/// are forgotten, so neither the 30 inserted before the truncation nor the
/// 30 an update took away counts against the 90 after; immediate, none
/// waits.
#[test]
fn totals_put_right_14_10() {
    for deferred in [false, true] {
        let db = filled();
        let a = db.session("A");
        a.run("CREATE ASSERTION BELOW CHECK ((SELECT SUM(N) FROM T) < 100) DEFERRABLE").unwrap();
        a.commit().unwrap();
        if deferred {
            a.run("SET CONSTRAINTS ALL DEFERRED").unwrap();
            a.run("INSERT INTO T VALUES (4, 30)").unwrap();
            a.run("UPDATE T SET N = 25 WHERE K = 3").unwrap();
        }
        a.run("TRUNCATE TABLE T").unwrap();
        a.run("INSERT INTO T VALUES (9, 90)").unwrap();
        a.commit().unwrap();
        assert_eq!(refused(a.run("INSERT INTO T VALUES (8, 10)")), Some(Code::Constraint), "100 reached, deferred {deferred}");
        a.rollback().unwrap();
        a.run("INSERT INTO T VALUES (8, 9)").unwrap();
        a.commit().unwrap();
        assert_eq!(query(&a, "SELECT SUM(N) FROM T"), ["99"], "deferred {deferred}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR2, §11.16 GR2,
/// §11.34 GR2] RULES.md D51 regression: ALTER TABLE adding a column copies
/// the table's rows, and adding a constraint checks each, without counting
/// them again in the totals an assertion that reads it keeps, so a sum of
/// 60 under 100 is no sum of 120 refusing the ALTER TABLE, nor of 180
/// refusing a row of 30 after it.
#[test]
fn alter_keeps_totals_11_11() {
    let db = filled();
    let a = db.session("A");
    a.run("CREATE ASSERTION BELOW CHECK ((SELECT SUM(N) FROM T) < 100)").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T ADD COLUMN M INTEGER").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T ADD CONSTRAINT POSITIVE CHECK (N > 0)").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T (K, N) VALUES (4, 30)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("INSERT INTO T (K, N) VALUES (5, 10)")), Some(Code::Constraint), "100 reached");
    a.rollback().unwrap();
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §14.10 GR4, IA234] a row
/// another transaction committed after this one's snapshot, which the
/// truncation would take unseen, refuses its commit at REPEATABLE READ too,
/// the row kept; a transaction that began before a truncation committed is
/// refused at its next use of the table (SQLCODE -911), as after ALTER
/// TABLE.
#[test]
fn others_14_10() {
    let db = filled();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM U"), ["0"]);
    b.run("INSERT INTO T VALUES (4, 40)").unwrap();
    b.commit().unwrap();
    a.run("TRUNCATE TABLE T").unwrap();
    assert_eq!(refused(a.commit()), Some(Code::Conflict), "a row committed after its snapshot");
    b.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(query(&b, "SELECT COUNT(*) FROM T"), ["4"], "kept");
    a.run("TRUNCATE TABLE T").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(b.open_cursor("SELECT K FROM T")), Some(Code::Conflict), "began before");
    b.rollback().unwrap();
    assert_eq!(query(&b, "SELECT COUNT(*) FROM T"), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10 GR5, GR8] RULES.md D49 a temporary
/// table's rows truncated are the session's alone, another's kept, and a
/// rollback restores them; RESTART IDENTITY restarts its identity column.
/// MC/DC of the restart's decision, RESTART and an identity column: of a
/// table with one, it restarts, so a value is given again; without
/// RESTART, it does not; of a table with none, RESTART alters nothing, so
/// another session's transaction open on its instance does not refuse it.
#[test]
fn temporary_14_10() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE GLOBAL TEMPORARY TABLE G (K INTEGER GENERATED ALWAYS AS IDENTITY, V CHAR(1)) ON COMMIT PRESERVE ROWS
           CREATE GLOBAL TEMPORARY TABLE H (K INTEGER) ON COMMIT PRESERVE ROWS",
    )
    .unwrap();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO G (V) VALUES ('a'), ('b')").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO G (V) VALUES ('c')").unwrap();
    b.commit().unwrap();
    assert_eq!(a.run("TRUNCATE TABLE G").unwrap(), Completion::Success);
    assert_eq!(query(&a, "SELECT COUNT(*) FROM G"), ["0"]);
    a.rollback().unwrap();
    assert_eq!(query(&a, "SELECT V FROM G ORDER BY V"), ["a", "b"], "rolled back");
    a.run("TRUNCATE TABLE G").unwrap();
    a.run("INSERT INTO G (V) VALUES ('d')").unwrap();
    assert_eq!(query(&a, "SELECT K, V FROM G"), ["4,d"], "CONTINUE");
    a.commit().unwrap();
    assert_eq!(query(&b, "SELECT K, V FROM G"), ["3,c"], "another's kept");
    b.commit().unwrap();
    a.run("TRUNCATE TABLE G RESTART IDENTITY").unwrap();
    a.run("INSERT INTO G (V) VALUES ('e')").unwrap();
    assert_eq!(query(&a, "SELECT K, V FROM G"), ["1,e"], "RESTART");
    a.commit().unwrap();
    assert_eq!(a.run("TRUNCATE TABLE H").unwrap(), Completion::NoData, "GR7");
    b.run("INSERT INTO H VALUES (1)").unwrap();
    a.run("TRUNCATE TABLE H RESTART IDENTITY").unwrap();
    a.commit().unwrap();
    b.commit().unwrap();
    assert_eq!(query(&b, "SELECT K FROM H"), ["1"]);
}

/// The database in `dir`, opened from its files by `fs`.
fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D25, D27 a table truncated with RESTART
/// IDENTITY, and a row inserted in the same transaction, are as committed
/// after a close, a crash, and a crash after a checkpoint: the rows gone,
/// the row inserted kept, its index of it alone, the generator restarted,
/// a value given since never given again.
#[test]
fn recovered() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("truncate-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before
            .schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER GENERATED ALWAYS AS IDENTITY, V CHAR(1) NOT NULL UNIQUE)")
            .unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T (V) VALUES ('a'), ('b'), ('c')").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        a.run("TRUNCATE TABLE T RESTART IDENTITY").unwrap();
        a.run("INSERT INTO T (V) VALUES ('a')").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        let a = db.session("A");
        assert_eq!(query(&a, "SELECT K, V FROM T"), ["1,a"], "{case}");
        assert_eq!(refused(a.run("INSERT INTO T (V) VALUES ('a')")), Some(Code::Constraint), "{case}: its index");
        a.rollback().unwrap();
        a.run("INSERT INTO T (V) VALUES ('b')").unwrap();
        let given: i64 = query(&a, "SELECT K FROM T WHERE V = 'b'")[0].parse().unwrap();
        assert!(given > 2, "{case}: 1 and 2 were given, so {given} is neither");
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}
