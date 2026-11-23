//! \[DESIGN\] A point UPDATE that reads of the row it replaces the columns
//! its statement sets alone (RULES.md D875). A compiled program's UPDATE
//! of one row by its key is written straight (D841) and tells its write
//! the columns it sets, so the row before is decoded by those columns
//! and its log record made of them. The same UPDATE interpreted runs the
//! general path, which reads the row before whole. Each case here runs
//! both and finds the same rows, the same index entries and the same log.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::disk::io::FileSystem;

use crate::common::{Scratch, after_crash, bytes_of, holds, os, rows};

/// T, with its primary key's index and one on each of A and B. Its CHECK
/// reads a column no UPDATE here reads, so each is written whole from the
/// row it read, not as a patch (RULES.md D917), and is still told the
/// columns it sets.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION A
   CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER, B INTEGER, C CHARACTER(12), D DECIMAL(6,2) NOT NULL CHECK (D > 0))";

/// One point UPDATE each: of an indexed column, of a column no index
/// reads, of both, and of an indexed column from its value before. A
/// value of -1, or of 'none', sets null.
const PROCEDURES: [&str; 4] = [
    "CREATE PROCEDURE SETA (IN KK INTEGER, IN X INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET A = NULLIF(X, -1) WHERE K = KK; END",
    "CREATE PROCEDURE SETC (IN KK INTEGER, IN X CHARACTER(12)) MODIFIES SQL DATA BEGIN UPDATE T SET C = NULLIF(X, 'none') WHERE K = KK; END",
    "CREATE PROCEDURE SETAC (IN KK INTEGER, IN X INTEGER, IN Y CHARACTER(12)) MODIFIES SQL DATA
       BEGIN UPDATE T SET A = X, C = Y WHERE K = KK; END",
    "CREATE PROCEDURE BUMPB (IN KK INTEGER) MODIFIES SQL DATA BEGIN UPDATE T SET B = B + 1 WHERE K = KK; END",
];

/// Rows of T as the cases begin: row K holds A = K + 100, B = K + 200.
const ROWS: i64 = 12;

