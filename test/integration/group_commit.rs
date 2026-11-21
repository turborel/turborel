//! Group commit [X3.135-1986 §8.2 GR3; RULES.md D25, D441, D740]:
//! commits that sync at once share the log's syncs, and each returns once
//! it, and every commit before it, is durable; a crash keeps every commit
//! that returned, and none that did not.

use std::thread;

use crate::common::{Scratch, reopened_after_crash, rows};

/// Committers committing at once, and the commits each makes.
const COMMITTERS: i64 = 8;
const COMMITS_EACH: i64 = 25;
/// The committer whose transaction is open at the crash.
const OPEN: i64 = 99;

/// Verifies: [X3.135-1986 §8.2 GR3]; [DESIGN] RULES.md D25, D441,
/// D740: committers syncing at once, through a file log: each commit,
/// once it returns, is seen by a snapshot taken after it, with every
/// commit its committer made before, so none returns before it is durable;
/// after a crash, the database opened again from its files holds every
/// commit that returned, and nothing of a transaction open at the crash.
#[test]
fn commits_at_once_are_durable_in_order() {
    let scratch = Scratch::new("group-commit");
    let dir = scratch.path("db");
    let (db, _) = reopened_after_crash(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (C INTEGER NOT NULL, N INTEGER NOT NULL, PRIMARY KEY (C, N))").unwrap();
    thread::scope(|scope| {
        for c in 0..COMMITTERS {
            let db = &db;
            scope.spawn(move || {
                let (s, reader) = (db.session("A"), db.session("A"));
                for n in 0..COMMITS_EACH {
                    s.run(&format!("INSERT INTO T VALUES ({c}, {n})")).unwrap();
                    s.commit().unwrap();
                    let seen = rows(&reader, &format!("SELECT COUNT(*) FROM T WHERE C = {c}")).unwrap();
                    reader.commit().unwrap();
                    assert_eq!(seen, [(n + 1).to_string()], "committer {c}, commit {n}");
                }
            });
        }
    });
    let open = db.session("A");
    open.run(&format!("INSERT INTO T VALUES ({OPEN}, 0)")).unwrap();
    // The database left open, as a crash leaves it, and opened again.
    let (back, _) = reopened_after_crash(&dir);
    let r = back.session("A");
    assert_eq!(rows(&r, "SELECT COUNT(*) FROM T").unwrap(), [(COMMITTERS * COMMITS_EACH).to_string()]);
    assert_eq!(rows(&r, &format!("SELECT COUNT(*) FROM T WHERE C = {OPEN}")).unwrap(), ["0"]);
    for c in 0..COMMITTERS {
        let each = rows(&r, &format!("SELECT COUNT(*), MIN(N), MAX(N) FROM T WHERE C = {c}")).unwrap();
        assert_eq!(each, [format!("{COMMITS_EACH},0,{}", COMMITS_EACH - 1)], "committer {c}");
    }
    drop(open);
}
