//! \[EXTENSION\] The eight table lock modes and their conflicts (RULES.md
//! D186), locking queries over joins (D185), the locks of tables an engine
//! leaves to the coordinator (D185), and INFORMATION_SCHEMA.LOCKS (D188).

use std::thread;

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};
use turborel::txn::LockWait;

/// The modes LOCK TABLE takes, in the order of the matrix.
const MODES: [&str; 8] = [
    "ACCESS SHARE",
    "ROW SHARE",
    "ROW EXCLUSIVE",
    "SHARE UPDATE EXCLUSIVE",
    "SHARE",
    "SHARE ROW EXCLUSIVE",
    "EXCLUSIVE",
    "ACCESS EXCLUSIVE",
];

/// Which of `MODES` conflict, row by row, 'x' a conflict.
const MATRIX: [&str; 8] = [".......x", "......xx", "....xxxx", "...xxxxx", "..xx.xxx", "..xxxxxx", ".xxxxxxx", "xxxxxxxx"];

/// A database of `A.Q (ID, DONE)` and `A.W (ID, NOTE)`, each holding 1 to
/// 3, committed.
fn tables(db: &Database) {
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE Q (ID INTEGER NOT NULL PRIMARY KEY, DONE INTEGER)
           CREATE TABLE W (ID INTEGER NOT NULL PRIMARY KEY, NOTE CHAR(2))",
    )
    .unwrap();
    let s = db.session("A");
    for id in 1..=3 {
        s.run(&format!("INSERT INTO Q VALUES ({id}, 0)")).unwrap();
        s.run(&format!("INSERT INTO W VALUES ({id}, 'W{id}')")).unwrap();
    }
    s.commit().unwrap();
}

