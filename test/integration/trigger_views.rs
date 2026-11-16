//! [ISO/IEC 9075-11:2006 draft §5.64 to §5.69, §6.53 to §6.58] The
//! Information Schema's views of triggers, over the Definition Schema's
//! trigger descriptors; and [ISO/IEC 9075-2:2023 §11.74 SR3, GR1] DROP
//! SEQUENCE of a sequence generator a trigger or a routine draws from.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{exec, rows};

/// Schema A: tables T (K, V) and U (N), sequence generator Q, function F;
/// TR1, a row trigger drawing from Q; TR2, an UPDATE OF V trigger whose
/// condition invokes F; TR3, a statement trigger; TR4, a row trigger after
/// TR1; and P, a procedure drawing from Q. B holds SELECT on T, C INSERT.
fn triggered() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, V INTEGER)
           CREATE TABLE U (N INTEGER)
           CREATE SEQUENCE Q
           CREATE FUNCTION F (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X
           CREATE TRIGGER TR1 AFTER INSERT ON T FOR EACH ROW INSERT INTO U VALUES (NEXT VALUE FOR Q)
           CREATE TRIGGER TR2 AFTER UPDATE OF V ON T REFERENCING OLD ROW AS O FOR EACH ROW
             WHEN (O.V > F(0)) DELETE FROM U WHERE N = O.K
           CREATE TRIGGER TR3 AFTER INSERT ON T FOR EACH STATEMENT DELETE FROM U
           CREATE TRIGGER TR4 AFTER INSERT ON T FOR EACH ROW DELETE FROM U
           CREATE PROCEDURE P () MODIFIES SQL DATA INSERT INTO U VALUES (NEXT VALUE FOR Q)
           GRANT SELECT ON T TO B
           GRANT INSERT ON T TO C",
    )
    .unwrap();
    db
}