/// A database in `dir`, opened through `fs`, with T, its indexes, its rows
/// and `PROCEDURES`, all committed; and A's session in it, its procedures
/// compiled or not.
fn database(dir: &std::path::Path, fs: &Arc<dyn FileSystem>, compiled: bool) -> (Database, Session) {
    let db = Database::new();
    // One instant for every commit, so two logs of the same changes are the same bytes.
    db.set_clock(Arc::new(FixedClock::at(1_000_000))).unwrap();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.schema(SCHEMA).unwrap();
    db.create_index("T_A", "A.T", &["A"], "btree").unwrap();
    db.create_index("T_B", "A.T", &["B"], "btree").unwrap();
    let s = session(&db, compiled);
    let values: Vec<String> = (1..=ROWS).map(|k| format!("({k}, {}, {}, 'row {k}', {k}.25)", k + 100, k + 200)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    for procedure in PROCEDURES {
        s.run(procedure).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// A's session in `db`, its procedures compiled or not.
fn session(db: &Database, compiled: bool) -> Session {
    let s = db.session("A");
    s.run(&format!("SET EXECUTOR.COMPILED_PROCEDURES = '{}'", if compiled { "ON" } else { "OFF" })).unwrap();
    s.commit().unwrap();
    s
}

/// The UPDATEs of the cases, a transaction each list: an indexed column
/// changed, a column no index reads changed, an indexed column given the
/// value it has, two columns of which one keeps its value, an indexed
/// column changed from its own value, and a row changed several times in
/// one transaction. Each procedure is called often enough for its plan to
/// be kept and its change written straight.
fn committed() -> Vec<Vec<String>> {
    let mut all = Vec::new();
    for k in 1..=6_i64 {
        all.push(vec![format!("CALL SETA({k}, {})", k + 500), format!("CALL SETC({k}, 'changed {k}')")]);
    }
    for k in 1..=6_i64 {
        // A keeps the value it was given above. C changes, then keeps its own.
        all.push(vec![
            format!("CALL SETA({k}, {})", k + 500),
            format!("CALL SETAC({k}, {}, 'both {k}')", k + 500),
            format!("CALL SETAC({k}, {}, 'both {k}')", k + 600),
            format!("CALL BUMPB({k})"),
            format!("CALL BUMPB({k})"),
        ]);
    }
    all.push(vec!["CALL SETA(99, 1)".to_owned(), "CALL SETC(7, 'none')".to_owned(), "CALL SETA(8, -1)".to_owned()]);
    all
}

/// Runs each list of `script` in `s` and commits it.
fn run(s: &Session, script: &[Vec<String>]) {
    for transaction in script {
        for call in transaction {
            s.run(call).unwrap();
        }
        s.commit().unwrap();
    }
}

/// T as `s` reads it: its rows, then the keys each value of A and of B
/// is found under, so each index answers for the entries it holds.
fn contents(s: &Session) -> Vec<String> {
    let mut all = rows(s, "SELECT K, A, B, C, D FROM T ORDER BY K").unwrap();
    for (column, values) in [("A", [101, 106, 107, 501, 506, 601, 606]), ("B", [201, 202, 203, 206, 207, 208, 212])] {
        for v in values {
            all.push(format!("{column} = {v}: {:?}", rows(s, &format!("SELECT K FROM T WHERE {column} = {v} ORDER BY K")).unwrap()));
        }
    }
    all.push(format!("A IS NULL: {:?}", rows(s, "SELECT K FROM T WHERE A IS NULL ORDER BY K").unwrap()));
    s.commit().unwrap();
    all
}

/// How many entries the indexes on A and on B hold.
fn entries(db: &Database) -> (usize, usize) {
    (db.index_entries("T_A").unwrap(), db.index_entries("T_B").unwrap())
}

/// Verifies: [DESIGN] RULES.md D875, D841; [X3.135-1986 §8.12 GR3]: the
/// same UPDATEs written straight, reading of the row replaced the columns
/// set alone, and by the general path, reading it whole, leave the same
/// rows, the same entries in each index and the same log, byte for byte.
/// Of an indexed column changed, of a column no index reads, of a column
/// given the value it has, of two columns of which one keeps its value,
/// and with two indexes of which one is touched; a transaction of them
/// rolled back leaves the rows as they were, either way.
#[test]
fn updates_by_the_columns_set_leave_what_whole_updates_leave() {
    let scratch = Scratch::new("sparse-updates-alike");
    let mut seen = Vec::new();
    for compiled in [false, true] {
        let dir = scratch.path(if compiled { "compiled" } else { "interpreted" });
        let (db, s) = database(&dir, &os(), compiled);
        run(&s, &committed());
        let before = contents(&s);
        for call in ["CALL SETA(2, 7)", "CALL SETC(2, 'undone')", "CALL SETAC(3, 8, 'undone')", "CALL BUMPB(3)"] {
            s.run(call).unwrap();
        }
        s.rollback().unwrap();
        assert_eq!(contents(&s), before, "rolled back, compiled {compiled}");
        let held = entries(&db);
        drop((s, db));
        seen.push((before, held, bytes_of(&dir, "log")));
    }
    let (compiled, interpreted) = (seen.pop().unwrap(), seen.pop().unwrap());
    assert_eq!(compiled.0, interpreted.0, "rows and the keys found under each value");
    assert_eq!(compiled.1, interpreted.1, "index entries");
    assert!(holds(&compiled.2, "both 6") && holds(&compiled.2, "changed 1"), "the log holds the changes");
    assert!(compiled.2 == interpreted.2, "the logs differ: {} bytes compiled, {} interpreted", compiled.2.len(), interpreted.2.len());
    assert!(compiled.0.contains(&"A = 606: [\"6\"]".to_owned()) && compiled.0.contains(&"A = 506: []".to_owned()), "{:?}", compiled.0);
}

/// Verifies: [DESIGN] RULES.md D875, D25; [X3.135-1986 §4.16]: recovery
/// after a crash replays the records of UPDATEs made of the columns set.
/// Those committed after the last checkpoint are done again from their
/// rows after, and those of a transaction open at the crash are undone
/// from the columns their records hold of the rows before, every other
/// column as it was. The rows and the keys each index finds are those of
/// a database that ran the committed UPDATEs alone, reading rows whole.
#[test]
fn recovery_replays_updates_by_the_columns_set() {
    let scratch = Scratch::new("sparse-updates-recovered");
    let expected = {
        let (_db, s) = database(&scratch.path("whole"), &os(), false);
        run(&s, &committed());
        contents(&s)
    };
    let dir = scratch.path("crashed");
    let (crashed, s) = database(&dir, &after_crash(), true);
    let open = session(&crashed, true);
    crashed.checkpoint().unwrap();
    // Still open at the crash, each of its rows changed more than once.
    for k in 9..=ROWS {
        for call in [format!("CALL SETA({k}, {k})"), format!("CALL SETAC({k}, 0, 'open {k}')"), format!("CALL BUMPB({k})")] {
            open.run(&call).unwrap();
        }
    }
    // Its changes in the pages the checkpoint holds, for recovery to undo.
    crashed.checkpoint().unwrap();
    run(&s, &committed());
    drop((s, crashed));
    let db = Database::new();
    let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.damaged_tables().unwrap(), vec![]);
    let s = session(&db, true);
    assert_eq!(contents(&s), expected);
    // Its records hold as the rows it finds: written again, they read the same.
    run(&s, &[vec!["CALL SETA(9, 109)".to_owned(), "CALL BUMPB(10)".to_owned()]]);
    assert_eq!(rows(&s, "SELECT A, B FROM T WHERE K IN (9, 10) ORDER BY K"), Ok(vec!["109,209".to_owned(), "110,211".to_owned()]));
    drop(open);
}
