//! \[DESIGN\] A bare COUNT(*) counted by its table's storage: of the row
//! store from its settled zones (RULES.md D350, D351), of the column store
//! from its groups' directory (D1180); the count a scan makes,
//! at every isolation level, of rows committed, uncommitted, deleted and
//! committed after a snapshot, of sealed rows too, and noted by EXPLAIN.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{RunLength, Scramble, exec, rows};

/// The rows of T written.
const ROWS: i64 = 3_000;

/// A database of table T of schema A, K never null, holding `ROWS` rows
/// committed, sealed through the test encodings if `sealed`, its zones
/// settled; its background work done only when a test asks (JOBS.POLICY
/// MANUAL, RULES.md D55).
fn counted_table(sealed: bool) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_encoding(Box::new(RunLength)).unwrap();
    db.register_encoding(Box::new(Scramble)).unwrap();
    let with = if sealed { " WITH (ENCODINGS = 'TEST SCRAMBLE')" } else { "" };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, S VARCHAR(1000)){with}")).unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(500) {
        let values: Vec<String> =
            (first..first + 500).map(|k| format!("({k}, '{}')", format!("s{k} ").repeat(usize::try_from(k % 60).unwrap()))).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// T's COUNT(*), and its count of K, never null, which a scan reads.
fn both(s: &Session) -> (Vec<String>, Vec<String>) {
    (rows(s, "SELECT COUNT(*) FROM T").unwrap(), rows(s, "SELECT COUNT(K) FROM T").unwrap())
}

/// T's COUNT(*), as text, checked against the scan's.
fn count(s: &Session) -> String {
    let (counted, scanned) = both(s);
    assert_eq!(counted, scanned);
    counted.concat()
}

/// Has T of `db` counted at READ COMMITTED, REPEATABLE READ and
/// SERIALIZABLE, each count checked against the scan's, as another session
/// deletes rows of K below 100 and inserts two, then commits.
fn counts_at_every_level(db: &Database) {
    let (writer, committed, repeatable, serializable) = (db.session("A"), db.session("A"), db.session("A"), db.session("A"));
    committed.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    repeatable.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    serializable.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    for s in [&committed, &repeatable, &serializable] {
        assert_eq!(count(s), "3000");
    }
    exec(&writer, "DELETE FROM T WHERE K < 100").unwrap();
    exec(&writer, "INSERT INTO T VALUES (-1, 'new'), (-2, 'new')").unwrap();
    assert_eq!(count(&writer), "2902", "its own changes");
    for s in [&committed, &repeatable, &serializable] {
        assert_eq!(count(s), "3000", "another's uncommitted changes");
    }
    writer.commit().unwrap();
    assert_eq!((count(&committed), count(&repeatable)), ("2902".to_owned(), "3000".to_owned()));
    db.maintain().unwrap();
    assert_eq!(count(&db.session("A")), "2902", "summed up");
    assert_eq!(count(&repeatable), "3000", "its snapshot still");
    let refused = exec(&serializable, "INSERT INTO T VALUES (-3, 'late')").and_then(|_| serializable.commit().map_err(|e| e.code));
    assert_eq!(refused.err(), Some(Code::Conflict), "read the whole table, which another changed");
}

/// Verifies: [X3.135-1986 §5.8 GR4b] [DESIGN] RULES.md D351, D29: COUNT(*) is the scan's count at READ COMMITTED, REPEATABLE
/// READ and SERIALIZABLE: of rows committed, from settled zones; of
/// another session's uncommitted inserts and deletions, not counted, and
/// of its own, counted; of a commit after a snapshot, counted by READ
/// COMMITTED's next statement and not by REPEATABLE READ's snapshot; of
/// rows sealed through encodings alike.
#[test]
fn a_count_is_the_scans_at_every_isolation_level() {
    for sealed in [false, true] {
        counts_at_every_level(&counted_table(sealed));
    }
}

/// Verifies: [X3.135-1986 §5.8 GR4b] [DESIGN] RULES.md D1180, D29: COUNT(*)
/// of a table of the column store, its rows frozen into groups sorted by
/// a column of their own, is the scan's count at every isolation level,
/// as frozen rows are deleted and rows added; EXPLAIN notes it counted by
/// the storage.
#[test]
fn a_grouped_count_is_the_scans_at_every_isolation_level() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, S VARCHAR(1000))
           USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'S')",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(500) {
        let values: Vec<String> = (first..first + 500).map(|k| format!("({k}, 's{}')", k % 37)).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let plan = a.explain("SELECT COUNT(*) FROM T", &Params::new()).unwrap();
    assert!(plan.lines().any(|l| l == "-- counted by storage: A.T"), "{plan}");
    counts_at_every_level(&db);
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D351: EXPLAIN notes the table
/// a bare COUNT(*) counts by its storage, under a projection too, and
/// never runs it in parallel, of a table of the column store too (D1180),
/// and a count of a column known not null, which counts rows (D1172); of
/// a count under a filter, a grouping, a count of a column that may hold
/// a null or with EXECUTOR.BATCHES OFF, it notes none.
#[test]
fn explain_notes_a_count_by_the_storage() {
    let db = counted_table(false);
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE U (K INTEGER) USING columnstore").unwrap();
    let a = db.session("A");
    let explained = |s: &Session, sql: &str| s.explain(sql, &Params::new()).unwrap();
    let noted = |s: &Session, sql: &str| explained(s, sql).lines().any(|l| l == "-- counted by storage: A.T");
    assert!(noted(&a, "SELECT COUNT(*) FROM T"));
    assert!(noted(&a, "SELECT COUNT(*) + 1, COUNT(*) FROM T"));
    assert!(noted(&a, "SELECT COUNT(K) FROM T"));
    for sql in ["SELECT COUNT(*) FROM T WHERE K > 1", "SELECT K, COUNT(*) FROM T GROUP BY K", "SELECT COUNT(S) FROM T"] {
        assert!(!noted(&a, sql), "{sql}");
    }
    a.run("SET EXECUTOR.PARALLEL_MIN_ROWS = 1").unwrap();
    a.run("SET EXECUTOR.MORSEL_ROWS = 100").unwrap();
    let plan = explained(&a, "SELECT COUNT(*) FROM T");
    assert!(!plan.contains("Parallel") && plan.contains("-- counted by storage: A.T"), "never split: {plan}");
    assert!(explained(&a, "SELECT SUM(K) FROM T").contains("Parallel"), "a sum is split");
    let c = db.session("C");
    assert!(explained(&c, "SELECT COUNT(*) FROM U").contains("-- counted by storage: C.U"), "a table of the column store too");
    a.run("SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    assert!(!noted(&a, "SELECT COUNT(*) FROM T"));
    assert_eq!(count(&a), "3000", "scanned row by row");
}
