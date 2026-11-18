//! Changes of definitions staged after a savepoint and cancelled by
//! ROLLBACK TO SAVEPOINT [ISO/IEC 9075-2:2023 §17.8 GR3d]: gone for the
//! transaction at once, never published as it commits, and left out by
//! recovery after a crash and by a standby following the log, as the
//! rows written after the savepoint are.

use std::sync::Arc;

use turborel::db::{Database, Standby};
use turborel::log::{Log, Lsn, MemoryLog};

use super::values;

/// A database logging to `log`: schema A with T (K, V), row 1 committed.
fn primary(log: &Arc<MemoryLog>) -> Database {
    let db = Database::new();
    db.set_log(Arc::clone(log) as Arc<dyn Log>).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4))").unwrap();
    db.create_index("T_V", "A.T", &["V"], "btree").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'one')").unwrap();
    a.commit().unwrap();
    db
}

/// One transaction of `db` that defines U before its savepoint and, after
/// it, defines V, adds a column to T, drops U's twin W and writes rows,
/// then goes back to the savepoint and commits what is left.
fn cancelled_after_a_savepoint(db: &Database) {
    let a = db.session("A");
    a.run("CREATE TABLE W (K INTEGER)").unwrap();
    a.commit().unwrap();
    a.run("CREATE TABLE U (K INTEGER)").unwrap();
    a.run("INSERT INTO U VALUES (1)").unwrap();
    a.run("SAVEPOINT S").unwrap();
    a.run("CREATE TABLE V (K INTEGER)").unwrap();
    a.run("INSERT INTO V VALUES (1)").unwrap();
    a.run("ALTER TABLE T ADD COLUMN X INTEGER").unwrap();
    a.run("UPDATE T SET X = 5").unwrap();
    a.run("DROP TABLE W CASCADE").unwrap();
    a.run("INSERT INTO T VALUES (2, 'two', 2)").unwrap();
    a.run("ROLLBACK TO SAVEPOINT S").unwrap();
    assert!(a.run("INSERT INTO V VALUES (2)").is_err(), "V cancelled");
    a.run("INSERT INTO T VALUES (3, 'tri')").unwrap();
    a.run("INSERT INTO W VALUES (3)").unwrap();
    a.commit().unwrap();
}

/// What the tests compare of a database: the tables of A, T's rows with
/// every column, U's and W's rows, and T_V's entries.
fn holds(db: &Database) -> (Vec<String>, Vec<String>, Vec<String>, usize) {
    let a = db.session("A");
    let tables = values(&a, "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY TABLE_NAME");
    let t = values(&a, "SELECT * FROM T ORDER BY K");
    let others = values(&a, "SELECT 'U', K FROM U UNION ALL SELECT 'W', K FROM W");
    (tables, t, others, db.index_entries("T_V").unwrap())
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d]; [DESIGN] RULES.md D88,
/// D25. A table defined, a column added and a table dropped after a
/// savepoint are cancelled by going back to it, with the rows written
/// since; the transaction commits the rest, and a database recovered from
/// the log after a crash holds what the primary holds, none of what was
/// cancelled.
#[test]
fn recovery_leaves_out_definitions_cancelled_by_a_savepoint() {
    let log = Arc::new(MemoryLog::new());
    let db = primary(&log);
    cancelled_after_a_savepoint(&db);
    let expected = (
        vec!["T".to_owned(), "U".to_owned(), "W".to_owned()],
        vec!["1,one".to_owned(), "3,tri".to_owned()],
        vec!["U,1".to_owned(), "W,3".to_owned()],
        2,
    );
    assert_eq!(holds(&db), expected);
    let recovered = Database::new();
    recovered.recover(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    assert_eq!(holds(&recovered), expected);
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d]; [DESIGN] RULES.md D88,
/// D57. A standby following the log, given it whole or one record per
/// call, leaves out the definitions and rows a savepoint's rollback
/// cancelled, and holds what its primary holds.
#[test]
fn a_standby_leaves_out_definitions_cancelled_by_a_savepoint() {
    let log = Arc::new(MemoryLog::new());
    let db = primary(&log);
    cancelled_after_a_savepoint(&db);
    let whole = Database::new();
    whole.set_log(Arc::new(MemoryLog::new())).unwrap();
    Standby::replay(&whole, &mut log.read_from(Lsn(0))).unwrap();
    assert_eq!(holds(&whole), holds(&db));
    let one_by_one = Database::new();
    one_by_one.set_log(Arc::new(MemoryLog::new())).unwrap();
    for read in log.read_from(Lsn(0)) {
        Standby::replay(&one_by_one, &mut std::iter::once(read)).unwrap();
    }
    assert_eq!(holds(&one_by_one), holds(&db));
}
