//! \[EXTENSION\] A host plug-in's BUFFER EVICTION and settings (RULES.md
//! D281, D283, D284): the example's policy KEEP, told its settings, keeps
//! the page they name, which least recently used evicts; and the program
//! killed stops the node, the pool reading right until then.

use std::path::Path;

use turborel::db::{Database, OpenOptions};
use turborel::value::{Decimal, Value};

use super::{crash, eventually, hosted, started};
use crate::common::{exec, rows};

/// The host plug-in's policy and its settings.
const KEEPING: &str = "\
HOST BUFFER EVICTION KEEP (EVERY = 1, SAMPLE = 64);
SETTING KEEP_FROM TYPE INTEGER MIN 0 DEFAULT 0;
SETTING KEEP_PAGES TYPE INTEGER MIN 0 DEFAULT 0;
";
/// The rows of the table, each of a wide value, so it fills several
/// times the pool's sixteen frames.
const ROWS: i32 = 120;

/// The whole number `n`.
fn whole(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A database of pages of 4096 bytes in a pool of sixteen frames, opened
/// in `dir`, its host plug-ins `spec`'s, told to keep `pages` pages from
/// page `from` once the policy says it keeps them.
fn opened(spec: &str, dir: &Path, plugin: &Path, (from, pages): (u64, u64)) -> Database {
    let db = started(spec);
    db.set_setting("HOSTED.KEEP_FROM", &whole(from)).unwrap();
    db.set_setting("HOSTED.KEEP_PAGES", &whole(pages)).unwrap();
    if pages > 0 {
        let kept = format!("keep {from} {pages}");
        assert!(eventually(|| std::fs::read_to_string(plugin.join("policy.log")).unwrap_or_default().contains(&kept)));
    }
    db.set_setting("STORAGE.PAGE_BYTES", &whole(4096)).unwrap();
    db.set_setting("STORAGE.POOL_BYTES", &whole(16 * 4096)).unwrap();
    db.open_files(&crate::common::os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// A table of `ROWS` wide rows, scanned twice.
fn worked(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER, S VARCHAR(1000))").unwrap();
    let s = db.session("U");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, '{}')", "x".repeat(900))).unwrap();
    }
    s.commit().unwrap();
    for _ in 0..2 {
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), [ROWS.to_string()]);
    }
}

/// The pages of file 0 the lines of `file` in `plugin` name.
fn pages(plugin: &Path, file: &str) -> Vec<u64> {
    let text = std::fs::read_to_string(plugin.join(file)).unwrap_or_default();
    text.lines().filter_map(|l| l.strip_prefix("0 ")).filter_map(|p| p.trim().parse().ok()).collect()
}

/// Verifies: [EXTENSION] RULES.md D281, D283, D284: a host plug-in's
/// settings are the node's, which the host sets and the program reads;
/// its eviction policy, pushed by the plug-in, changes which pages the
/// pool evicts: a page least recently used evicts under the default
/// policy is, under one weighing it 0, never evicted, and still in the
/// pool when the work is done.
#[test]
fn a_host_policy_changes_which_pages_are_evicted() {
    let (scratch, plugin, spec) = hosted("host_eviction", KEEPING);
    let db = opened(&spec, &scratch.path("lru"), &plugin, (0, 0));
    worked(&db);
    assert!(eventually(|| !pages(&plugin, "evicted.log").is_empty()), "least recently used evicted pages");
    let victim = pages(&plugin, "evicted.log").first().copied().unwrap();
    db.close().unwrap();
    drop(db);
    std::fs::remove_file(plugin.join("evicted.log")).unwrap();

    let db = opened(&spec, &scratch.path("kept"), &plugin, (victim, 1));
    worked(&db);
    assert!(eventually(|| pages(&plugin, "resident.log").contains(&victim)), "page {victim} kept");
    assert!(eventually(|| !pages(&plugin, "evicted.log").is_empty()), "other pages evicted");
    assert!(!pages(&plugin, "evicted.log").contains(&victim), "page {victim} never evicted");
    let s = db.session("U");
    assert_eq!(exec(&s, "SET DATABASE HOSTED.KEEP_PAGES = 2"), Err(turborel::error::Code::Privilege));
    db.close().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D249, D283: the eviction policy's
/// program killed stops the node, every statement after failing with
/// SQLSTATE 38T11; until then the pool reads by the last policy pushed,
/// each scan whole; and the database opened again with no host plug-in
/// gives back every row.
#[test]
fn a_host_policy_killed_stops_the_node_and_the_pool_reads_until_then() {
    let (scratch, plugin, spec) = hosted("host_eviction_kill", KEEPING);
    let dir = scratch.path("db");
    let db = opened(&spec, &dir, &plugin, (0, 4));
    worked(&db);
    let s = db.session("U");
    crash(&plugin, "advise", true);
    let mut scans = 0_u32;
    while let Ok(counted) = rows(&s, "SELECT COUNT(*) FROM T") {
        assert_eq!(counted, [ROWS.to_string()], "scan {scans} whole");
        scans = scans.saturating_add(1);
    }
    let stopped = s.run("SELECT COUNT(*) FROM T").unwrap_err();
    assert_eq!(stopped.sqlstate().as_str(), "38T11", "{stopped:?}");
    assert!(db.host_stopped().unwrap().is_some());
    drop((s, db));
    crash(&plugin, "advise", false);
    let again = Database::new();
    again.open_files(&crate::common::os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&again.session("U"), "SELECT COUNT(*) FROM T").unwrap(), [ROWS.to_string()]);
}
