//! A restore to a point in time while commits are pipelined (RULES.md D25,
//! D163, D164, D441): sessions commit on threads, each releasing its locks
//! before its commit record is synced, and a restore point is made among
//! them. A restore from a backup through the archive to that point holds
//! exactly the commits whose records the log holds before it, each of
//! which returned committed, and none after.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use turborel::backup::seed::RecoveryTarget;
use turborel::clock::FixedClock;
use turborel::db::backup::{BackupOptions, RestoreOptions};
use turborel::log::Lsn;
use turborel::value::Value;

use crate::backups::{dir_target, open, options, restored};
use crate::common::{Scratch, rows};

/// Sessions committing at once.
const WRITERS: i64 = 4;

/// Verifies: [EXTENSION] RULES.md D163, D164; [DESIGN] RULES.md D25, D441:
/// a restore point made while sessions commit, their locks released before
/// their syncs, restores exactly the commits logged before it: every one
/// of them returned committed, and no commit logged after it is there, by
/// its position and by its name.
#[test]
fn a_restore_point_among_pipelined_commits_holds_what_was_logged_before_it() {
    let scratch = Scratch::new("commit-restore");
    let clock = Arc::new(FixedClock::at(1_000));
    let db = open(&scratch.path("db"), &clock);
    let archive = dir_target(&scratch.path("archive"));
    db.set_setting("ARCHIVE.TARGET", &Value::Char(archive.clone())).unwrap();
    db.set_setting("ARCHIVE.MODE", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)").unwrap();
    let location = dir_target(&scratch.path("target"));
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let (s, stop) = (db.session("APP"), Arc::clone(&stop));
            thread::spawn(move || {
                let mut committed = Vec::new();
                for i in 0.. {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let k = i * WRITERS + w;
                    s.run(&format!("INSERT INTO T VALUES ({k}, {w})")).unwrap();
                    s.commit().unwrap();
                    committed.push((s.canceller().served().take_committed().unwrap(), k));
                }
                committed
            })
        })
        .collect();
    thread::sleep(std::time::Duration::from_millis(200));
    let Lsn(mark) = db.create_restore_point("AMONG").unwrap();
    thread::sleep(std::time::Duration::from_millis(200));
    stop.store(true, Ordering::SeqCst);
    let committed: Vec<(u64, i64)> = writers.into_iter().flat_map(|w| w.join().unwrap()).collect();
    db.archive_now().unwrap();
    let before: BTreeSet<i64> = committed.iter().filter(|(at, _)| *at < mark).map(|(_, k)| *k).collect();
    assert!(!before.is_empty() && before.len() < committed.len(), "the point falls among the commits");
    for (n, target) in [RecoveryTarget::Position(Lsn(mark)), RecoveryTarget::Point("AMONG".into())].into_iter().enumerate() {
        let dir = scratch.path(&format!("back{n}"));
        let back = restored(&RestoreOptions { archive: Some(archive.clone()), target, ..options(&dir, &location) }, &clock);
        let held: BTreeSet<i64> = rows(&back.session("APP"), "SELECT K FROM T").unwrap().iter().map(|k| k.parse().unwrap()).collect();
        assert_eq!(held, before, "target {n}");
        back.close().unwrap();
    }
}
