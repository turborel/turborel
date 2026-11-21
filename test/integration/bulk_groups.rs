//! \[EXTENSION\] A load into the column store written straight as frozen
//! groups (RULES.md D533): seen by its transaction alone until it commits,
//! then by each snapshot taken after; undone by a statement's failure, a
//! savepoint and a rollback; recovered after a crash, a checkpoint having
//! stored its groups while it was open, as if written through the tree.

use turborel::db::{Database, Opened, Session};
use turborel::error::Code;
use turborel::params::Load;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, count, reopened_after_crash, rows};

/// The rows of a load.
const ROWS: i64 = 12_000;

/// The schema S with T, of the column store in groups of 4096 rows: a key,
/// a value scrambled, and a CHECK.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL, \
                      C INTEGER CHECK (C >= 0)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'GROUPS')";

/// A database of `SCHEMA`, its background work done only when asked.
fn database() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(SCHEMA).unwrap();
    db
}

/// `n` as an exact numeric value.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// T's rows `from..to`, as a copy gives an INSERT's parameters.
fn given(from: i64, to: i64) -> Vec<Vec<Value>> {
    (from..to).map(|k| vec![int(k), int((k * 7919) % 101), int(k % 37)]).collect()
}

/// Inserts T's rows `from..to` in one INSERT, a load if 4096 or more.
fn inserted(s: &Session, (from, to): (i64, i64)) {
    let rows: Vec<String> = (from..to).map(|k| format!("({k}, {}, {})", (k * 7919) % 101, k % 37)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
}

/// What a session finds of T: its rows by a scan, by its key, and their
/// sum.
fn found(s: &Session) -> (usize, usize, Vec<String>) {
    (count(s, "SELECT K FROM T"), count(s, "SELECT K FROM T WHERE K = 5"), rows(s, "SELECT SUM(A), SUM(C) FROM T").unwrap())
}

/// Whether every row of T is in a group: its bytes not estimated.
fn all_frozen(s: &Session) -> bool {
    let sql = "SELECT BYTES_ESTIMATED FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'";
    rows(s, sql).unwrap() == ["NO"]
}

/// Verifies: [X3.135-1986 §4.16, §8.7 GR1c]; [EXTENSION] RULES.md D533:
/// a load into an empty table of the column store goes straight to its
/// groups, none of its rows through the tree; its transaction sees every
/// row, by a scan and by its key, and another sees none until the load
/// commits, nor after, while its REPEATABLE READ snapshot is from before,
/// though a READ COMMITTED statement after does; then each sees what rows
/// written one statement at a time give.
#[test]
fn a_load_goes_straight_to_groups_its_transaction_alone_sees() {
    let db = database();
    let (s, reader, each) = (db.session("S"), db.session("S"), db.session("S"));
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(found(&reader).0, 0);
    assert_eq!(found(&each).0, 0);
    inserted(&s, (0, ROWS));
    let alone = database();
    let one = alone.session("S");
    for first in (0..ROWS).step_by(1000) {
        inserted(&one, (first, first + 1000));
    }
    one.commit().unwrap();
    assert_eq!(found(&s), found(&one), "its own rows seen");
    assert_eq!(found(&reader), (0, 0, vec!["NULL,NULL".to_owned()]), "none of an open load's rows");
    assert_eq!(found(&each).0, 0, "none, read committed");
    s.commit().unwrap();
    assert_eq!(found(&reader).0, 0, "a snapshot from before the commit");
    assert_eq!(found(&each), found(&one), "a statement's snapshot after it");
    reader.commit().unwrap();
    assert_eq!(found(&reader), found(&one), "seen once committed");
    assert!(all_frozen(&reader), "no row through the tree");
    db.maintain().unwrap();
    assert_eq!(found(&reader), found(&one), "its groups settled");
    s.run("UPDATE T SET A = A + 1 WHERE K < 10").unwrap();
    s.run("DELETE FROM T WHERE K >= 11000").unwrap();
    one.run("UPDATE T SET A = A + 1 WHERE K < 10").unwrap();
    one.run("DELETE FROM T WHERE K >= 11000").unwrap();
    assert_eq!(found(&s), found(&one), "changed as any frozen row");
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §8.9]; [ISO/IEC 9075:1992 §14.2,
/// §14.3]; [EXTENSION] RULES.md D533: the groups of a load are undone by a
/// savepoint rolled back, by a statement of the load failing, the groups
/// of its executions before kept, and by a rollback; a load after loads
/// again, each of a copy's executions a group, and tells a copy the rows
/// of a group, so its executions make whole ones.
#[test]
fn the_groups_of_a_load_are_undone_with_it() {
    let db = database();
    let s = db.session("S");
    s.run("SAVEPOINT P").unwrap();
    inserted(&s, (0, 5000));
    s.run("ROLLBACK TO SAVEPOINT P").unwrap();
    assert_eq!(found(&s).0, 0, "a savepoint rolled back");
    let p = s.prepare("INSERT INTO T (K, A, C) VALUES (?, ?, ?)").unwrap();
    s.execute_prepared_rows(&p, &given(0, 4096), Load::Going).unwrap();
    let mut broken = given(4096, 5000);
    broken.push(vec![int(5000), int(1), int(-1)]);
    assert_eq!(s.execute_prepared_rows(&p, &broken, Load::Going).map_err(|e| e.code), Err(Code::Constraint));
    assert_eq!(found(&s).0, 4096, "the failed execution undone alone");
    s.rollback().unwrap();
    assert_eq!(found(&s).0, 0, "a rollback");
    assert_eq!(s.load_rows(), None, "no load under way");
    s.execute_prepared_rows(&p, &given(0, ROWS), Load::Going).unwrap();
    assert_eq!(s.load_rows(), Some(4096), "a copy's executions take a group's rows");
    s.execute_prepared_rows(&p, &[], Load::Ending).unwrap();
    assert_eq!(s.load_rows(), None, "ended");
    s.commit().unwrap();
    assert_eq!(found(&s).0, usize::try_from(ROWS).unwrap());
    assert!(all_frozen(&s), "no row through the tree");
}

/// Verifies: [EXTENSION] RULES.md D533: a load into a table of the
/// column store that leaves COLUMNSTORE.LOAD as TREE goes through its tree,
/// no group written, a copy told no group's rows; set to GROUPS in place,
/// the next load goes straight to groups.
#[test]
fn a_load_goes_through_the_tree_unless_asked() {
    let tree = || {
        let db = Database::new();
        db.set_job_policy(JobPolicy::Manual).unwrap();
        db.schema(&SCHEMA.replace(", COLUMNSTORE.LOAD = 'GROUPS'", "")).unwrap();
        db
    };
    let db = tree();
    let s = db.session("S");
    let p = s.prepare("INSERT INTO T (K, A, C) VALUES (?, ?, ?)").unwrap();
    s.execute_prepared_rows(&p, &given(0, 5000), Load::Going).unwrap();
    assert_eq!(s.load_rows(), None, "no group's rows told");
    s.execute_prepared_rows(&p, &[], Load::Ending).unwrap();
    s.commit().unwrap();
    assert_eq!(found(&s).0, 5000);
    assert!(!all_frozen(&s), "its rows in the tree");
    let asked = tree();
    let a = asked.session("S");
    a.run("ALTER TABLE T SET (COLUMNSTORE.LOAD = 'GROUPS')").unwrap();
    a.commit().unwrap();
    inserted(&a, (0, 5000));
    a.commit().unwrap();
    assert_eq!(found(&a).0, 5000);
    assert!(all_frozen(&a), "straight to groups once asked");
}

/// The bytes of T as TABLE_STATISTICS shows them.
fn bytes(s: &Session) -> Vec<String> {
    rows(s, "SELECT ROW_COUNT, BYTES FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'").unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, D533: a database crashed with a load
/// committed, then another open whose groups a checkpoint stored, is
/// recovered without a row of the open load or its groups, its bytes as
/// the committed load left them; one whose second load committed after
/// the checkpoint, before the crash, is recovered with both; one whose
/// second load committed before the checkpoint keeps its groups.
#[test]
fn a_checkpoint_during_an_open_load_recovers_without_it() {
    for second in ["open", "committed after", "committed before"] {
        let scratch = Scratch::new("bulk-groups-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        crashed.schema(SCHEMA).unwrap();
        let s = crashed.session("S");
        let p = s.prepare("INSERT INTO T (K, A, C) VALUES (?, ?, ?)").unwrap();
        s.execute_prepared_rows(&p, &given(0, 6000), Load::Ending).unwrap();
        s.commit().unwrap();
        crashed.checkpoint().unwrap();
        let first = bytes(&s);
        s.execute_prepared_rows(&p, &given(6000, ROWS), Load::Going).unwrap();
        if second == "committed before" {
            s.commit().unwrap();
        }
        crashed.checkpoint().unwrap();
        if second == "committed after" {
            s.commit().unwrap();
        }
        let (db, opened) = reopened_after_crash(&dir);
        assert!(matches!(opened, Opened::Recovered(_)), "{second}: {opened:?}");
        let r = db.session("S");
        let expected = if second == "open" { 6000 } else { usize::try_from(ROWS).unwrap() };
        assert_eq!(
            (count(&r, "SELECT K FROM T"), count(&r, "SELECT K FROM T WHERE K = 7000")),
            (expected, usize::from(expected > 6000)),
            "{second}"
        );
        if second == "open" {
            assert_eq!(bytes(&r), first, "the open load's groups gone");
        }
        r.execute_prepared_rows(&r.prepare("INSERT INTO T (K, A, C) VALUES (?, ?, ?)").unwrap(), &given(20_000, 20_010), Load::Ending)
            .unwrap();
        drop((db, s, crashed));
    }
}
