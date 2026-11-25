//! The mark of a database reopened clean and running (RULES.md D1164) in
//! its copies (D57, D160, D162). An export, a backup and a restore of a
//! database marked so neither refuse it nor copy it, and a database
//! following that installs an export takes out a stale mark of the state
//! it replaces.

use std::sync::Arc;

use turborel::clock::FixedClock;
use turborel::db::backup::BackupOptions;
use turborel::db::{Database, OpenOptions, Standby};

use crate::backups::{base, contents, dir_target, open, options};
use crate::common::{Scratch, os, rows};

/// Verifies: [DESIGN] RULES.md D1164, D57; [EXTENSION] RULES.md D160,
/// D162: a database reopened clean, marked running, exports its files
/// with no mark among them; a backup of it restores, with its rows, into
/// a directory with no mark, which reopens with them; a database
/// following installs its export, a stale mark in its directory taken
/// out, with its rows.
///
/// Regression test (R-TEST-7): the mark was of no kind of file the
/// database knows, so each export, backup and state transfer of a
/// running database was refused (SQLCODE -910).
#[test]
fn a_database_marked_running_is_copied_without_its_mark() {
    let scratch = Scratch::new("running-mark");
    let clock = Arc::new(FixedClock::at(1_000));
    let dir = scratch.path("db");
    let first = open(&dir, &clock);
    base(&first, 20);
    first.close().unwrap();
    drop(first);
    let db = open(&dir, &clock);
    assert!(dir.join("opened").exists(), "reopened clean, marked running");
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (20, 'after the start')").unwrap();
    s.commit().unwrap();
    drop(s);
    let expected = contents(&db);
    assert_eq!(expected.len(), 21);

    let manifest = db.export_manifest().unwrap();
    assert!(manifest.iter().all(|f| f.within.file_name().is_none_or(|n| n != "opened")), "{manifest:?}");

    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let into = scratch.path("restored");
    let restorer = Database::new();
    restorer.set_clock(Arc::clone(&clock) as Arc<dyn turborel::clock::Clock>).unwrap();
    restorer.restore(&options(&into, &location)).unwrap();
    assert!(!into.join("opened").exists(), "a restore is not the database running");
    let reopened = open(&into, &clock);
    assert_eq!(contents(&reopened), expected);
    reopened.close().unwrap();

    let follower = Database::new();
    let following = scratch.path("follower");
    follower.open_following(&os(), &following, &OpenOptions::default()).unwrap();
    std::fs::write(following.join("opened"), b"").unwrap();
    Standby::import(&follower, db.export().unwrap()).unwrap();
    assert!(!following.join("opened").exists(), "the mark of the state replaced taken out");
    assert_eq!(rows(&follower.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["21".into()]));
    follower.close().unwrap();
    db.close().unwrap();
}