/// The rows `sql` gives in `s`, each its values joined by commas.
fn opened(s: &Session, sql: &str) -> Result<Vec<String>, SqlError> {
    let mut c = s.open_cursor(sql)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch()? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// The SQLSTATE `sql` fails with in `s`, or "" if it does not fail.
fn state(s: &Session, sql: &str) -> String {
    opened(s, sql).err().map(|e| e.sqlstate().as_str().to_owned()).unwrap_or_default()
}

/// Verifies: [EXTENSION] RULES.md D186: of each pair of the eight modes,
/// LOCK TABLE NOWAIT in the second is refused with SQLSTATE TBL01 while
/// another transaction holds the first exactly when the two conflict; a
/// transaction holds each mode it took, a weaker one taken after a
/// stronger leaving the stronger held.
#[test]
fn lock_table_modes_conflict_as_the_matrix_says_d186() {
    let db = Database::new();
    tables(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    for (held, row) in MODES.iter().zip(MATRIX) {
        for (asked, cell) in MODES.iter().zip(row.chars()) {
            a.run(&format!("LOCK TABLE Q IN {held} MODE")).unwrap();
            let got = b.run(&format!("LOCK TABLE Q IN {asked} MODE NOWAIT")).map_err(|e| e.sqlstate().as_str().to_owned());
            assert_eq!(got.is_err(), cell == 'x', "{held} then {asked}: {got:?}");
            if let Err(state) = got {
                assert_eq!(state, "TBL01");
            }
            a.rollback().unwrap();
            b.rollback().unwrap();
        }
    }
    a.run("LOCK TABLE Q IN SHARE MODE").unwrap();
    a.run("LOCK TABLE Q IN ROW SHARE MODE").unwrap();
    assert_eq!(b.run("LOCK TABLE Q IN ROW EXCLUSIVE MODE NOWAIT").unwrap_err().sqlstate().as_str(), "TBL01", "SHARE is held still");
}

/// Verifies: [EXTENSION] RULES.md D186: writers of a table whose engine
/// isolates transactions hold ROW EXCLUSIVE, so two never wait for each
/// other, nor for LOCK TABLE in ROW EXCLUSIVE; LOCK TABLE in SHARE holds
/// them out, and a writer holds out LOCK TABLE in SHARE.
#[test]
fn writers_share_row_exclusive_d186() {
    let db = Database::new();
    tables(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO Q VALUES (10, 0)").unwrap();
    b.run("INSERT INTO Q VALUES (11, 0)").unwrap();
    a.run("LOCK TABLE Q IN ROW EXCLUSIVE MODE").unwrap();
    b.run("UPDATE Q SET DONE = 1 WHERE ID = 1").unwrap();
    assert_eq!(a.run("LOCK TABLE Q IN SHARE MODE NOWAIT").unwrap_err().sqlstate().as_str(), "TBL01");
    a.rollback().unwrap();
    b.rollback().unwrap();
    a.run("LOCK TABLE Q IN SHARE MODE").unwrap();
    assert_eq!(b.run("INSERT INTO Q VALUES (12, 0)").unwrap_err().code, Code::Conflict, "SHARE holds writers out");
    assert_eq!(opened(&b, "SELECT COUNT(*) FROM Q").unwrap(), vec!["3"], "but not readers");
}

/// Verifies: [EXTENSION] RULES.md D185: a locking query of a join locks
/// the rows of each table without OF, and of the tables OF names alone
/// with it; a row of an outer join with no row of a table locks nothing of
/// it; OF names only a table the join reads.
#[test]
fn locking_queries_over_joins_d185() {
    let db = Database::new();
    tables(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    let join = "SELECT Q.ID, W.NOTE FROM Q JOIN W ON Q.ID = W.ID WHERE Q.ID = 1";
    assert_eq!(opened(&a, &format!("{join} FOR UPDATE OF Q")).unwrap(), vec!["1,W1"]);
    assert_eq!(state(&b, "SELECT ID FROM Q WHERE ID = 1 FOR UPDATE NOWAIT"), "TBL01");
    assert_eq!(opened(&b, "SELECT ID FROM W WHERE ID = 1 FOR UPDATE NOWAIT").unwrap(), vec!["1"], "W is not named");
    a.rollback().unwrap();
    b.rollback().unwrap();
    assert_eq!(opened(&a, "SELECT Q.ID, X.NOTE FROM W AS X, Q WHERE Q.ID = X.ID AND Q.ID = 2 FOR SHARE").unwrap(), vec!["2,W2"]);
    assert_eq!(state(&b, "SELECT ID FROM Q WHERE ID = 2 FOR UPDATE NOWAIT"), "TBL01");
    assert_eq!(state(&b, "SELECT ID FROM W WHERE ID = 2 FOR UPDATE NOWAIT"), "TBL01");
    assert_eq!(opened(&b, "SELECT ID FROM W WHERE ID = 3 FOR UPDATE NOWAIT").unwrap(), vec!["3"]);
    a.rollback().unwrap();
    b.rollback().unwrap();
    b.run("INSERT INTO Q VALUES (4, 0)").unwrap();
    b.commit().unwrap();
    let outer = "SELECT Q.ID, W.NOTE FROM Q LEFT JOIN W ON Q.ID = W.ID WHERE Q.ID >= 3 ORDER BY Q.ID FOR UPDATE";
    assert_eq!(opened(&a, outer).unwrap(), vec!["3,W3", "4,NULL"]);
    assert_eq!(opened(&b, "SELECT Q.ID FROM Q JOIN W ON Q.ID = W.ID ORDER BY Q.ID FOR UPDATE SKIP LOCKED").unwrap(), vec!["1", "2"]);
    assert_eq!(opened(&b, &format!("{join} FOR UPDATE OF X")).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(opened(&b, "SELECT DISTINCT Q.ID FROM Q, W FOR UPDATE").unwrap_err().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] RULES.md D185: a table whose engine leaves
/// concurrency to the coordinator is locked whole by a locking query: FOR
/// UPDATE in SHARE UPDATE EXCLUSIVE, FOR SHARE in SHARE. While another
/// holds it so, SKIP LOCKED passes over every row and NOWAIT is refused at
/// once; readers read on.
#[test]
fn tables_locked_whole_skip_and_refuse_at_once_d185() {
    let db = crate::common::locking_database();
    tables(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID = 1 FOR UPDATE").unwrap(), vec!["1"]);
    assert_eq!(opened(&b, "SELECT ID FROM Q ORDER BY ID FOR UPDATE SKIP LOCKED").unwrap(), Vec::<String>::new());
    assert_eq!(state(&b, "SELECT ID FROM Q FOR UPDATE NOWAIT"), "TBL01");
    assert_eq!(state(&b, "SELECT ID FROM Q FOR SHARE NOWAIT"), "TBL01");
    assert_eq!(opened(&b, "SELECT COUNT(*) FROM Q").unwrap(), vec!["3"], "a reader reads on");
    assert_eq!(opened(&b, "SELECT ID FROM W WHERE ID = 1 FOR UPDATE SKIP LOCKED").unwrap(), vec!["1"], "another table");
    a.rollback().unwrap();
    b.rollback().unwrap();
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID = 1 FOR SHARE").unwrap(), vec!["1"]);
    assert_eq!(opened(&b, "SELECT ID FROM Q WHERE ID = 2 FOR SHARE NOWAIT").unwrap(), vec!["2"], "SHARE beside SHARE");
    assert_eq!(state(&b, "SELECT ID FROM Q FOR UPDATE NOWAIT"), "TBL01");
    a.rollback().unwrap();
    b.rollback().unwrap();
    assert_eq!(opened(&b, "SELECT ID FROM Q ORDER BY ID FOR UPDATE SKIP LOCKED").unwrap(), vec!["1", "2", "3"]);
}

/// Verifies: [EXTENSION] RULES.md D188: INFORMATION_SCHEMA.LOCKS shows the
/// database's owner each mode a transaction holds a table's lock in, each
/// row lock, each advisory lock, and each lock a transaction waits for, by
/// session and transaction; anyone else sees none.
#[test]
fn information_schema_locks_shows_held_and_sought_d188() {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    tables(&db);
    let a = db.session("A");
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID = 2 FOR UPDATE").unwrap(), vec!["2"]);
    a.run("LOCK TABLE W IN SHARE MODE").unwrap();
    assert_eq!(opened(&a, "VALUES (ADVISORY_LOCK(7))").unwrap(), vec!["TRUE"]);
    let shown = "SELECT LOCK_TYPE, TABLE_NAME, ROW_ID IS NOT NULL, ADVISORY_KEY, LOCK_MODE, IS_WAITING, \
                 TRANSACTION_ID IS NOT NULL FROM INFORMATION_SCHEMA.LOCKS ORDER BY 1, 2, 5";
    let owner = db.session("OWNER");
    assert_eq!(
        opened(&owner, shown).unwrap(),
        vec![
            "ADVISORY,NULL,FALSE,7,EXCLUSIVE,NO,FALSE",
            "ROW,Q,TRUE,NULL,FOR UPDATE,NO,TRUE",
            "TABLE,Q,FALSE,NULL,ROW SHARE,NO,TRUE",
            "TABLE,W,FALSE,NULL,SHARE,NO,TRUE",
        ]
    );
    let b = db.session("A");
    let waiter = thread::spawn(move || {
        let got = b.run("UPDATE Q SET DONE = 1 WHERE ID = 2").map(drop).map_err(|e| e.code);
        b.rollback().unwrap();
        got
    });
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiter.is_finished(), "it waits");
        thread::yield_now();
    }
    let sought = "SELECT LOCK_TYPE, TABLE_NAME, LOCK_MODE FROM INFORMATION_SCHEMA.LOCKS WHERE IS_WAITING = 'YES'";
    assert_eq!(opened(&owner, sought).unwrap(), vec!["ROW,Q,FOR UPDATE"]);
    let sessions = "SELECT COUNT(DISTINCT SESSION_ID) FROM INFORMATION_SCHEMA.LOCKS";
    assert_eq!(opened(&owner, sessions).unwrap(), vec!["2"]);
    assert_eq!(opened(&db.session("A"), shown).unwrap(), Vec::<String>::new(), "only the owner sees them");
    a.rollback().unwrap();
    assert_eq!(waiter.join().unwrap(), Ok(()));
}