/// The rows of `sql` in `s`, each value's trailing spaces cut, then
/// commits.
fn shown(s: &Session, sql: &str) -> Vec<String> {
    let out = rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.commit().unwrap();
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §5.69, §6.58
/// Description 2 to 13, §5.64, §6.53]: TRIGGERS has each trigger on a table
/// the user holds a privilege but SELECT on: its event, its place among the
/// triggers of its table, event, time and orientation, in the order of
/// creation, its condition and statement as written to the owner of its
/// schema and null to others, its orientation, its time, its transition
/// names, and a null timestamp; one holding SELECT alone sees none.
/// TRIGGERED_UPDATE_COLUMNS has each column of a trigger column list.
#[test]
fn triggers_are_described_5_69() {
    let db = triggered();
    let described = "SELECT TRIGGER_NAME, EVENT_MANIPULATION, EVENT_OBJECT_TABLE, ACTION_ORDER, ACTION_CONDITION, ACTION_STATEMENT, \
                     ACTION_ORIENTATION, ACTION_TIMING, ACTION_REFERENCE_OLD_ROW, ACTION_REFERENCE_NEW_ROW, CREATED \
                     FROM INFORMATION_SCHEMA.TRIGGERS ORDER BY TRIGGER_NAME";
    assert_eq!(
        shown(&db.session("A"), described),
        [
            "TR1,INSERT,T,1,NULL,INSERT INTO U VALUES (NEXT VALUE FOR Q),ROW,AFTER,NULL,NULL,NULL",
            "TR2,UPDATE,T,1,O.V > F(0),DELETE FROM U WHERE N = O.K,ROW,AFTER,O,NULL,NULL",
            "TR3,INSERT,T,1,NULL,DELETE FROM U,STATEMENT,AFTER,NULL,NULL,NULL",
            "TR4,INSERT,T,2,NULL,DELETE FROM U,ROW,AFTER,NULL,NULL,NULL",
        ]
    );
    let to_c = "SELECT TRIGGER_NAME, ACTION_CONDITION, ACTION_STATEMENT FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_NAME = 'TR2'";
    assert_eq!(shown(&db.session("C"), to_c), ["TR2,NULL,NULL"], "C holds INSERT, and owns no schema of them");
    assert_eq!(shown(&db.session("B"), described), Vec::<String>::new(), "B holds SELECT alone");
    let columns = "SELECT TRIGGER_NAME, EVENT_OBJECT_TABLE, EVENT_OBJECT_COLUMN FROM INFORMATION_SCHEMA.TRIGGERED_UPDATE_COLUMNS";
    assert_eq!(shown(&db.session("A"), columns), ["TR2,T,V"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-11:2006 draft §5.65 to §5.68,
/// §6.54 to §6.57]: the usage views show, to the owner of a trigger's
/// schema alone, the tables each triggered action reads, its triggered
/// action column set, its subject routines by specific name, and the
/// sequence generators it draws from.
#[test]
fn what_triggers_name_is_described_5_68() {
    let db = triggered();
    let a = db.session("A");
    let tables = "SELECT TRIGGER_NAME, TABLE_NAME FROM INFORMATION_SCHEMA.TRIGGER_TABLE_USAGE ORDER BY 1, 2";
    assert_eq!(shown(&a, tables), ["TR1,U", "TR2,U", "TR3,U", "TR4,U"]);
    let columns = "SELECT TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.TRIGGER_COLUMN_USAGE WHERE TRIGGER_NAME = 'TR2' ORDER BY 1, 2";
    assert_eq!(shown(&a, columns), ["T,K", "T,V", "U,N"]);
    let routines = "SELECT TRIGGER_NAME, R.ROUTINE_NAME FROM INFORMATION_SCHEMA.TRIGGER_ROUTINE_USAGE AS TRU \
                    JOIN INFORMATION_SCHEMA.ROUTINES AS R USING (SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME)";
    assert_eq!(shown(&a, routines), ["TR2,F"]);
    let sequences = "SELECT TRIGGER_NAME, SEQUENCE_SCHEMA, SEQUENCE_NAME FROM INFORMATION_SCHEMA.TRIGGER_SEQUENCE_USAGE";
    assert_eq!(shown(&a, sequences), ["TR1,A,Q"]);
    for usage in [tables, sequences] {
        assert_eq!(shown(&db.session("C"), usage), Vec::<String>::new(), "C owns no schema of them");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.74 SR3a, SR3b, GR1]: DROP SEQUENCE
/// with RESTRICT is refused (SQLCODE -103) while a trigger's triggered
/// action or a routine's body draws from the generator; with CASCADE the
/// trigger and the routine go with it, and a generator created again of
/// its name is drawn from by neither. Regression: RESTRICT dropped it,
/// leaving the trigger and the procedure to fail, or to draw from the new
/// generator.
#[test]
fn drop_sequence_takes_what_draws_from_it_11_74() {
    let db = triggered();
    let a = db.session("A");
    assert_eq!(exec(&a, "DROP SEQUENCE Q RESTRICT"), Err(Code::SyntaxRule), "SR3b: TR1 draws from it");
    a.rollback().unwrap();
    a.run("DROP TRIGGER TR1").unwrap();
    assert_eq!(exec(&a, "DROP SEQUENCE Q RESTRICT"), Err(Code::SyntaxRule), "SR3a: P draws from it");
    a.rollback().unwrap();
    a.run("DROP SEQUENCE Q CASCADE").unwrap();
    a.commit().unwrap();
    let names = "SELECT TRIGGER_NAME FROM INFORMATION_SCHEMA.TRIGGERS ORDER BY 1";
    assert_eq!(shown(&a, names), ["TR2", "TR3", "TR4"]);
    assert_eq!(shown(&a, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'"), ["F"]);
    a.run("CREATE SEQUENCE Q START WITH 100").unwrap();
    a.run("INSERT INTO T VALUES (1, 1)").unwrap();
    assert_eq!(shown(&a, "SELECT N FROM U"), Vec::<String>::new(), "TR1 is gone, and TR4 emptied U");
}
