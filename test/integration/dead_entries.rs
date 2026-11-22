//! \[DESIGN\] Index entries of rows gone for every reader, taken back by
//! the scan that meets them (RULES.md D837): a queue's deletions, left by
//! collection, are taken back by the next search for its first entry,
//! never while a snapshot may still read them, never of a deletion not
//! committed, and a crash after loses nothing and keeps nothing of them.
//! Every session here collects only when told (D450), so what is taken
//! back is the scan's.

use std::path::Path;

use turborel::db::{Database, Opened, Session};
use turborel::params::Params;

use crate::common::{Scratch, reopened_after_crash, rows};

/// The first key of group 1, as the queue's head is found.
const HEAD: &str = "SELECT MIN(K) FROM Q WHERE G = 1";

/// The groups of `Q`, so a group's rows are found by its index.
const GROUPS: usize = 100;

/// Makes `Q` (G, K) in `db`, holding keys 1 to 10 of each group, its index
/// `Q_GK` on (G, K) made after them, and checks the head of group 1 is
/// found by that index.
fn queue(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE Q (G INTEGER NOT NULL, K INTEGER NOT NULL)").unwrap();
    let s = db.session("S");
    let values: Vec<String> = (1..=GROUPS).flat_map(|g| (1..=10).map(move |k| format!("({g}, {k})"))).collect();
    s.run(&format!("INSERT INTO Q VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db.create_index("Q_GK", "S.Q", &["G", "K"], "btree").unwrap();
    db.maintain().unwrap();
    let shown = s.explain(HEAD, &Params::new()).unwrap();
    assert!(shown.split("-- physical:").nth(1).is_some_and(|p| p.contains("Q_GK")), "{shown}");
    s.commit().unwrap();
}

/// The entries of `Q_GK` in `db` but those of the groups past 1, which
/// keep theirs.
fn entries(db: &Database) -> usize {
    db.index_entries("Q_GK").unwrap() - (GROUPS - 1) * 10
}

/// A session of `db` that collects only when told (RULES.md D450).
fn later(db: &Database) -> Session {
    let s = db.session("S");
    s.collect_after_reply(true);
    s
}

/// The first key of group 1 as `s` finds it.
fn head(s: &Session) -> Vec<String> {
    rows(s, HEAD).unwrap()
}

/// Deletes keys 1 to 5 in a session of `db` and commits, leaving their
/// garbage uncollected.
fn deleted_five(db: &Database) {
    let d = later(db);
    d.run("DELETE FROM Q WHERE G = 1 AND K <= 5").unwrap();
    d.commit().unwrap();
    assert_eq!(entries(db), 10, "left by collection");
}

/// Verifies: [DESIGN] RULES.md D837: a search for a queue's first key
/// takes back the entries of the deletions it passes, committed before
/// every snapshot, and collection after finds nothing left to take and
/// fails nothing.
#[test]
fn a_scan_takes_back_the_entries_of_rows_gone_for_every_reader() {
    let db = Database::new();
    queue(&db);
    deleted_five(&db);
    let r = later(&db);
    assert_eq!(head(&r), vec!["6".to_owned()]);
    assert_eq!(entries(&db), 5, "taken back by the scan");
    r.commit().unwrap();
    r.collect();
    assert_eq!(entries(&db), 5);
    assert_eq!(rows(&r, "SELECT COUNT(*) FROM Q WHERE G = 1"), Ok(vec!["5".to_owned()]));
    assert_eq!(head(&r), vec!["6".to_owned()]);
}

/// Verifies: [DESIGN] RULES.md D837, D29: a snapshot taken before the
/// deletions keeps their entries: a scan of a later one passes them and
/// takes none back, and the older one still finds the deleted rows by the
/// index. Once it ends, the next scan takes them back.
#[test]
fn an_older_snapshot_keeps_the_entries_it_may_read() {
    let db = Database::new();
    queue(&db);
    let old = later(&db);
    old.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(rows(&old, "SELECT COUNT(*) FROM Q WHERE G = 1"), Ok(vec!["10".to_owned()]));
    deleted_five(&db);
    let r = later(&db);
    assert_eq!(head(&r), vec!["6".to_owned()]);
    r.commit().unwrap();
    assert_eq!(entries(&db), 10, "a snapshot may read them");
    assert_eq!(head(&old), vec!["1".to_owned()], "found by the index");
    old.commit().unwrap();
    assert_eq!(head(&r), vec!["6".to_owned()]);
    assert_eq!(entries(&db), 5);
}

/// Verifies: [DESIGN] RULES.md D837, D29: a deletion not committed is
/// never taken back, neither by another's scan nor by its own
/// transaction's, which does not see its rows; once rolled back, every
/// row is found by the index again.
#[test]
fn a_deletion_rolled_back_is_never_taken_back() {
    let db = Database::new();
    queue(&db);
    let d = later(&db);
    d.run("DELETE FROM Q WHERE G = 1 AND K <= 5").unwrap();
    assert_eq!(head(&d), vec!["6".to_owned()], "its own deletion");
    let r = later(&db);
    assert_eq!(head(&r), vec!["1".to_owned()]);
    r.commit().unwrap();
    assert_eq!(entries(&db), 10);
    d.rollback().unwrap();
    assert_eq!(head(&r), vec!["1".to_owned()]);
    assert_eq!(rows(&r, "SELECT COUNT(*) FROM Q WHERE G = 1 AND K <= 5"), Ok(vec!["5".to_owned()]));
    assert_eq!(entries(&db), 10);
}

/// The queue of `dir` reopened after a crash, checked: the deleted rows
/// stay deleted, the others are found by the index and by a scan, and a
/// deleted key may be inserted again and is found.
fn recovered(dir: &Path, case: &str) {
    let (db, opened) = reopened_after_crash(dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{case}: {opened:?}");
    let r = db.session("S");
    assert_eq!(head(&r), vec!["6".to_owned()], "{case}");
    assert_eq!(rows(&r, "SELECT COUNT(*) FROM Q WHERE G + 0 = 1"), Ok(vec!["5".to_owned()]), "{case}");
    assert_eq!(rows(&r, "SELECT K FROM Q WHERE G = 1 AND K = 3"), Ok(Vec::new()), "{case}");
    r.run("INSERT INTO Q VALUES (1, 3)").unwrap();
    r.commit().unwrap();
    assert_eq!(head(&r), vec!["3".to_owned()], "{case}");
    r.commit().unwrap();
    assert_eq!(entries(&db), 6, "{case}: one entry a row");
}

/// Verifies: [DESIGN] RULES.md D837, D25: a database crashed after a scan
/// took back the entries of committed deletions is recovered with them
/// deleted and every other row found, from the log alone and from a
/// checkpoint taken after the scan, which logs as stale entries already
/// taken back.
#[test]
fn a_crash_after_entries_are_taken_back_loses_nothing() {
    for checkpoint in [false, true] {
        let scratch = Scratch::new("dead-entries-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        queue(&crashed);
        deleted_five(&crashed);
        let r = later(&crashed);
        assert_eq!(head(&r), vec!["6".to_owned()]);
        r.commit().unwrap();
        assert_eq!(entries(&crashed), 5);
        if checkpoint {
            crashed.checkpoint().unwrap();
        }
        recovered(&dir, if checkpoint { "a checkpoint after" } else { "the log alone" });
        drop((r, crashed));
    }
}
